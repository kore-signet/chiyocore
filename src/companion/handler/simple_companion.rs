use core::cell::RefCell;

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::sync::Arc;
use arrayref::array_ref;
use bumpalo::Bump;
use bumpalo::collections::Vec as BumpaloVec;
use ed25519_compact::{KeyPair, Seed, SigningState, x25519};
use embassy_executor::SendSpawner;
use esp_hal::rtc_cntl::Rtc;
use lora_phy::mod_params::PacketStatus;
use meshcore::crypto::{ContainsEncryptable, DecryptedView, Encryptable, VerifiablePayload};
use meshcore::identity::LocalIdentity;
use meshcore::io::ByteVecImpl;
use meshcore::payloads::{
    Ack, Advert, EncryptedMessageWithDst, GroupText, ReturnedPath, TextHeader, TextMessageData,
    TextType,
};
use meshcore::{DecodeError, Packet, Path, PathHashMode, PayloadType, RouteType, SerDeser};

use crate::companion::handler::BotLayer;
use crate::companion::handler::message_log::{
    HashableChannelMessage, HashableContactMessage, MESSAGE_LOG_SIZE, MessageLog, SavedMessage,
};
use crate::companion::handler::storage::{
    CachedContact, Channel, ChannelManager, CompanionConfig, Contact, StoredIdentity,
};
use crate::companion::protocol::responses::{MsgSent, RfLogData};
use crate::companion::protocol::{CompanionSink, responses};
use crate::companion::tcp::TcpCompanionSink;
use crate::crypto::{HardwareAES, HardwareHMAC, HardwareSHA};
use crate::lora::LoraTaskChannel;
use crate::storage::{
    ActiveFilesystem, CachedFileDb, CachedVersion, FS_SIZE, FsPartition, SimpleFileDb,
};
use crate::{EspMutex, FirmwareError, MeshcoreHandler};

#[embassy_executor::task(pool_size = 16)]
async fn send_packet_after(
    packet: meshcore::Packet<'static>,
    lora: thingbuf::mpsc::StaticSender<heapless::Vec<u8, 256>>,
    delay: embassy_time::Duration,
) {
    embassy_time::Timer::after(delay).await;
    let mut tx_slot = lora.send_ref().await.unwrap();
    Packet::encode_into_vec(&packet, &mut *tx_slot).unwrap();
    drop(tx_slot);
}

#[derive(Debug)]
pub enum CompanionError {
    NoKnownChannel,
    NoKnownContact,
    DecryptFailure,
    VerifyFailure,
    AesFailure(esp_hal::aes::Error),
    DecodeFailure(DecodeError),
    Firmware(FirmwareError),
}

impl From<FirmwareError> for CompanionError {
    fn from(value: FirmwareError) -> Self {
        Self::Firmware(value)
    }
}

impl From<DecodeError> for CompanionError {
    fn from(value: DecodeError) -> Self {
        Self::DecodeFailure(value)
    }
}

impl From<esp_hal::aes::Error> for CompanionError {
    fn from(value: esp_hal::aes::Error) -> Self {
        Self::AesFailure(value)
    }
}

pub type CompanionResult<T> = Result<T, CompanionError>;

/// takes raw meshcore packets and decodes them according to stored contacts & channels
/// there's a lot of refcells here in a not good way. todo: fix that - will probably require rearranging/reorg'ing to more clearly separate what is mutable and not mutable during a packet rx
pub struct SimpleCompanion<B: BotLayer + Send> {
    pub identity: StoredIdentity,
    pub channels: ChannelManager,
    pub config: CompanionConfig,
    pub contacts: CachedFileDb<FS_SIZE, Contact>,
    pub config_db: SimpleFileDb<FS_SIZE>,
    pub log: RefCell<MessageLog>,
    pub lora_tx: LoraTaskChannel,
    pub companion_tx: RefCell<TcpCompanionSink>,
    pub rtc: Rtc<'static>,
    pub scratch: bumpalo::Bump,
    pub(crate) signature_in_progress: Option<SigningState>,
    pub bot: RefCell<B>,
}

