use core::time::Duration;

use alloc::{borrow::Cow, string::String};
use arrayref::array_ref;
use lora_phy::mod_params::PacketStatus;
use meshcore::{
    Packet, PacketHeader, PacketPayload, Path, PathHashMode, PayloadType, RouteType, SerDeser,
    crypto::{ChannelKeys, ContainsEncryptable, DecryptedView, Encryptable, VerifiablePayload},
    identity::{ForeignIdentity, LocalIdentity},
    io::ByteVecImpl,
    payloads::{Ack, Advert, EncryptedMessageWithDst, GroupText, ReturnedPath, TextMessageData},
};

use crate::{
    BumpaloVec, CompanionError, CompanionResult, MeshcoreHandler,
    companion_protocol::protocol::responses::MsgSent,
    crypto::{HardwareAES, HardwareHMAC, HardwareSHA},
    lora::LoraTaskChannel,
    simple_mesh::{
        packet_log::{HashLog, SavedMessage},
        storage::{
            MeshStorage,
            channel::Channel,
            contact::{CachedContact, Contact},
        },
    },
};

pub mod packet_log;
pub mod storage;

pub struct SimpleMesh {
    pub identity: LocalIdentity,
    pub packet_log: HashLog<32>,
    pub message_log: HashLog<32>,
    pub lora_tx: LoraTaskChannel,
    pub scratch: bumpalo::Bump,
    pub path_hash_mode: PathHashMode,
    pub storage: MeshStorage,
    // layers: SmallVec<[Arc<EspMutex<Box<dyn SimpleMeshLayer + Send>>>; 2]>, // don't love this triple allocation
}

impl SimpleMesh {
    pub fn new(
        identity: LocalIdentity,
        storage: MeshStorage,
        lora_tx: LoraTaskChannel,
    ) -> SimpleMesh {
        SimpleMesh {
            identity,
            packet_log: HashLog::new(),
            message_log: HashLog::new(),
            lora_tx,
            scratch: bumpalo::Bump::new(),
            path_hash_mode: PathHashMode::OneByte,
            storage,
        }
    }

    // pub fn add_layer(&mut self, layer: impl SimpleMeshLayer + Send + 'static) -> Arc<EspMutex<Box<dyn SimpleMeshLayer + Send>>> {
    //     let layer: Arc<embassy_sync::mutex::Mutex<esp_sync::RawMutex, Box<dyn SimpleMeshLayer + Send + 'static>>> = Arc::new(
    //         EspMutex::new(
    //             Box::new(
    //                 layer
    //             )
    //         )
    //     );

    //     self.layers.push(Arc::clone(&layer));
    //     layer
    // }
}

