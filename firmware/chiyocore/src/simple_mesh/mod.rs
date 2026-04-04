use core::{ops::DerefMut, time::Duration};

use crate::PacketStatus;
use alloc::{borrow::Cow, string::String, sync::Arc, vec::Vec};
use arrayref::array_ref;
use embassy_executor::SendSpawner;
use embassy_sync::rwlock::RwLock;
use esp_hal::rtc_cntl::Rtc;
use futures_util::FutureExt;
use maitake_sync::{WaitCell, WaitMap};
use meshcore::{
    Packet, PacketHeader, PacketPayload, Path, PathHashMode, PayloadType, RouteType, SerDeser,
    crypto::{ChannelKeys, ContainsEncryptable, DecryptedView, Encryptable, VerifiablePayload},
    identity::{ForeignIdentity, LocalIdentity},
    io::ByteVecImpl,
    payloads::{
        Ack, Advert, AnonymousRequest, ControlPayload, EncryptedMessageWithDst, GroupText,
        RequestPayload, ResponsePayload, ReturnedPath, TextMessageData, TracePacket,
    },
};

use crate::{
    BumpaloVec, CompanionError, CompanionResult, EspMutex, MeshcoreHandler,
    crypto::{HardwareAES, HardwareHMAC, HardwareSHA},
    lora::LoraTaskChannel,
    simple_mesh::{
        packet_log::{HashLog, SavedMessage},
        storage::{
            MeshStorage,
            channel::Channel,
            contact::{CachedContact, Contact},
            shared_key_cache::SharedKeyCache,
        },
    },
};

pub mod packet_log;
pub mod storage;

pub struct MsgSent {
    pub is_flood: bool,
    pub expected_ack: [u8; 4],
    pub suggested_timeout: u32,
}

#[derive(Clone, Copy)]
pub struct AckState {
    pub failed: bool,
    pub attempt: u8,
}

pub struct SimpleMesh {
    pub identity: LocalIdentity,
    pub packet_log: HashLog<32>,
    pub message_log: HashLog<32>,
    pub lora_tx: LoraTaskChannel,
    pub scratch: bumpalo::Bump,
    pub path_hash_mode: PathHashMode,
    pub storage: MeshStorage,
    pub rtc: Arc<Rtc<'static>>,
    ack_table: Arc<WaitMap<[u8; 4], bool>>,
    key_cache: SharedKeyCache,
}

impl SimpleMesh {
    pub fn new(
        identity: LocalIdentity,
        storage: MeshStorage,
        lora_tx: LoraTaskChannel,
        rtc: &Arc<Rtc<'static>>,
    ) -> SimpleMesh {
        SimpleMesh {
            key_cache: SharedKeyCache::new(&identity),
            identity,
            packet_log: HashLog::new(),
            message_log: HashLog::new(),
            lora_tx,
            scratch: bumpalo::Bump::new(),
            path_hash_mode: PathHashMode::OneByte,
            rtc: Arc::clone(rtc),
            storage,
            ack_table: Arc::new(WaitMap::new()), // ack_table: LiteMap::new_vec()
        }
    }
}

#[embassy_executor::task(pool_size = 16)]
async fn send_with_retry(
    wait_map: Arc<WaitMap<[u8; 4], bool>>,
    mut message: TextMessageData<'static>,
    contact: ForeignIdentity,
    mesh: Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>,
    mut path: Option<meshcore::Path<'static>>,
    waker: Arc<WaitCell>,
) {
    let self_identity = mesh.read().await.identity.clone();
    let mut attempt = 0u8;
    let mut has_flooded = false;
    // let mut has_flooded = path.is_none();
    while attempt <= 3 {
        log::info!("retrying msg, attempt {attempt}");
        message.header = message.header.with_attempt(attempt);

        let expected_ack =
            Ack::calculate::<HardwareSHA>(&message, &self_identity.as_foreign()).await;

        let delay = mesh
            .read()
            .await
            .send_to_contact::<TextMessageData<'_>>(&contact, path.clone(), &message, None)
            .await
            .unwrap();

        let delay = delay.max(Duration::from_secs(1));

        match embassy_futures::select::select(
            wait_map.wait(expected_ack.crc),
            embassy_time::Timer::after(embassy_time::Duration::from_millis(
                delay.as_millis() as u64
            )),
        )
        .await
        {
            embassy_futures::select::Either::First(_) => {
                waker.wake();
                break;
            }
            embassy_futures::select::Either::Second(_) => {
                attempt += 1;
                if attempt >= 3 && !has_flooded {
                    attempt = 0;
                    let mesh = mesh.read().await;
                    let mut contacts = mesh.storage.contacts.write().await;
                    let mut contact = contacts
                        .full_get(*contact.verify_key)
                        .await
                        .unwrap()
                        .unwrap();
                    contact.path_to = None;
                    contacts.insert(contact).await.unwrap();

                    path = None;
                    has_flooded = true;
                }
            }
        }
    }

    waker.wake();
}