impl<B: BotLayer + Send> SimpleCompanion<B> {
    pub async fn load(
        fs: &Arc<EspMutex<ActiveFilesystem<FS_SIZE>>>,
        log_fs: FsPartition<MESSAGE_LOG_SIZE>,
        lora_tx: LoraTaskChannel,
        rtc: Rtc<'static>,
        tcp_tx: TcpCompanionSink,
        bot: B,
    ) -> SimpleCompanion<B> {
        static FS_CONTACTS_PREFIX: &littlefs2::path::Path = littlefs2::path!("/contacts/");

        let mut config_db = SimpleFileDb::new(Arc::clone(fs));
        config_db.make_dir(littlefs2::path!("/companion/")).await;
        let identity = if let Some(identity) = config_db
            .get::<StoredIdentity>(littlefs2::path!("/companion/identity"))
            .await
            .unwrap()
        {
            identity
        } else {
            let mut seed = [0u8; 32];
            let mut rng = esp_hal::rng::Trng::try_new().unwrap();
            loop {
                rand::Rng::fill(&mut rng, &mut seed);
                let pair = KeyPair::from_seed(Seed::new(seed));
                if let Ok(x25519_pair) = x25519::KeyPair::from_ed25519(&pair) {
                    let name = hex::encode(&pair.pk[..8]);
                    let id = StoredIdentity {
                        keys: LocalIdentity {
                            signing_keys: pair,
                            encryption_keys: x25519_pair,
                        },
                        name,
                    };
                    config_db
                        .insert(littlefs2::path!("/companion/identity"), &id)
                        .await
                        .unwrap();
                    break id;
                }
            }
        };

        let config = config_db
            .get(littlefs2::path!("/companion/config"))
            .await
            .unwrap()
            .unwrap_or_default();

        let bump = Bump::with_capacity(1024);
        bump.set_allocation_limit(Some(2048));

        SimpleCompanion {
            identity,
            config_db,
            config,
            channels: ChannelManager::new(Arc::clone(fs)).await,
            contacts: CachedFileDb::init(Arc::clone(fs), FS_CONTACTS_PREFIX).await,
            lora_tx,
            companion_tx: RefCell::new(tcp_tx),
            rtc,
            // storage: SimpleFileDb::new(Arc::clone(fs)),
            scratch: bump,
            log: RefCell::new(MessageLog::new(log_fs)),
            signature_in_progress: None,
            bot: RefCell::new(bot),
        }
    }

    pub fn self_hash(&self) -> u8 {
        self.identity.pubkey()[0]
    }

    pub async fn send_to_channel(
        &self,
        scratch_alloc: &bumpalo::Bump,
        channel_idx: u8,
        msg: &str,
        timestamp: Option<u32>,
    ) -> CompanionResult<MsgSent> {
        let channel = self
            .channels
            .db
            .get_cached(&[channel_idx])
            .ok_or(CompanionError::NoKnownChannel)?;
        let text_data = TextMessageData::plaintext(
            timestamp.unwrap_or_else(|| (self.rtc.current_time_us() / 1_000_000) as u32),
            msg.as_bytes(),
        );

        self.log
            .borrow_mut()
            .make_seen(&HashableChannelMessage {
                idx: channel_idx,
                timestamp: text_data.timestamp,
                data: msg.as_bytes(),
            })
            .await;

        let mut scratch = BumpaloVec::new_in(scratch_alloc);
        TextMessageData::encrypt::<HardwareAES>(&text_data, &channel.key, &mut scratch).await?;

        let group_text = GroupText::new(channel.hash, &*scratch, &channel.key);

        let timeout = self
            .lora_tx
            .send_flood::<GroupText<'_>>(&group_text, Path::empty(meshcore::PathHashMode::OneByte))
            .await;

        Ok(MsgSent {
            is_flood: true,
            expected_ack: [0; 4],
            suggested_timeout: timeout.as_secs() as u32,
        })
    }

