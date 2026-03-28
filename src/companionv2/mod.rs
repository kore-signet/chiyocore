use alloc::{borrow::Cow, sync::Arc};
use ed25519_compact::Noise;
use embassy_sync::rwlock::RwLock;
use esp_hal::{rng::Trng, rtc_cntl::Rtc};
use lora_phy::mod_params::PacketStatus;
use meshcore::{
    Packet, PacketHeader, Path, PayloadType, RouteType, SerDeser,
    crypto::ChannelKeys,
    identity::LocalIdentity,
    payloads::{
        Ack, Advert, AdvertisementExtraData, AppdataFlags, RepeaterLogin, RequestPayload, ReturnedPath, TextHeader, TextMessageData, TextType, TracePacket
    },
    repeater_protocol::LoginResponse,
};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use smallvec::SmallVec;
use smol_str::{SmolStr, ToSmolStr};

use crate::{
    CompanionResult, EspMutex, FirmwareResult,
    companion_protocol::{
        protocol::{
            CompanionHandler, CompanionSink, NullPaddedSlice, NullPaddedString,
            responses::{
                self, ChannelInfo, CompanionProtoResult, DeviceInfo, GetMessageRes, MsgSent,
                SelfInfo,
            },
        },
        tcp::TcpCompanionSink,
    },
    companionv2::message_log::{MESSAGE_LOG_SIZE, MessageLog},
    lora::LORA_FREQUENCY_IN_HZ,
    simple_mesh::{
        SimpleMesh, SimpleMeshLayer,
        packet_log::SavedMessage,
        storage::{
            MeshStorage,
            channel::Channel,
            contact::{CachedContact, Contact},
        },
    },
    storage::{ActiveFilesystem, FS_SIZE, FsPartition, SimpleFileDb},
};

pub mod message_log;

#[derive(Serialize, Deserialize)]
pub struct CompanionConfig {
    pub wifi_ssid: SmolStr,
    pub wifi_password: SmolStr,
    pub name: SmolStr,
}

impl CompanionConfig {
    pub fn as_vars(&self) -> SmallVec<[(&str, &str); 8]> {
        smallvec::smallvec![
            ("wifi_ssid", self.wifi_ssid.as_str()),
            ("wifi_password", self.wifi_password.as_str())
        ]
    }

    pub fn set(&mut self, key: &str, val: &str) {
        match key {
            "wifi_ssid" => self.wifi_ssid = val.into(),
            "wifi_password" => self.wifi_password = val.into(),
            _ => {}
        }
    }

    pub async fn load(fs: &Arc<EspMutex<ActiveFilesystem<FS_SIZE>>>) -> FirmwareResult<Self> {
        SimpleFileDb::new(Arc::clone(fs), littlefs2::path!("/companion/"))
            .await
            .get(c"config")
            .await
            .transpose()
            .unwrap_or_else(|| {
                Ok(CompanionConfig {
                    wifi_ssid: "".into(),
                    wifi_password: "".into(),
                    name: "".into(),
                })
            })
    }
}

pub struct Companion {
    config: CompanionConfig,
    db: SimpleFileDb<FS_SIZE>,
    identity: LocalIdentity,
    storage: MeshStorage,
    mesh: Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>,
    companion_sink: TcpCompanionSink,
    rtc: Arc<Rtc<'static>>,
    message_log: MessageLog,
    signature_in_progress: Option<ed25519_compact::SigningState>,
    login_in_progress: Option<[u8; 32]>, // repeater id
}