/* tx methods */
impl SimpleMesh {
    pub async fn send_to_contact_with_retry<P: SerDeser + Encryptable + PacketPayload>(
        mesh: &Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>,
        contact: &ForeignIdentity,
        path: Option<meshcore::Path<'static>>,
        message: &TextMessageData<'_>,
    ) -> CompanionResult<Arc<WaitCell>> {
        let message = TextMessageData {
            timestamp: message.timestamp,
            header: message.header,
            message: message.message.clone().into_owned().into(),
        };
        let identity = contact.clone();
        let mesh = Arc::clone(mesh);

        let wait_map = Arc::clone(&mesh.read().await.ack_table);
        let wait_cell = Arc::new(WaitCell::new());

        SendSpawner::for_current_executor()
            .await
            .spawn(send_with_retry(
                wait_map,
                message,
                identity,
                mesh,
                path,
                Arc::clone(&wait_cell),
            ))
            .unwrap();

        Ok(wait_cell)
    }

    /// returns est. timeout
    pub async fn send_to_contact<P: SerDeser + Encryptable + PacketPayload>(
        &self,
        contact: &ForeignIdentity,
        path: Option<meshcore::Path<'_>>,
        message: &P::Representation<'_>,
        delay: Option<Duration>,
    ) -> CompanionResult<Duration> {
        let mut encrypt_scratch = BumpaloVec::new_in(&self.scratch);
        let mut encode_vec = BumpaloVec::new_in(&self.scratch);

        let shared_key = self.key_cache.get_key(contact).await;

        EncryptedMessageWithDst::encode_into_vec(
            &self
                .identity
                .make_message_with_key::<P, HardwareAES>(
                    message,
                    contact.verify_key[0],
                    shared_key,
                    &mut encrypt_scratch,
                )
                .await?,
            &mut encode_vec,
        )
        .unwrap();

        let route_type = if path.is_some() {
            RouteType::Direct
        } else {
            RouteType::Flood
        };

        let packet = Packet {
            header: PacketHeader::new()
                .with_payload_type(<EncryptedMessageWithDst<'_, P>>::PAYLOAD_TYPE)
                .with_route_type(route_type),
            transport_codes: None,
            path: path.unwrap_or(Path::empty(self.path_hash_mode)),
            payload: Cow::Borrowed(&encode_vec),
        };

        let timeout = self.send_packet(&packet, true, delay).await?;

        Ok(timeout)
    }

    pub async fn send_anon_req<P: SerDeser + Encryptable>(
        &self,
        contact: &ForeignIdentity,
        path: Option<meshcore::Path<'_>>,
        message: &P::Representation<'_>,
    ) -> CompanionResult<Duration> {
        let mut scratch = BumpaloVec::new_in(&self.scratch);
        let anon_req = self
            .identity
            .make_anon_req::<P, HardwareAES>(message, contact, &mut scratch)
            .await
            .unwrap();
        let anon_req_bytes = AnonymousRequest::encode_to_vec(&anon_req).unwrap();

        let packet = Packet {
            header: PacketHeader::new()
                .with_payload_type(PayloadType::AnonReq)
                .with_route_type(if path.is_some() {
                    meshcore::RouteType::Direct
                } else {
                    meshcore::RouteType::Flood
                }),
            transport_codes: None,
            path: path.unwrap_or(Path::empty(self.path_hash_mode)),
            payload: anon_req_bytes.into(),
        };

        self.packet_log.push(&packet).await;

        let timeout = self.lora_tx.send_packet(&packet).await;

        Ok(timeout)
    }

    pub async fn send_direct_message(
        &self,
        contact: &ForeignIdentity,
        path: Option<meshcore::Path<'_>>, // if None, flood
        message: TextMessageData<'_>,
        delay: Option<Duration>,
    ) -> CompanionResult<MsgSent> {
        let is_flood = path.is_none();
        let ack = Ack::calculate::<HardwareSHA>(&message, &self.identity.as_foreign()).await;
        let timeout = self
            .send_to_contact::<TextMessageData>(contact, path, &message, delay)
            .await?;

        Ok(MsgSent {
            is_flood,
            expected_ack: ack.crc,
            suggested_timeout: timeout.as_millis() as u32,
        })
    }

    pub async fn send_channel_message(
        &self,
        channel: &ChannelKeys,
        text: &TextMessageData<'_>,
        delay: Option<Duration>,
    ) -> CompanionResult<Duration> {
        let mut scratch = BumpaloVec::new_in(&self.scratch);
        TextMessageData::encrypt::<HardwareAES>(text, &channel.secret, &mut scratch).await?;

        let group_text = GroupText::new(channel.hash, &*scratch, &channel.secret);

        let timeout = self
            .send_flood_packet::<GroupText>(&group_text, delay)
            .await?;

        Ok(timeout)
    }

    pub async fn send_flood_packet<P: PacketPayload>(
        &self,
        payload: &P::Representation<'_>,
        delay: Option<Duration>,
    ) -> CompanionResult<Duration> {
        let mut scratch = BumpaloVec::new_in(&self.scratch);
        let _ = P::encode_into_vec(payload, &mut scratch);

        let packet = Packet {
            header: PacketHeader::new()
                .with_payload_type(P::PAYLOAD_TYPE)
                .with_route_type(RouteType::Flood),
            transport_codes: None,
            path: Path::empty(self.path_hash_mode),
            payload: Cow::Borrowed(&scratch),
        };

        self.send_packet(&packet, true, delay).await
    }

    pub async fn send_direct_packet<P: PacketPayload>(
        &self,
        payload: &P::Representation<'_>,
        path: Path<'_>,
        _delay: Option<Duration>,
    ) -> CompanionResult<Duration> {
        let mut scratch = BumpaloVec::new_in(&self.scratch);
        let _ = P::encode_into_vec(payload, &mut scratch);

        let packet = Packet {
            header: PacketHeader::new()
                .with_payload_type(P::PAYLOAD_TYPE)
                .with_route_type(RouteType::Direct),
            transport_codes: None,
            path,
            payload: Cow::Borrowed(&scratch),
        };

        self.packet_log.push(&packet).await;

        let timeout = self.lora_tx.send_packet(&packet).await;

        Ok(timeout)
    }

    pub async fn send_packet(
        &self,
        packet: &Packet<'_>,
        log: bool,
        delay: Option<Duration>,
    ) -> CompanionResult<Duration> {
        if log {
            self.packet_log.push(packet).await;
        }

        Ok(if let Some(delay) = delay {
            self.lora_tx.send_delayed(packet, delay).await
        } else {
            self.lora_tx.send_packet(packet).await
        })
    }

    async fn send_ack(
        &self,
        packet: &Packet<'_>,
        txt_decoded: &TextMessageData<'_>,
        contact: &CachedContact,
    ) -> CompanionResult<()> {
        let ack = Ack::calculate::<HardwareSHA>(txt_decoded, &contact.as_identity()).await;
        let mut ack_scratch = BumpaloVec::new_in(&self.scratch);
        let mut extra_scratch = BumpaloVec::new_in(&self.scratch);
        let ack_packet: Packet<'_> = match packet.header.route_type() {
            RouteType::TransportFlood | RouteType::Flood => {
                let returned_path = ReturnedPath {
                    path: packet.path.clone(),
                    extra: Some((PayloadType::Ack, Cow::Borrowed(&ack.crc))),
                };

                let encrypted = self
                    .identity
                    .make_message::<ReturnedPath, HardwareAES>(
                        &returned_path,
                        &contact.as_identity(),
                        &mut extra_scratch,
                    )
                    .await
                    .unwrap();

                let payload_buf = <EncryptedMessageWithDst<ReturnedPath>>::encode_into_vec(
                    &encrypted,
                    &mut ack_scratch,
                )
                .unwrap();

                Packet::flood::<EncryptedMessageWithDst<ReturnedPath>>(
                    Path::empty(PathHashMode::OneByte),
                    payload_buf,
                )
            }
            RouteType::Direct | RouteType::TransportDirect => {
                let ack_bytes = Ack::encode_into_vec(&ack, &mut ack_scratch).unwrap();
                Packet::direct::<Ack>(packet.path.clone(), ack_bytes)
            }
        };

        let rx_time = crate::timing::rx_retransmit_delay(packet);

        self.send_packet(&ack_packet, false, Some(rx_time)).await?;

        Ok(())
    }
}