    pub async fn send_direct_message(
        &self,
        scratch_alloc: &bumpalo::Bump,
        destination: &[u8],
        msg: &str,
        attempt: u8,
        text_type: TextType,
        timestamp: Option<u32>,
    ) -> CompanionResult<MsgSent> {
        let contact = self
            .contacts
            .get_cached(destination)
            .ok_or(CompanionError::NoKnownContact)?;

        let text_data = TextMessageData {
            timestamp: timestamp.unwrap_or_else(|| (self.rtc.current_time_us() / 1_000_000) as u32),
            header: TextHeader::new()
                .with_attempt(attempt)
                .with_text_type(text_type),
            message: msg.as_bytes().into(),
        };

        self.log
            .borrow_mut()
            .make_seen(&HashableContactMessage {
                pk_prefix: array_ref![self.identity.pubkey(), 0, 6],
                timestamp: text_data.timestamp,
                data: msg.as_bytes(),
            })
            .await;

        let ack = Ack::calculate::<HardwareSHA>(&text_data, &self.identity.as_foreign()).await;

        let mut encrypt_scratch = BumpaloVec::new_in(scratch_alloc);
        let mut encode_vec = BumpaloVec::new_in(scratch_alloc);

        EncryptedMessageWithDst::encode_into_vec(
            &self
                .identity
                .make_message::<TextMessageData, HardwareAES>(
                    &text_data,
                    contact,
                    &mut encrypt_scratch,
                )
                .await?,
            &mut encode_vec,
        )
        .unwrap();

        let packet = if let Some(path) = contact.path_to.as_ref() {
            Packet::direct::<EncryptedMessageWithDst<TextMessageData>>(path.clone(), &*encode_vec)
        } else {
            Packet::flood::<EncryptedMessageWithDst<TextMessageData>>(
                Path::empty(PathHashMode::OneByte),
                &*encode_vec,
            )
        };

        let timeout = self.lora_tx.send_packet(&packet).await;

        Ok(MsgSent {
            is_flood: packet.header.route_type().is_flood(),
            expected_ack: ack.crc,
            suggested_timeout: timeout.as_secs() as u32,
        })
    }
}

/*  decoding */
impl<B: BotLayer + Send> SimpleCompanion<B> {
    async fn decode_text<'s>(
        &self,
        payload: &EncryptedMessageWithDst<'_, TextMessageData<'static>>,
        scratch: &'s mut impl ByteVecImpl,
    ) -> CompanionResult<Option<(&CachedContact, DecryptedView<'s, TextMessageData<'static>>)>>
    {
        log::trace!(
            "\ttext msg | {src:x}->{dst:x}",
            src = payload.source_hash,
            dst = payload.destination_hash
        );
        if self.self_hash() != payload.destination_hash {
            return Ok(None);
        }

        // todo: we might want to iterate through all keys starting with prefix
        let Some(other_ident) = self.contacts.get_cached(&[payload.source_hash]) else {
            log::error!("\tmessage is destined to us, but no matching key for src found!");
            return Err(CompanionError::NoKnownContact);
        };

        let shared_secret = self.identity.shared_secret(other_ident);
        let Ok(txt_msg) = payload
            .decrypt::<HardwareAES>(array_ref![&shared_secret, 0, 16], scratch)
            .await
        else {
            log::error!("\tmessage failed to decrypt");
            return Err(CompanionError::DecryptFailure);
        };

        // let verify = payload.verify::<C>(array_ref![&shared_secret, 0, 16]);
        // log::trace!("\tverify: {verify}");
        // if !verify {
        //     log::error!("\tmac check failed, returning");
        //     return Err(DispatcherError::VerifyFailure);
        // }

        Ok(Some((other_ident, txt_msg)))
    }

    async fn decode_channel_message<'s>(
        &self,
        payload: &GroupText<'_>,
        scratch: &'s mut impl ByteVecImpl,
    ) -> CompanionResult<Option<(&Channel, DecryptedView<'s, TextMessageData<'static>>)>> {
        log::trace!("\tgroup msg | channel: {:x}", payload.channel);
        let Some(channel) = self.channels.by_hash(payload.channel) else {
            log::error!("\t(no keys for channel)");
            return Err(CompanionError::NoKnownChannel);
        };
        // let channel_name = self.channel_keys.name_by_hash(payload.channel).unwrap();

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

    async fn decode_returned_path<'s>(
        &self,
        payload: &EncryptedMessageWithDst<'_, ReturnedPath<'static>>,
        scratch: &'s mut impl ByteVecImpl,
    ) -> CompanionResult<Option<(&CachedContact, DecryptedView<'s, ReturnedPath<'static>>)>> {
        // todo i think this should have a mac check?
        log::trace!(
            "\treturned path msg | {src:x}->{dst:x}",
            src = payload.source_hash,
            dst = payload.destination_hash
        );
        if self.self_hash() != payload.destination_hash {
            return Ok(None);
        }

        let Some(other_ident) = self.contacts.get_cached(&[payload.source_hash]) else {
            log::error!("\tmessage is destined to us, but no matching key for src found!");
            return Err(CompanionError::NoKnownContact);
        };

        let shared_secret = self.identity.shared_secret(other_ident);
        let Ok(returned_path_dec) = payload
            .decrypt::<HardwareAES>(array_ref![&shared_secret, 0, 16], scratch)
            .await
        else {
            log::error!("\tmessage failed to decrypt");
            return Err(CompanionError::DecryptFailure);
        };

        // let verify = payload.verify::<HardwareHMAC>(array_ref![&shared_secret, 16, 16]);
        // log::trace!("\tverify: {verify}");
        // if !verify {
        // log::error!("\tmac check failed, returning");
        // return Err(DispatcherError::VerifyFailure);
        // }

        // if !verify {
        //     log::error!("\tmac check failed, returning");
        //     return Err(DispatcherError::VerifyFailure);
        // }

        Ok(Some((other_ident, returned_path_dec)))
    }

    async fn send_ack(
        &self,
        packet: &Packet<'_>,
        txt_decoded: &TextMessageData<'_>,
        contact: &CachedContact,
        scratch: &mut impl ByteVecImpl,
    ) {
        let ack = Ack::calculate::<HardwareSHA>(txt_decoded, contact).await;

        let ack_packet: Packet<'static> = match packet.header.route_type() {
            RouteType::TransportFlood | RouteType::Flood => {
                let returned_path = ReturnedPath {
                    path: packet.path.clone(),
                    extra: Some((PayloadType::Ack, Cow::Borrowed(&ack.crc))),
                };

                let encrypted = self
                    .identity
                    .make_message::<ReturnedPath, HardwareAES>(&returned_path, contact, scratch)
                    .await
                    .unwrap();

                let payload_buf =
                    <EncryptedMessageWithDst<ReturnedPath>>::encode_to_vec(&encrypted).unwrap();

                Packet::<'static>::flood::<EncryptedMessageWithDst<ReturnedPath>>(
                    Path::empty(PathHashMode::OneByte),
                    payload_buf,
                )
            }
            RouteType::Direct | RouteType::TransportDirect => {
                let ack_bytes = Ack::encode_to_vec(&ack).unwrap();
                let path = packet.path.to_owned();
                Packet::<'static>::direct::<Ack>(path, ack_bytes)
            }
        };

        let packet_timeout = ack_packet.timeout_est(&packet.path, packet.header.route_type());

        SendSpawner::for_current_executor()
            .await
            .spawn(send_packet_after(
                ack_packet,
                self.lora_tx.tx.clone(),
                embassy_time::Duration::try_from(packet_timeout).unwrap(),
            ))
            .unwrap();
    }
}

