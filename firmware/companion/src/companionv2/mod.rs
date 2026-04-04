use alloc::{ffi::CString, sync::Arc};
use chiyocore::{PacketStatus, meshcore};
use embassy_sync::rwlock::RwLock;
use esp_hal::rtc_cntl::Rtc;
use litemap::LiteMap;
use meshcore::{
    Packet, SerDeser,
    identity::LocalIdentity,
    payloads::{Ack, Advert, ReturnedPath, TextMessageData},
    repeater_protocol::LoginResponse,
};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::companion_protocol::protocol::{
    ChannelCompanionSink, CompanionSink,
    responses::{self},
};
use chiyocore::{
    CompanionResult, EspMutex,
    message_log::MessageLog,
    simple_mesh::{
        SimpleMesh, SimpleMeshLayer,
        packet_log::SavedMessage,
        storage::{MeshStorage, channel::Channel, contact::CachedContact},
    },
    storage::{ActiveFilesystem, FS_SIZE, PersistedObject, SimpleFileDb},
};

mod build;
pub mod companion_proto_handler;

#[derive(Serialize, Deserialize, Default)]
pub struct CompanionConfig {
    pub name: SmolStr,
}

pub struct Companion {
    companion_config: PersistedObject<CompanionConfig, FS_SIZE>,
    global_config: PersistedObject<LiteMap<SmolStr, SmolStr>, FS_SIZE>,
    identity: LocalIdentity,
    storage: MeshStorage,
    mesh: Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>,
    companion_sink: ChannelCompanionSink,
    rtc: Arc<Rtc<'static>>,
    message_log: Arc<EspMutex<MessageLog>>,
    signature_in_progress: Option<ed25519_compact::SigningState>,
    login_in_progress: Option<[u8; 32]>, // repeater id
}

impl Companion {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        pfx: &str,
        rtc: &Arc<Rtc<'static>>,
        shared_storage: MeshStorage,
        global_cfg_db: &SimpleFileDb<FS_SIZE>,
        main_fs: &Arc<EspMutex<ActiveFilesystem<FS_SIZE>>>,
        msg_log: &Arc<EspMutex<MessageLog>>,
        mesh: &Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>,
        companion_sink: ChannelCompanionSink,
    ) -> CompanionResult<Companion> {
        let pfx = alloc::format!("/companion/{pfx}/");
        let pfx = CString::new(pfx).unwrap();

        let cfg_db = SimpleFileDb::new(
            Arc::clone(main_fs),
            littlefs2::path::Path::from_cstr(&pfx).unwrap(),
        )
        .await;
        let identity = mesh.read().await.identity.clone();
        let cfg = cfg_db
            .get_persistable::<CompanionConfig>(c"cfg", || CompanionConfig {
                name: const_hex::const_encode::<6, false>(arrayref::array_ref![
                    identity.pubkey(),
                    0,
                    6
                ])
                .as_str()
                .into(),
            })
            .await?;

        let global_cfg = global_cfg_db
            .get_persistable(c"general", LiteMap::new)
            .await?;

        // let msg_log = MessageLog::new(log_fs);

        Ok(Companion {
            companion_config: cfg,
            global_config: global_cfg,
            companion_sink,
            identity,
            storage: shared_storage,
            mesh: Arc::clone(mesh),
            rtc: Arc::clone(rtc),
            message_log: Arc::clone(msg_log),
            signature_in_progress: None,
            login_in_progress: None,
        })
    }
}