pub trait SimpleMeshLayer {
    fn packet<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_bytes: &'f [u8],
        packet_status: PacketStatus,
    ) -> impl Future<Output = CompanionResult<()>>;

    fn text_message<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        message: &'f TextMessageData<'_>,
    ) -> impl Future<Output = CompanionResult<()>>;

    fn group_text<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        channel: &'f Channel,
        message: &'f TextMessageData<'_>,
    ) -> impl Future<Output = CompanionResult<()>>;

    fn ack<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        ack: &'f Ack,
    ) -> impl Future<Output = CompanionResult<()>>;

    fn advert<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        advert: &'f Advert<'_>,
    ) -> impl Future<Output = CompanionResult<()>>;

    fn returned_path<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        path: &'f ReturnedPath<'_>,
    ) -> impl Future<Output = CompanionResult<()>>;

    fn response<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        response: &'f [u8],
    ) -> impl Future<Output = CompanionResult<()>>;

    fn request<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        request: &'f RequestPayload<'_>,
    ) -> impl Future<Output = CompanionResult<()>>;

    fn anonymous_request<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f ForeignIdentity,
        data: &'f [u8],
    ) -> impl Future<Output = CompanionResult<()>>;

    fn trace_packet<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        snrs: &'f [i8],
        trace: &'f TracePacket<'_>,
    ) -> impl Future<Output = CompanionResult<()>>;

    fn control_packet<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        payload: &'f ControlPayload,
    ) -> impl Future<Output = CompanionResult<()>>;
}