impl<B: BotLayer + Send> MeshcoreHandler for SimpleCompanion<B> {
    type Error = CompanionError;

    async fn packet(
        &mut self,
        packet: &meshcore::Packet<'_>,
        packet_status: PacketStatus,
        bytes: &[u8],
    ) -> CompanionResult<()> {
        let mut scratch = core::mem::take(&mut self.scratch);
        log::info!(
            "packet ({payload_ty:?}) | {transport_ty:?} : {path:?}",
            payload_ty = packet.header.payload_type(),
            transport_ty = packet.header.route_type(),
            path = packet.path
        );

        self.companion_tx
            .borrow_mut()
            .write_packet(&RfLogData::new(packet_status, bytes))
            .await;

        let _: () = match packet.header.payload_type() {
            PayloadType::Request => {}
            PayloadType::Response => {}
            PayloadType::TxtMsg => {
                self.text_message(
                    (packet, packet_status),
                    &packet
                        .decode_payload_as::<EncryptedMessageWithDst<'_, TextMessageData<'static>>>(
                        )?,
                    &scratch,
                )
                .await?;
            }
            PayloadType::Ack => {
                self.ack(
                    (packet, packet_status),
                    packet.decode_payload_as::<Ack>()?,
                    &scratch,
                )
                .await?;
            }
            PayloadType::Advert => {
                self.advert(
                    (packet, packet_status),
                    &packet.decode_payload_as::<Advert<'_>>()?,
                    &scratch,
                )
                .await?;
            }
            PayloadType::GrpTxt => {
                self.channel_message(
                    (packet, packet_status),
                    &packet.decode_payload_as::<GroupText<'_>>()?,
                    &scratch,
                )
                .await?;
            }
            PayloadType::AnonReq => {}
            PayloadType::Path => {
                self.returned_path(
                    (packet, packet_status),
                    &packet
                        .decode_payload_as::<EncryptedMessageWithDst<'_, ReturnedPath<'static>>>(
                        )?,
                    &scratch,
                )
                .await?;
            }
            PayloadType::Trace => {}
            PayloadType::Multipart => {}
            PayloadType::Control => {}
            PayloadType::RawCustom => {}
        };

        scratch.reset();
        self.scratch = scratch;

        Ok(())
    }
}