/* tx methods */
impl SimpleMesh {
    /// returns est. timeout
    pub async fn send_to_contact<P: SerDeser + Encryptable + PacketPayload>(
        &self,
        contact: &ForeignIdentity,
        path: Option<meshcore::Path<'_>>,
        message: &P::Representation<'_>,
    ) -> CompanionResult<Duration> {
        let mut encrypt_scratch = BumpaloVec::new_in(&self.scratch);
        let mut encode_vec = BumpaloVec::new_in(&self.scratch);

        EncryptedMessageWithDst::encode_into_vec(
            &self
                .identity
                .make_message::<P, HardwareAES>(message, contact, &mut encrypt_scratch)
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

        self.packet_log.push(&packet).await;

        let timeout = self.lora_tx.send_packet(&packet).await;

        Ok(timeout)
    }

    pub async fn send_direct_message(
        &self,
        contact: &ForeignIdentity,
        path: Option<meshcore::Path<'_>>, // if None, flood
        message: TextMessageData<'_>,
    ) -> CompanionResult<MsgSent> {
        let is_flood = path.is_none();
        let ack = Ack::calculate::<HardwareSHA>(&message, &self.identity.as_foreign()).await;
        let timeout = self
            .send_to_contact::<TextMessageData>(contact, path, &message)
            .await?;

        Ok(MsgSent {
            is_flood,
            expected_ack: ack.crc,
            suggested_timeout: timeout.as_secs() as u32,
        })
    }

    pub async fn send_channel_message(
        &self,
        channel: &ChannelKeys,
        text: &TextMessageData<'_>,
    ) -> CompanionResult<Duration> {
        let mut scratch = BumpaloVec::new_in(&self.scratch);
        TextMessageData::encrypt::<HardwareAES>(text, &channel.secret, &mut scratch).await?;

        let group_text = GroupText::new(channel.hash, &*scratch, &channel.secret);

        let timeout = self.send_flood_packet::<GroupText>(&group_text).await?;

        Ok(timeout)
    }

    pub async fn send_flood_packet<P: PacketPayload>(
        &self,
        payload: &P::Representation<'_>,
    ) -> CompanionResult<Duration> {
        let mut scratch = BumpaloVec::new_in(&self.scratch);
        P::encode_into_vec(payload, &mut scratch);

        let packet = Packet {
            header: PacketHeader::new()
                .with_payload_type(P::PAYLOAD_TYPE)
                .with_route_type(RouteType::Flood),
            transport_codes: None,
            path: Path::empty(self.path_hash_mode),
            payload: Cow::Borrowed(&scratch),
        };

        self.packet_log.push(&packet).await;

        let timeout = self.lora_tx.send_packet(&packet).await;

        Ok(timeout)
    }

    pub async fn send_direct_packet<P: PacketPayload>(
        &self,
        payload: &P::Representation<'_>,
        path: Path<'_>,
    ) -> CompanionResult<Duration> {
        let mut scratch = BumpaloVec::new_in(&self.scratch);
        P::encode_into_vec(payload, &mut scratch);

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

    pub async fn send_packet(&self, packet: &Packet<'_>, log: bool) -> CompanionResult<Duration> {
        if log {
            self.packet_log.push(packet).await;
        }

        let timeout = self.lora_tx.send_packet(packet).await;

        Ok(timeout)
    }

    async fn send_ack(
        &self,
        packet: &Packet<'_>,
        txt_decoded: &TextMessageData<'_>,
        contact: &CachedContact,
    ) {
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

        // todo: make ack late? i think it should be late for some reason
        self.send_packet(&ack_packet, false).await;

        // let packet_timeout = ack_packet.timeout_est(&packet.path, packet.header.route_type());

        // SendSpawner::for_current_executor()
        //     .await
        //     .spawn(send_packet_after(
        //         ack_packet,
        //         self.lora_tx.tx.clone(),
        //         embassy_time::Duration::try_from(packet_timeout).unwrap(),
        //     ))
        //     .unwrap();
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

        // todo: we might want to iterate through all keys starting with prefix
        let Some(other_ident) = self
            .storage
            .contacts
            .read()
            .await
            .fast_get(&[payload.source_hash])
            .cloned()
        else {
            log::error!("\tmessage is destined to us, but no matching key for src found!");
            return Err(CompanionError::NoKnownContact);
        };

        let shared_secret = self.identity.shared_secret(&other_ident.as_identity());
        let Ok(msg) = payload
            .decrypt::<HardwareAES>(array_ref![&shared_secret, 0, 16], scratch)
            .await
        else {
            log::error!("\tmessage failed to decrypt");
            return Err(CompanionError::DecryptFailure);
        };

        Ok(Some((other_ident, msg)))
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
        self.send_ack(packet, &text, &contact).await;

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
            .await;
        // }

        // for layer in self.layers.iter() {
        //     layer.lock().await
        //         .text_message(self, packet, packet_status, &contact, &text)
        //         .await;
        // }

        // self.layers.iter_mut().for_each(|v| );
        // log::info!(
        //     "\t[DM] {contact_name} > {text_msg}",
        //     contact_name = contact_full.name,
        //     text_msg = text.as_utf8()? // text_msg = text.decoded()?.as_utf8()?
        // );

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
            .await;
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
        // for layer in layers {
        layers.ack(self, packet, packet_status, &ack).await;
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
                path_to: Some(packet.path.to_owned()),
                flags: appdata.flags.bits(),
                latitude: appdata.latitude.unwrap_or(0),
                longitude: appdata.longitude.unwrap_or(0),
                last_heard: 0, // last_heard: (self.rtc.current_time_us() / 1_000_000) as u32,
            })
            .await?;

        // for layer in layers {
        layers.advert(self, packet, packet_status, &payload).await;
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

        // for layer in &mut layers[..] {
        layers
            .returned_path(self, packet, packet_status, &contact, &decoded)
            .await;
        // }

        if let Ok(ack) = decoded.decode_payload_as::<Ack>() {
            self.ack(packet, packet_status, ack, layers);
        }

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
        extra.packet(self, packet, bytes, packet_status).await;
        // }
        //
        match packet.header.payload_type() {
            PayloadType::Request => {}
            PayloadType::Response => {}
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
            PayloadType::AnonReq => {}
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
            PayloadType::Trace => {}
            PayloadType::Multipart => {}
            PayloadType::Control => {}
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

macro_rules! impl_mesh_layer_tuple {
    ($join:path; $($var:ident),*) => {
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