impl SimpleMesh {
    pub async fn decode_contact_message<'a, 'b, T: SerDeser + Encryptable>(
        &self,
        payload: &EncryptedMessageWithDst<'_, T>,
        scratch: &'a mut BumpaloVec<'b, u8>,
    ) -> CompanionResult<Option<(CachedContact, DecryptedView<'a, T>)>> {
        log::trace!(
            "\ttext msg | {src:x}->{dst:x}",
            src = payload.source_hash,
            dst = payload.destination_hash
        );

        if self.identity.pubkey()[0] != payload.destination_hash {
            return Ok(None);
        }

        let contacts_storage = self.storage.contacts.read().await;
        let Some(mut contact_idx) = contacts_storage.find_idx(&[payload.source_hash]) else {
            return Err(CompanionError::NoKnownContact);
        };

        while let Some(other_ident) = contacts_storage
            .hot_cache
            .get(contact_idx)
            .filter(|v| v.key[0] == payload.source_hash)
        {
            let shared_secret = self.key_cache.get_key(&other_ident.as_identity()).await;

            if !payload.verify::<HardwareHMAC>(array_ref![&shared_secret, 0, 32]) {
                log::error!("\tmessage failed verify, trying next contact");
                contact_idx += 1;
                continue;
            }

            let Ok(msg) = payload
                .decrypt::<HardwareAES>(array_ref![&shared_secret, 0, 16], scratch)
                .await
            else {
                log::error!("\tmessage failed to decrypt");
                return Err(CompanionError::DecryptFailure);
            };

            return Ok(Some((other_ident.clone(), msg)));
        }

        Err(CompanionError::NoKnownContact)
    }

    async fn decode_channel_message<'s>(
        &self,
        payload: &GroupText<'_>,
        scratch: &'s mut impl ByteVecImpl,
    ) -> CompanionResult<Option<(Channel, DecryptedView<'s, TextMessageData<'static>>)>> {
        log::trace!("\tgroup msg | channel: {:x}", payload.channel);
        let Some(channel) = self
            .storage
            .channels
            .read()
            .await
            .get_by_hash(payload.channel)
            .cloned()
        else {
            log::error!("\t(no keys for channel)");
            return Err(CompanionError::NoKnownChannel);
        };

        log::trace!("\t<{}>", channel.name);
        let verify = payload.verify::<HardwareHMAC>(&channel.key);
        log::trace!("\tverify: {verify}");
        if !verify {
            log::error!("\tmac check failed, returning");
            return Err(CompanionError::VerifyFailure);
        }

        let Ok(txt_bytes) = payload.decrypt::<HardwareAES>(&channel.key, scratch).await else {
            log::error!("\tfailed decrypt");
            return Err(CompanionError::DecryptFailure);
        };

        Ok(Some((channel, txt_bytes)))
    }
}

/* rx methods */
impl SimpleMesh {
    async fn text_message(
        &self,
        packet: &Packet<'_>,
        packet_status: PacketStatus,
        message: EncryptedMessageWithDst<'_, TextMessageData<'static>>,
        layers: &mut impl SimpleMeshLayer,
    ) -> CompanionResult<()> {
        let mut decrypt_scratch = BumpaloVec::new_in(&self.scratch);

        let Some((contact, text)) = self
            .decode_contact_message::<TextMessageData<'_>>(&message, &mut decrypt_scratch)
            .await?
        else {
            return Ok(());
        };

        let text = text.decoded()?;
        self.send_ack(packet, &text, &contact).await?;
        if !self
            .message_log
            .push(&SavedMessage::contact_msg(&contact, packet, &text))
            .await
        {
            return Ok(());
        }

        // for layer in layers {
        layers
            .text_message(self, packet, packet_status, &contact, &text)
            .await?;

        Ok(())
    }