impl<B: BotLayer + Send> SimpleCompanion<B> {
    async fn advert(
        &mut self,
        (packet, _packet_status): (&meshcore::Packet<'_>, PacketStatus),
        payload: &Advert<'_>,
        _scratch: &Bump,
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

        // if !self.contacts.contains(&payload.public_key) {
        self.contacts
            .insert(&Contact {
                key: payload.public_key,
                name: String::from(name),
                path_to: Some(packet.path.to_owned()),
                flags: appdata.flags.bits(),
                latitude: appdata.latitude.unwrap_or(0),
                longitude: appdata.longitude.unwrap_or(0),
                last_heard: (self.rtc.current_time_us() / 1_000_000) as u32,
            })
            .await?;
        // }

        Ok(())
    }

    async fn ack(
        &mut self,
        _packet: (&meshcore::Packet<'_>, PacketStatus),
        ack: Ack,
        _scratch: &Bump,
    ) -> CompanionResult<()> {
        self.companion_tx
            .borrow_mut()
            .write_packet(&responses::Ack { code: ack.crc })
            .await;
        Ok(())
    }

    async fn text_message(
        &mut self,
        (packet, _packet_status): (&meshcore::Packet<'_>, PacketStatus),
        text: &EncryptedMessageWithDst<'_, TextMessageData<'static>>,
        scratch: &Bump,
    ) -> CompanionResult<()> {
        let mut decrypt_scratch = BumpaloVec::new_in(scratch);
        let Some((contact, text)) = self.decode_text(text, &mut decrypt_scratch).await? else {
            return Ok(());
        };

        let contact_full = self.contacts.get_full(contact.key()).await?.unwrap();

        let text = text.decoded()?;

        // todo: should this be after message_is_new?
        self.send_ack(packet, &text, contact, &mut BumpaloVec::new_in(scratch))
            .await;

        let saved = SavedMessage::contact_msg(contact, packet, &text);
        let message_is_new = self.log.borrow_mut().push(&saved).await;

        if !message_is_new {
            return Ok(());
        }

        self.companion_tx.borrow_mut().write_packet(&saved).await;

        log::info!(
            "\t[DM] {contact_name} > {text_msg}",
            contact_name = contact_full.name,
            text_msg = text.as_utf8()? // text_msg = text.decoded()?.as_utf8()?
        );

        Ok(())
    }

    async fn channel_message(
        &mut self,
        (packet, packet_status): (&meshcore::Packet<'_>, PacketStatus),
        text: &GroupText<'_>,
        scratch: &Bump,
    ) -> CompanionResult<()> {
        let mut decrypt_scratch = BumpaloVec::new_in(scratch);
        let Some((channel, text)) = self
            .decode_channel_message(text, &mut decrypt_scratch)
            .await?
        else {
            return Ok(());
        };

        let text = text.decoded()?;

        let saved = SavedMessage::channel_msg(channel, packet, &text);
        let message_is_new = self.log.borrow_mut().push(&saved).await;
        if !message_is_new {
            return Ok(());
        }

        self.bot
            .borrow_mut()
            .channel_message(scratch, channel, (packet, packet_status), &text, self)
            .await?;
        self.companion_tx.borrow_mut().write_packet(&saved).await;

        log::info!(
            "\t{channel_name} > {text_msg}",
            channel_name = channel.name,
            text_msg = text.as_utf8()?
        );

        Ok(())
    }

    async fn returned_path(
        &mut self,
        packet: (&meshcore::Packet<'_>, PacketStatus),
        payload: &EncryptedMessageWithDst<'_, ReturnedPath<'static>>,
        scratch: &Bump,
    ) -> CompanionResult<()> {
        let mut decrypt_scratch = BumpaloVec::new_in(scratch);
        let Some((contact_cached, ret_path_bytes)) = self
            .decode_returned_path(payload, &mut decrypt_scratch)
            .await?
        else {
            return Ok(());
        };

        let mut contact = self
            .contacts
            .get_full(contact_cached.key())
            .await?
            .ok_or(CompanionError::NoKnownContact)?;

        let ret_path = ret_path_bytes.decoded()?;
        // todo: support other paths
        if let Ok(ack) = ret_path.decode_payload_as::<Ack>() {
            // let ack = ret_path.decode_payload_as::<Ack>()?;
            self.ack(packet, ack, scratch).await?;
        }

        contact.path_to = Some(ret_path.path.to_owned());
        self.contacts.insert(&contact).await?;

        Ok(())
    }
}