impl SimpleMeshLayer for Companion {
    async fn packet<'f>(
        &'f mut self,
        _mesh: &'f SimpleMesh,
        _packet: &'f Packet<'_>,
        packet_bytes: &'f [u8],
        packet_status: PacketStatus,
    ) -> CompanionResult<()> {
        self.companion_sink
            .write_packet(&responses::RfLogData::new(packet_status, packet_bytes))
            .await;
        Ok(())
    }

    async fn text_message<'f>(
        &'f mut self,
        _mesh: &'f SimpleMesh,
        packet: &'f meshcore::Packet<'_>,
        _packet_status: PacketStatus,
        contact: &'f CachedContact,
        message: &'f TextMessageData<'_>,
    ) -> CompanionResult<()> {
        let saved = SavedMessage::contact_msg(contact, packet, message);
        self.companion_sink.write_packet(&saved).await;
        self.message_log.lock().await.push(&saved).await;

        log::info!("DM [{:x}]: {}", contact.key[0], message.as_utf8()?);

        Ok(())
    }

    async fn group_text<'f>(
        &'f mut self,
        _mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        _packet_status: PacketStatus,
        channel: &'f Channel,
        message: &'f TextMessageData<'_>,
    ) -> CompanionResult<()> {
        let saved = SavedMessage::channel_msg(channel, packet, message);
        self.companion_sink.write_packet(&saved).await;
        self.message_log.lock().await.push(&saved).await;

        log::info!("group-text [{}]: {}", channel.name, message.as_utf8()?);

        Ok(())
    }

    async fn ack<'f>(
        &'f mut self,
        _mesh: &'f SimpleMesh,
        _packet: &'f Packet<'_>,
        _packet_status: PacketStatus,
        ack: &'f Ack,
    ) -> CompanionResult<()> {
        self.companion_sink
            .write_packet(&responses::Ack { code: ack.crc })
            .await;
        Ok(())
    }

    async fn advert<'f>(
        &'f mut self,
        _mesh: &'f SimpleMesh,
        _packet: &'f Packet<'_>,
        _packet_status: PacketStatus,
        _advert: &'f Advert<'_>,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn returned_path<'f>(
        &'f mut self,
        _mesh: &'f SimpleMesh,
        _packet: &'f Packet<'_>,
        _packet_status: PacketStatus,
        _contact: &'f CachedContact,
        _path: &'f ReturnedPath<'_>,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn response<'f>(
        &'f mut self,
        _mesh: &'f SimpleMesh,
        _packet: &'f Packet<'_>,
        _packet_status: PacketStatus,
        contact: &'f CachedContact,
        response: &'f [u8],
    ) -> CompanionResult<()> {
        if let Some(rptr_key) = self.login_in_progress.take_if(|v| *v == contact.key) {
            // response is a login resp
            let Ok(res) = LoginResponse::decode(response) else {
                log::error!("failed to decode login response");
                return Ok(());
            };
            log::info!("response: {:?}", res);

            if res.response_code == 0 {
                // LOGIN_SUCCESS
                self.companion_sink
                    .write_packet(&responses::LoginSuccess {
                        permissions: res.permissions,
                        prefix: *arrayref::array_ref![rptr_key, 0, 6],
                    })
                    .await;
            }
        } else {
            self.companion_sink
                .write_packet(&responses::BinaryResponse {
                    data: response.into(),
                })
                .await;
        }

        Ok(())
    }

    async fn request<'f>(
        &'f mut self,
        _mesh: &'f SimpleMesh,
        _packet: &'f Packet<'_>,
        _packet_status: PacketStatus,
        _contact: &'f CachedContact,
        _request: &'f meshcore::payloads::RequestPayload<'_>,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn anonymous_request<'f>(
        &'f mut self,
        _mesh: &'f SimpleMesh,
        _packet: &'f Packet<'_>,
        _packet_status: PacketStatus,
        _contact: &'f meshcore::identity::ForeignIdentity,
        _data: &'f [u8],
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn trace_packet<'f>(
        &'f mut self,
        _mesh: &'f SimpleMesh,
        _packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        snrs: &'f [i8],
        trace: &'f meshcore::payloads::TracePacket<'_>,
    ) -> CompanionResult<()> {
        if snrs.len() < trace.path.len() {
            return Ok(());
        }

        self.companion_sink
            .write_packet(&responses::TraceData {
                reserved: 0,
                flags: trace.flags,
                tag: trace.tag,
                auth_code: trace.auth_code,
                path: trace.path.clone(),
                snrs: snrs.into(),
                last_snr: packet_status.snr as i8 * 4,
            })
            .await;

        Ok(())
    }

    async fn control_packet<'f>(
        &'f mut self,
        _mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        _payload: &'f meshcore::payloads::ControlPayload,
    ) -> CompanionResult<()> {
        self.companion_sink
            .write_packet(&responses::ControlData {
                snr: (packet_status.snr * 4) as i8,
                rssi: packet_status.rssi as i8,
                path_len: packet.path.len() as u8,
                payload: (&packet.payload[..]).into(),
            })
            .await;

        Ok(())
    }
}