    async fn group_text(
        &self,
        packet: &Packet<'_>,
        packet_status: PacketStatus,
        message: GroupText<'_>,
        layers: &mut impl SimpleMeshLayer,
    ) -> CompanionResult<()> {
        let mut decrypt_scratch = BumpaloVec::new_in(&self.scratch);
        let Some((channel, text)) = self
            .decode_channel_message(&message, &mut decrypt_scratch)
            .await?
        else {
            return Ok(());
        };

        let text = text.decoded()?;

        if !self
            .message_log
            .push(&SavedMessage::channel_msg(&channel, packet, &text))
            .await
        {
            return Ok(());
        }

        // for layer in layers {
        layers
            .group_text(self, packet, packet_status, &channel, &text)
            .await?;
        // }

        Ok(())
    }

    async fn ack(
        &self,
        packet: &Packet<'_>,
        packet_status: PacketStatus,
        ack: Ack,
        layers: &mut impl SimpleMeshLayer,
    ) -> CompanionResult<()> {
        self.ack_table.wake(&ack.crc, true);

        layers.ack(self, packet, packet_status, &ack).await?;
        // }

        Ok(())
    }

    async fn advert(
        &self,
        packet: &Packet<'_>,
        packet_status: PacketStatus,
        payload: Advert<'_>,
        layers: &mut impl SimpleMeshLayer,
    ) -> CompanionResult<()> {
        log::info!("\tadvert | from {:x}", payload.public_key[0]);
        let Some(appdata) = payload.appdata.as_ref() else {
            return Ok(());
        };

        let Some(name) = appdata
            .name
            .as_ref()
            .and_then(|v| core::str::from_utf8(v).ok())
        else {
            return Ok(());
        };
        log::info!("\tname: {name}");

        self.storage
            .contacts
            .write()
            .await
            .insert(Contact {
                key: payload.public_key,
                name: String::from(name),
                path_to: None,
                flags: appdata.flags.bits(),
                latitude: appdata.latitude.unwrap_or(0),
                longitude: appdata.longitude.unwrap_or(0),
                last_heard: (self.rtc.current_time_us() / 1_000_000) as u32,
            })
            .await?;

        // for layer in layers {
        layers.advert(self, packet, packet_status, &payload).await?;
        // }

        Ok(())
    }

    async fn returned_path(
        &self,
        packet: &Packet<'_>,
        packet_status: PacketStatus,
        message: EncryptedMessageWithDst<'_, ReturnedPath<'static>>,
        layers: &mut impl SimpleMeshLayer,
    ) -> CompanionResult<()> {
        let mut decrypt_scratch = BumpaloVec::new_in(&self.scratch);
        let Some((contact, path)) = self
            .decode_contact_message::<ReturnedPath<'_>>(&message, &mut decrypt_scratch)
            .await?
        else {
            return Ok(());
        };

        let decoded = path.decoded()?;

        let mut contacts_db = self.storage.contacts.write().await;
        let mut contact_full = contacts_db.full_get(contact.key).await?.unwrap();
        contact_full.path_to = Some(decoded.path.to_owned());
        contacts_db.insert(contact_full).await?;

        layers
            .returned_path(self, packet, packet_status, &contact, &decoded)
            .await?;

        let Some((extra_ty, extra_bytes)) = decoded.extra.as_ref() else {
            return Ok(());
        };
        match extra_ty {
            PayloadType::Ack => {
                self.ack(
                    packet,
                    packet_status,
                    decoded.decode_payload_as::<Ack>()?,
                    layers,
                )
                .await?
            }
            PayloadType::Response => {
                layers
                    .response(self, packet, packet_status, &contact, extra_bytes)
                    .await?
            }
            _ => {}
        }

        Ok(())
    }

    async fn response(
        &self,
        packet: &Packet<'_>,
        packet_status: PacketStatus,
        message: EncryptedMessageWithDst<'_, ResponsePayload<'static, Cow<'_, [u8]>>>,
        layers: &mut impl SimpleMeshLayer,
    ) -> CompanionResult<()> {
        let mut decrypt_scratch = BumpaloVec::new_in(&self.scratch);
        let Some((contact, res)) = self
            .decode_contact_message::<ResponsePayload<'_, Cow<'_, [u8]>>>(
                &message,
                &mut decrypt_scratch,
            )
            .await?
        else {
            return Ok(());
        };

        log::info!("decoded response");

        let decoded = res.decoded()?;
        layers
            .response(self, packet, packet_status, &contact, &decoded.data)
            .await?;

        Ok(())
    }