impl Companion {
    pub async fn new(
        rtc: &Arc<Rtc<'static>>,
        shared_storage: MeshStorage,
        main_fs: &Arc<EspMutex<ActiveFilesystem<FS_SIZE>>>,
        log_fs: FsPartition<MESSAGE_LOG_SIZE>,
        mesh: &Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>,
        companion_sink: TcpCompanionSink,
    ) -> CompanionResult<Companion> {
        let cfg_db = SimpleFileDb::new(Arc::clone(main_fs), littlefs2::path!("/companion/")).await;
        let identity = mesh.read().await.identity.clone();
        let cfg = cfg_db
            .get(c"config")
            .await?
            .unwrap_or_else(|| CompanionConfig {
                wifi_ssid: SmolStr::from(""),
                wifi_password: SmolStr::from(""),
                name: SmolStr::from(
                    const_hex::const_encode::<6, false>(arrayref::array_ref![
                        identity.pubkey(),
                        0,
                        6
                    ])
                    .as_str(),
                ),
            });

        let msg_log = MessageLog::new(log_fs);

        Ok(Companion {
            config: cfg,
            companion_sink,
            identity,
            storage: shared_storage,
            mesh: Arc::clone(mesh),
            rtc: Arc::clone(rtc),
            db: cfg_db,
            message_log: msg_log,
            signature_in_progress: None,
            login_in_progress: None,
        })
    }

    async fn store_cfg(&self) -> FirmwareResult<()> {
        self.db.insert(c"config", &self.config).await?;

        Ok(())
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
        self.message_log.push(&saved).await;

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
        self.message_log.push(&saved).await;

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
            self.companion_sink.write_packet(&responses::BinaryResponse {
                data: response.into()
            }).await;
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

impl CompanionHandler for Companion {
    fn start_req(&mut self) {}

    async fn app_start<'a>(
        &'a mut self,
        _app_ver: u8,
        _reserved: [u8; 6],
        _app_name: &str,
    ) -> CompanionProtoResult<responses::SelfInfo<'a>> {
        Ok(SelfInfo {
            advertisement_type: 0,
            tx_power: 22,
            max_tx_power: 22,
            public_key: **self.mesh.read().await.identity.pubkey(),
            lat: 0,
            long: 0,
            multi_acks: 0,
            adv_loc_policy: 0,
            telemetry_mode: 0,
            manual_add_contacts: false,
            radio_freq: LORA_FREQUENCY_IN_HZ,
            radio_bandwidth: 62_500,
            radio_sf: 7,
            radio_cr: 5,
            device_name: &self.config.name,
        })
    }