    async fn request(
        &self,
        packet: &Packet<'_>,
        packet_status: PacketStatus,
        message: EncryptedMessageWithDst<'_, RequestPayload<'static>>,
        layers: &mut impl SimpleMeshLayer,
    ) -> CompanionResult<()> {
        let mut decrypt_scratch = BumpaloVec::new_in(&self.scratch);
        let Some((contact, res)) = self
            .decode_contact_message::<RequestPayload<'_>>(&message, &mut decrypt_scratch)
            .await?
        else {
            return Ok(());
        };

        log::info!("decoded request");

        let decoded = res.decoded()?;
        layers
            .request(self, packet, packet_status, &contact, &decoded)
            .await?;

        Ok(())
    }

    async fn anonymous_request(
        &self,
        packet: &Packet<'_>,
        packet_status: PacketStatus,
        message: AnonymousRequest<'_, Cow<'static, [u8]>>,
        layers: &mut impl SimpleMeshLayer,
    ) -> CompanionResult<()> {
        if message.destination_hash != self.identity.pubkey()[0] {
            return Ok(());
        }

        let mut decrypt_scratch = BumpaloVec::new_in(&self.scratch);

        let other_ident = ForeignIdentity::new(message.sender_key);
        let shared_key = self.identity.shared_secret(&other_ident);

        if !message.verify::<HardwareHMAC>(array_ref![&shared_key, 16, 16]) {
            log::error!("\tanon req failed verify");
            return Err(CompanionError::VerifyFailure);
        }

        let decrypted = message
            .decrypt::<HardwareAES>(array_ref![&shared_key, 0, 16], &mut decrypt_scratch)
            .await?;
        let decoded = decrypted.decoded()?;
        layers
            .anonymous_request(self, packet, packet_status, &other_ident, &decoded)
            .await?;

        Ok(())
    }

    async fn control_packet(
        &self,
        packet: &Packet<'_>,
        packet_status: PacketStatus,
        message: ControlPayload,
        layers: &mut impl SimpleMeshLayer,
    ) -> CompanionResult<()> {
        layers
            .control_packet(self, packet, packet_status, &message)
            .await?;
        Ok(())
    }

    async fn trace_packet(
        &self,
        packet: &Packet<'_>,
        packet_status: PacketStatus,
        message: TracePacket<'_>,
        layers: &mut impl SimpleMeshLayer,
    ) -> CompanionResult<()> {
        let snrs = unsafe { core::mem::transmute::<&[u8], &[i8]>(packet.path.raw_bytes()) };

        layers
            .trace_packet(self, packet, packet_status, snrs, &message)
            .await?;

        log::info!(
            "\ttrace | path: {:?} | snrs: {:?}",
            message.path,
            snrs.iter().map(|v| *v as f32 / 4.0).collect::<Vec<_>>()
        );

        Ok(())
    }
}

impl MeshcoreHandler for SimpleMesh {
    type Error = CompanionError;

    async fn packet(
        &mut self,
        packet: &Packet<'_>,
        packet_status: lora_phy::mod_params::PacketStatus,
        bytes: &[u8],
        extra: &mut impl SimpleMeshLayer,
    ) -> Result<(), Self::Error> {
        if !self.packet_log.push(packet).await {
            // already seen
            return Ok(());
        }

        // for layer in &mut extra[..] {
        extra.packet(self, packet, bytes, packet_status).await?;
        // }
        //
        match packet.header.payload_type() {
            PayloadType::Request => {
                self.request(
                    packet,
                    packet_status,
                    packet
                        .decode_payload_as::<EncryptedMessageWithDst<'_, RequestPayload<'static>>>(
                        )?,
                    extra,
                )
                .await?;
            }
            PayloadType::Response => {
                self
                    .response(
                        packet,
                        packet_status,
                        packet.decode_payload_as::<EncryptedMessageWithDst<
                            '_,
                            ResponsePayload<'static, Cow<'static, [u8]>>,
                        >>()?,
                        extra,
                    )
                    .await?;
            }
            PayloadType::TxtMsg => {
                self.text_message(
                    packet,
                    packet_status,
                    packet
                        .decode_payload_as::<EncryptedMessageWithDst<'_, TextMessageData<'static>>>(
                        )?,
                    extra,
                )
                .await?;
            }
            PayloadType::Ack => {
                self.ack(
                    packet,
                    packet_status,
                    packet.decode_payload_as::<Ack>()?,
                    extra,
                )
                .await?;
            }
            PayloadType::Advert => {
                self.advert(
                    packet,
                    packet_status,
                    packet.decode_payload_as::<Advert<'_>>()?,
                    extra,
                )
                .await?;
            }
            PayloadType::GrpTxt => {
                self.group_text(
                    packet,
                    packet_status,
                    packet.decode_payload_as::<GroupText<'_>>()?,
                    extra,
                )
                .await?;
            }
            PayloadType::AnonReq => {
                self.anonymous_request(
                    packet,
                    packet_status,
                    packet.decode_payload_as::<AnonymousRequest<'_, Cow<'static, [u8]>>>()?,
                    extra,
                )
                .await?;
            }
            PayloadType::Path => {
                self.returned_path(
                    packet,
                    packet_status,
                    packet
                        .decode_payload_as::<EncryptedMessageWithDst<'_, ReturnedPath<'static>>>(
                        )?,
                    extra,
                )
                .await?;
            }
            PayloadType::Trace => {
                // TODO: pretty sure this is broken for multi-byte paths
                self.trace_packet(
                    packet,
                    packet_status,
                    packet.decode_payload_as::<TracePacket<'_>>()?,
                    extra,
                )
                .await?;
            }
            PayloadType::Multipart => {}
            PayloadType::Control => {
                self.control_packet(
                    packet,
                    packet_status,
                    packet.decode_payload_as::<ControlPayload>()?,
                    extra,
                )
                .await?;
            }
            PayloadType::RawCustom => {}
        }

        self.scratch.reset();
        Ok(())
    }
}

//     pub struct SimpleCompanion<B: CompanionLayer + Send> {
//     pub identity: StoredIdentity,
//     pub config: CompanionConfig,
//     pub lora_tx: LoraTaskChannel,
//     pub companion_tx: RefCell<TcpCompanionSink>,
//     pub rtc: Rtc<'static>,
//     pub scratch: bumpalo::Bump,
//     pub(crate) signature_in_progress: Option<SigningState>,
//     pub bot: RefCell<B>,
//     pub storage: SimpleCompanionStorage,
//     pub stats: SimpleCompanionStats,
// }

// }

/* da tuple zone */

impl<A> SimpleMeshLayer for &mut A
where
    A: SimpleMeshLayer,
{
    fn packet<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_bytes: &'f [u8],
        packet_status: PacketStatus,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::packet(self, mesh, packet, packet_bytes, packet_status)
    }

    fn text_message<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        message: &'f TextMessageData<'_>,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::text_message(self, mesh, packet, packet_status, contact, message)
    }

    fn group_text<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        channel: &'f Channel,
        message: &'f TextMessageData<'_>,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::group_text(self, mesh, packet, packet_status, channel, message)
    }

    fn ack<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        ack: &'f Ack,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::ack(self, mesh, packet, packet_status, ack)
    }

    fn advert<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        advert: &'f Advert<'_>,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::advert(self, mesh, packet, packet_status, advert)
    }

    fn returned_path<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        path: &'f ReturnedPath<'_>,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::returned_path(self, mesh, packet, packet_status, contact, path)
    }

    fn response<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        response: &'f [u8],
    ) -> impl Future<Output = CompanionResult<()>> {
        A::response(self, mesh, packet, packet_status, contact, response)
    }

    fn request<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        request: &'f RequestPayload<'_>,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::request(self, mesh, packet, packet_status, contact, request)
    }

    fn anonymous_request<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f ForeignIdentity,
        data: &'f [u8],
    ) -> impl Future<Output = CompanionResult<()>> {
        A::anonymous_request(self, mesh, packet, packet_status, contact, data)
    }

    fn trace_packet<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        snrs: &'f [i8],
        trace: &'f TracePacket<'_>,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::trace_packet(self, mesh, packet, packet_status, snrs, trace)
    }

    fn control_packet<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        payload: &'f ControlPayload,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::control_packet(self, mesh, packet, packet_status, payload)
    }
}

pub trait WithLayer {
    fn call_me(&self, layer: impl SimpleMeshLayer) -> impl Future<Output = ()>;
}

pub trait MeshLayerGet: Send {
    type Layer<'a>: SimpleMeshLayer + Send;

    fn with_layer(&self, f: impl WithLayer) -> impl Future<Output = ()>;
}

impl<L: SimpleMeshLayer + Send + 'static> MeshLayerGet for Arc<RwLock<esp_sync::RawMutex, L>> {
    type Layer<'a> = &'a mut L;

    fn with_layer(&self, f: impl WithLayer) -> impl Future<Output = ()> {
        self.write()
            .then(|mut v| async move { f.call_me(&mut *v).await })
    }
}

impl<L: SimpleMeshLayer + Send + 'static> MeshLayerGet for Arc<EspMutex<L>> {
    type Layer<'a> = &'a mut L;

    fn with_layer(&self, f: impl WithLayer) -> impl Future<Output = ()> {
        self.lock()
            .then(|mut v| async move { f.call_me(&mut *v).await })
    }
}