    async fn device_query<'a>(
        &'a mut self,
        _app_ver: u8,
    ) -> CompanionProtoResult<responses::DeviceInfo<'a>> {
        Ok(DeviceInfo {
            fw_version: 14,
            max_contacts: 160,
            max_channels: 20,
            ble_pin: 0,
            firmware_build: NullPaddedSlice(b"nya"),
            model: NullPaddedSlice(b"bunnymesh"),
            version: NullPaddedSlice(b"0.0.0"),
            client_repeat_enabled: false,
            path_hash_mode: self.mesh.read().await.path_hash_mode as u8,
        })
    }

    async fn channel_info<'a>(
        &'a mut self,
        idx: u8,
    ) -> CompanionProtoResult<responses::ChannelInfo> {
        let channels = self.storage.channels.read().await;
        let Some(channel) = channels.get(idx) else {
            return Err(responses::Err { code: None });
        };

        Ok(ChannelInfo {
            idx,
            name: NullPaddedString(channel.name.clone()),
            secret: channel.key,
        })
    }

    async fn set_channel(
        &mut self,
        idx: u8,
        name: &str,
        secret: &[u8; 16],
    ) -> CompanionProtoResult<responses::Ok> {
        let mut channels = self.storage.channels.write().await;
        let channel_keys = ChannelKeys::from_secret(*secret);
        channels
            .insert(Channel {
                name: name.to_smolstr(),
                key: channel_keys.secret,
                hash: channel_keys.hash,
                idx,
            })
            .await;
        Ok(responses::Ok { code: None })
    }

    async fn send_channel_message(
        &mut self,
        _text_type: TextType,
        idx: u8,
        timestamp: u32,
        txt: &str,
    ) -> CompanionProtoResult<responses::MsgSent> {
        let channels = self.storage.channels.read().await;
        let Some(channel) = channels.get(idx) else {
            return Err(responses::Err { code: None });
        };

        let msg = heapless::format!(152; "{}: {}", self.config.name, txt).unwrap();

        let text = TextMessageData::plaintext(timestamp, msg.as_bytes());
        let timeout = self
            .mesh
            .read()
            .await
            .send_channel_message(&channel.as_keys(), &text)
            .await?;

        Ok(MsgSent {
            is_flood: true,
            expected_ack: [0u8; 4],
            suggested_timeout: timeout.as_millis() as u32,
        })
    }

    async fn send_contact_message(
        &mut self,
        text_type: TextType,
        attempt: u8,
        timestamp: u32,
        destination: &[u8; 6],
        text: &str,
    ) -> CompanionProtoResult<responses::MsgSent> {
        let contacts = self.storage.contacts.read().await;
        let Some(contact) = contacts.fast_get(destination) else {
            return Err(responses::Err { code: None });
        };

        let text = TextMessageData {
            timestamp,
            header: TextHeader::new()
                .with_attempt(attempt)
                .with_text_type(text_type),
            message: text.as_bytes().into(),
        };

        self.mesh
            .read()
            .await
            .send_direct_message(&contact.as_identity(), contact.path.clone(), text)
            .await
            .map_err(|e| e.into())
    }

    async fn get_time(&mut self) -> CompanionProtoResult<responses::CurrentTime> {
        Ok(responses::CurrentTime {
            time: (self.rtc.current_time_us() / 1_000_000) as u32,
        })
    }

    async fn set_time(&mut self, time: u32) -> CompanionProtoResult<responses::Ok> {
        self.rtc.set_current_time_us((time as u64) * 1_000_000);
        Ok(responses::Ok { code: None })
    }

    async fn send_self_advert(&mut self, flood: bool) -> CompanionProtoResult<responses::Ok> {
        let appdata = AdvertisementExtraData {
            flags: AppdataFlags::HAS_NAME | AppdataFlags::IS_CHAT_NODE,
            latitude: None,
            longitude: None,
            feature_1: None,
            feature_2: None,
            name: Some(self.config.name.as_bytes().into()),
        };

        let random_bytes = rand::Rng::random(&mut Trng::try_new().unwrap());
        let mesh = self.mesh.read().await;
        let advert = mesh.identity.make_advert(
            (self.rtc.current_time_us() / 1_000_000) as u32,
            appdata,
            random_bytes,
        );

        if flood {
            mesh.send_flood_packet::<Advert>(&advert).await?;
        } else {
            mesh.send_direct_packet::<Advert>(&advert, Path::empty(mesh.path_hash_mode))
                .await?;
        };

        Ok(responses::Ok { code: None })
    }

    async fn set_advert_name(&mut self, name: &str) -> CompanionProtoResult<responses::Ok> {
        self.config.name = name.to_smolstr();
        self.store_cfg().await?;
        Ok(responses::Ok { code: None })
    }

    async fn set_radio_params(
        &mut self,
        _freq: u32,
        _bandwidth: u32,
        _spreading_factor: u8,
        _coding_rate: u8,
    ) -> CompanionProtoResult<responses::Ok> {
        Err(responses::Err { code: None })
    }

    async fn set_tx_power(&mut self, _power: u8) -> CompanionProtoResult<responses::Ok> {
        Err(responses::Err { code: None })
    }

    async fn reset_path(&mut self, pk: &[u8; 32]) -> CompanionProtoResult<responses::Ok> {
        let mut contacts = self.storage.contacts.write().await;
        let Some(mut contact) = contacts.full_get(*pk).await? else {
            return Err(responses::Err { code: None });
        };

        contact.path_to = None;
        contacts.insert(contact).await?;

        Ok(responses::Ok { code: None })
    }

    async fn set_lat_long(&mut self, _lat: u32, _long: u32) -> CompanionProtoResult<responses::Ok> {
        Err(responses::Err { code: None })
    }

    async fn add_update_contact(
        &mut self,
        contact: Contact,
    ) -> CompanionProtoResult<responses::Ok> {
        let mut contacts = self.storage.contacts.write().await;
        contacts.insert(contact);

        Ok(responses::Ok { code: None })
    }

    async fn remove_contact(&mut self, contact: &[u8; 32]) -> CompanionProtoResult<responses::Ok> {
        let mut contacts = self.storage.contacts.write().await;
        contacts.delete(*contact).await;
        Ok(responses::Ok { code: None })
    }

    async fn sync_next_message<'a>(
        &'a mut self,
    ) -> CompanionProtoResult<responses::GetMessageRes<'a>> {
        Ok(match self.message_log.pop().await {
            Some(SavedMessage::Channel(v)) => GetMessageRes::Channel(v),
            Some(SavedMessage::Contact(v)) => GetMessageRes::Contact(v),
            None => GetMessageRes::NoMoreMessages,
        })
    }

    async fn get_battery(&mut self) -> CompanionProtoResult<responses::Battery> {
        Err(responses::Err { code: None })
    }

    async fn send_login(
        &mut self,
        pk: &[u8; 32],
        password: &[u8],
    ) -> CompanionProtoResult<responses::MsgSent> {
        let contacts = self.storage.contacts.read().await;
        let contact = contacts.fast_get(pk).ok_or(responses::Err { code: None })?;

        let login = RepeaterLogin {
            timestamp: (self.rtc.current_time_us() / 1_000_000) as u32,
            password: password.into(),
        };

        self.login_in_progress = Some(*pk);

        let msg_timeout = self.mesh.read().await.send_anon_req::<RepeaterLogin>(&contact.as_identity(), contact.path.clone(), &login).await?;

        Ok(responses::MsgSent {
            is_flood: contact.path.is_none(),
            expected_ack: login.timestamp.to_le_bytes(),
            suggested_timeout: msg_timeout.as_millis() as u32,
        })
    }

    async fn get_contacts<'s>(
        &'s mut self,
        _since: Option<u32>,
        out: &mut impl CompanionSink,
    ) -> CompanionProtoResult<responses::ContactEnd> {
        let contacts = self.storage.contacts.read().await;
        out.write_packet(&responses::ContactStart {
            contacts: contacts.hot_cache.len() as u32,
        })
        .await;

        for contact in &contacts.hot_cache {
            let contact = contacts.full_get(contact.key).await?.unwrap();
            out.write_packet(&contact).await;
        }

        Ok(responses::ContactEnd { last_mod: 0 })
    }

    async fn sign_start(&mut self) -> CompanionProtoResult<responses::SignStart> {
        // responses::S
        let noise = Noise::new(rand::Rng::random(&mut Trng::try_new().unwrap()));
        self.signature_in_progress = Some(self.identity.signing_keys.sk.sign_incremental(noise));

        Ok(responses::SignStart {
            reserved: 0,
            max_len: 16384,
        })
    }

    async fn sign_data(&mut self, data: &[u8]) -> CompanionProtoResult<responses::Ok> {
        let Some(signature) = self.signature_in_progress.as_mut() else {
            return Err(responses::Err { code: None });
        };
        signature.absorb(data);
        Ok(responses::Ok { code: None })
    }

    async fn sign_finish(&mut self) -> CompanionProtoResult<responses::SignatureResponse> {
        let Some(signature) = self.signature_in_progress.take() else {
            return Err(responses::Err { code: None });
        };

        Ok(responses::SignatureResponse {
            signature: *signature.sign(),
        })
    }

    async fn export_private_key(&mut self) -> CompanionProtoResult<responses::PrivateKeyResponse> {
        Ok(responses::PrivateKeyResponse {
            key: *self.identity.signing_keys.sk,
        })
    }

    async fn get_custom_vars<'s>(&'s mut self) -> CompanionProtoResult<responses::CustomVars<'s>> {
        Ok(responses::CustomVars(self.config.as_vars()))
    }

    async fn set_custom_var(
        &mut self,
        key: &str,
        val: &str,
    ) -> CompanionProtoResult<responses::Ok> {
        self.config.set(key, val);
        self.store_cfg().await?;
        Ok(responses::Ok { code: None })
    }

    async fn get_core_stats(&mut self) -> CompanionProtoResult<responses::CoreStats> {
        Err(responses::Err { code: None })
    }

    async fn get_radio_stats(&mut self) -> CompanionProtoResult<responses::RadioStats> {
        Err(responses::Err { code: None })
    }

    async fn get_packet_stats(&mut self) -> CompanionProtoResult<responses::PacketStats> {
        Err(responses::Err { code: None })
    }

    async fn send_control_data(&mut self, data: &[u8]) -> CompanionProtoResult<responses::Ok> {
        let mesh = self.mesh.read().await;
        let packet = Packet {
            header: PacketHeader::new()
                .with_payload_type(PayloadType::Control)
                .with_route_type(RouteType::Direct),
            transport_codes: None,
            path: Path::empty(mesh.path_hash_mode),
            payload: Cow::Borrowed(data),
        };

        let _timeout = mesh.send_packet(&packet, true).await?;
        Ok(responses::Ok { code: None })
    }

    async fn send_trace(
        &mut self,
        tag: [u8; 4],
        auth_code: [u8; 4],
        flags: u8,
        path: Path<'_>,
    ) -> CompanionProtoResult<responses::MsgSent> {
        let payload = TracePacket {
            tag,
            auth_code,
            flags,
            path: path.clone(),
        };

        let payload_bytes = TracePacket::encode_to_vec(&payload).unwrap();

        let packet = Packet {
            header: PacketHeader::new()
                .with_route_type(RouteType::Direct)
                .with_payload_type(PayloadType::Trace),
            transport_codes: None,
            path: Path::empty(meshcore::PathHashMode::OneByte),
            payload: payload_bytes.into(),
        };

        let timeout = packet.timeout_est(&path, RouteType::Direct);

        self.mesh.read().await.send_packet(&packet, true).await?;

        Ok(MsgSent {
            is_flood: false,
            expected_ack: tag,
            suggested_timeout: timeout.as_millis() as u32,
        })
    }

    async fn send_binary_req(&mut self, pub_key: &[u8; 32], data: &[u8]) -> CompanionProtoResult<responses::MsgSent> {
        let contacts = self.storage.contacts.read().await;
        let contact = contacts.fast_get(pub_key).ok_or(responses::Err { code: None })?;

        let time = (self.rtc.current_time_us() / 1_000_000) as u32;
        let timeout = self.mesh.read().await.send_to_contact::<RequestPayload>(&contact.as_identity(), contact.path.clone(), &RequestPayload {
            time,
            data: data.into(),
        }).await?;

        Ok(responses::MsgSent {
            is_flood: contact.path.is_some(),
            expected_ack: time.to_le_bytes(),
            suggested_timeout: timeout.as_millis() as u32
        })
    }

    async fn send_anon_req(&mut self, pub_key: &[u8; 32], data: &[u8]) -> CompanionProtoResult<responses::MsgSent> {
        let contacts = self.storage.contacts.read().await;
        let contact = contacts.fast_get(pub_key).ok_or(responses::Err { code: None })?;

        let req = RequestPayload {
            time: (self.rtc.current_time_us() / 1_000_000) as u32,
            data: data.into()
        };

        let msg_timeout = self.mesh.read().await.send_anon_req::<RequestPayload>(&contact.as_identity(), contact.path.clone(), &req).await?;
        Ok(responses::MsgSent {
            is_flood: contact.path.is_none(),
            expected_ack: req.time.to_le_bytes(),
            suggested_timeout: msg_timeout.as_millis() as u32,
        })
    }
}