macro_rules! impl_mesh_layer_tuple {
    ($join:path; $($var:ident),*) => {
        #[allow(non_snake_case)]
        impl <$($var),*> MeshLayerGet for ($(Arc<RwLock<esp_sync::RawMutex, $var>>),*) where $($var: SimpleMeshLayer + Send + 'static),* {
            type Layer<'a> = ($(&'a mut $var),*);

                async fn with_layer(&self, f: impl WithLayer) {
                    let ($($var),*) = self;
                    let ($(mut $var),*) = $join(
                        $(
                           $var.write()
                        ),*
                    ).await;

                    f.call_me(($($var.deref_mut()),*)).await;
                }
        }

        #[allow(non_snake_case)]
        impl <$($var),*> MeshLayerGet for ($(Arc<EspMutex<$var>>),*) where $($var: SimpleMeshLayer + Send + 'static),* {
            type Layer<'a> = ($(&'a mut $var),*);

                async fn with_layer(&self, f: impl WithLayer) {
                    let ($($var),*) = self;
                    let ($(mut $var),*) = $join(
                        $(
                           $var.lock()
                        ),*
                    ).await;

                    f.call_me(($($var.deref_mut()),*)).await;
                }
        }

        #[allow(non_snake_case)]
        impl <$($var),*> SimpleMeshLayer for ($(&mut $var),*) where $($var: SimpleMeshLayer),* {
                async fn packet<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_bytes: &'f [u8],
                    packet_status: PacketStatus,
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.packet(mesh, packet, packet_bytes, packet_status)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn text_message<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    contact: &'f CachedContact,
                    message: &'f TextMessageData<'_>,
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.text_message(mesh, packet, packet_status, contact, message)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn group_text<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    channel: &'f Channel,
                    message: &'f TextMessageData<'_>,
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.group_text(mesh, packet, packet_status, channel, message)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn ack<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    ack: &'f Ack,
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.ack(mesh, packet, packet_status, ack)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn advert<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    advert: &'f Advert<'_>,
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.advert(mesh, packet, packet_status, advert)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn returned_path<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    contact: &'f CachedContact,
                    path: &'f ReturnedPath<'_>,
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.returned_path(mesh, packet, packet_status, contact, path)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn response<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    contact: &'f CachedContact,
                    response_bytes: &'f [u8]
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.response(mesh, packet, packet_status, contact, response_bytes)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn request<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    contact: &'f CachedContact,
                    request: &'f RequestPayload<'_>
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.request(mesh, packet, packet_status, contact, request)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn anonymous_request<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    contact: &'f ForeignIdentity,
                    request: &'f [u8]
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.anonymous_request(mesh, packet, packet_status, contact, request)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }


                async fn trace_packet<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    snrs: &'f [i8],
                    trace: &'f TracePacket<'_>
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.trace_packet(mesh, packet, packet_status, snrs, trace)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn control_packet<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    payload: &'f ControlPayload
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.control_packet(mesh, packet, packet_status, payload)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }
        }
    };
}

impl_mesh_layer_tuple!(
    embassy_futures::join::join;
    A,B
);
impl_mesh_layer_tuple!(
    embassy_futures::join::join3;
    A,B,C
);
impl_mesh_layer_tuple!(
    embassy_futures::join::join4;
    A,B,C,D
);
impl_mesh_layer_tuple!(
    embassy_futures::join::join5;
    A,B,C,D,F
);

// async fn test_layer<A: SimpleMeshLayer, B: SimpleMeshLayer>(
//     (A, B): &(Arc<RwLock<esp_sync::RawMutex, A>>, Arc<RwLock<esp_sync::RawMutex, B>>),
//     with_fn: impl AsyncFnOnce((&mut A, &mut B))
// ) {
//     let (mut A, mut B) = embassy_futures::join::join(A.write(), B.write()).await;
//     with_fn((A.deref_mut(), B.deref_mut())).await;

//     todo!()
// }

// #[allow(non_snake_case)]
// impl<A, B> MeshLayerGet
//     for (
//         Arc<RwLock<esp_sync::RawMutex, A>>,
//         Arc<RwLock<esp_sync::RawMutex, B>>,
//     )
// where
//     A: SimpleMeshLayer + Send + 'static,
//     B: SimpleMeshLayer + Send + 'static,
// {
//     type Layer<'a> = (&'a mut A, &'a mut B);
//     async fn with_layer(&self, f: impl AsyncFnOnce(Self::Layer<'_>)) {
//         let (A, B) = self;
//         let (mut A, mut B) = embassy_futures::join::join(A.write(), B.write()).await;
//         f((A.deref_mut(), B.deref_mut())).await;
//     }
// }
