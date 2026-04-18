use alloc::{
    borrow::{Cow, ToOwned},
    string::String,
};
use chiyo_hal::esp_hal::rng::Trng;
use ed25519_compact::Noise;
use meshcore::{
    Packet, PacketHeader, Path, PayloadType, RouteType, SerDeser,
    crypto::ChannelKeys,
    payloads::{
        Advert, AdvertisementExtraData, AppdataFlags, RepeaterLogin, RequestPayload, TextHeader,
        TextMessageData, TextType, TracePacket,
    },
};
use smol_str::ToSmolStr;

use crate::{
    companion_protocol::protocol::{
        CompanionHandler, CompanionSink, NullPaddedSlice, NullPaddedString,
        responses::{
            self, ChannelInfo, CompanionProtoResult, DeviceInfo, GetMessageRes, MsgSent, SelfInfo,
        },
    },
    companionv2::Companion,
};
use chiyocore::{
    lora::LORA_FREQUENCY_IN_HZ,
    meshcore,
    simple_mesh::{
        storage::packet_log::SavedMessage,
        storage::{channel::Channel, contact::Contact},
    },
};

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
            device_name: &self.name,
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

    async fn channel_info(&mut self, idx: u8) -> CompanionProtoResult<responses::ChannelInfo> {
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
            .await?;
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

        let msg = heapless::format!(152; "{}: {}", self.name, txt).unwrap();

        let text = TextMessageData::plaintext(timestamp, msg.as_bytes());
        let timeout = self
            .mesh
            .read()
            .await
            .send_channel_message(&channel.as_keys(), &text, None)
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
            .send_direct_message(&contact.as_identity(), contact.path.clone(), text, None)
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
            name: Some(self.name.as_bytes().into()),
        };

        let random_bytes = rand::Rng::random(&mut Trng::try_new().unwrap());
        let mesh = self.mesh.read().await;
        let advert = mesh.identity.make_advert(
            (self.rtc.current_time_us() / 1_000_000) as u32,
            appdata,
            random_bytes,
        );

        if flood {
            mesh.send_flood_packet::<Advert>(&advert, None).await?;
        } else {
            mesh.send_direct_packet::<Advert>(&advert, Path::empty(mesh.path_hash_mode), None)
                .await?;
        };

        Ok(responses::Ok { code: None })
    }

    async fn set_advert_name(&mut self, name: &str) -> CompanionProtoResult<responses::Ok> {
        self.name = name.to_owned();
        self.mesh
            .write()
            .await
            .advert_data
            .with_mut(|v| v.name = Some(Cow::Owned(name.as_bytes().to_owned())))
            .await;
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
        contacts.insert(contact).await?;

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
        Ok(match self.message_log.lock().await.pop().await {
            Some(SavedMessage::Channel(v)) => GetMessageRes::Channel(v.clone_with_data()),
            Some(SavedMessage::Contact(v)) => GetMessageRes::Contact(v.clone_with_data()),
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

        let msg_timeout = self
            .mesh
            .read()
            .await
            .send_anon_req::<RepeaterLogin>(&contact.as_identity(), contact.path.clone(), &login)
            .await?;

        Ok(responses::MsgSent {
            is_flood: contact.path.is_none(),
            expected_ack: login.timestamp.to_le_bytes(),
            suggested_timeout: msg_timeout.as_millis() as u32,
        })
    }

    async fn get_contacts(
        &mut self,
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
        Ok(responses::CustomVars(
            self.global_config
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect(),
        ))
    }

    async fn set_custom_var(
        &mut self,
        key: &str,
        val: &str,
    ) -> CompanionProtoResult<responses::Ok> {
        self.global_config
            .with_mut(|cfg| {
                cfg.insert(key.into(), val.into());
            })
            .await?;
        Ok(responses::Ok { code: None })
    }

    async fn get_core_stats(&mut self) -> CompanionProtoResult<responses::CoreStats> {
        Ok(responses::CoreStats {
            battery_mv: 0,
            uptime_secs: self.rtc.time_since_power_up().as_secs() as u32,
            errors: 0,
            queue_len: 0,
        })
    }

    async fn get_radio_stats(&mut self) -> CompanionProtoResult<responses::RadioStats> {
        Ok(responses::RadioStats {
            noise_floor: 0,
            last_rssi: 0,
            last_snr: 0,
            tx_air_secs: 0,
            rx_air_secs: 0,
        })
    }

    async fn get_packet_stats(&mut self) -> CompanionProtoResult<responses::PacketStats> {
        Ok(responses::PacketStats {
            recv: 0,
            sent: 0,
            flood_tx: 0,
            direct_tx: 0,
            flood_rx: 0,
            direct_rx: 0,
            recv_errors: 0,
        })
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

        let _timeout = mesh.send_packet(&packet, true, None).await?;
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

        self.mesh
            .read()
            .await
            .send_packet(&packet, true, None)
            .await?;

        Ok(MsgSent {
            is_flood: false,
            expected_ack: tag,
            suggested_timeout: timeout.as_millis() as u32,
        })
    }

    async fn send_binary_req(
        &mut self,
        pub_key: &[u8; 32],
        data: &[u8],
    ) -> CompanionProtoResult<responses::MsgSent> {
        let contacts = self.storage.contacts.read().await;
        let contact = contacts
            .fast_get(pub_key)
            .ok_or(responses::Err { code: None })?;

        let time = (self.rtc.current_time_us() / 1_000_000) as u32;
        let timeout = self
            .mesh
            .read()
            .await
            .send_to_contact::<RequestPayload>(
                &contact.as_identity(),
                contact.path.clone(),
                &RequestPayload {
                    time,
                    data: data.into(),
                },
                None,
            )
            .await?;

        Ok(responses::MsgSent {
            is_flood: contact.path.is_some(),
            expected_ack: time.to_le_bytes(),
            suggested_timeout: timeout.as_millis() as u32,
        })
    }

    async fn send_anon_req(
        &mut self,
        pub_key: &[u8; 32],
        data: &[u8],
    ) -> CompanionProtoResult<responses::MsgSent> {
        let contacts = self.storage.contacts.read().await;
        let contact = contacts
            .fast_get(pub_key)
            .ok_or(responses::Err { code: None })?;

        let req = RequestPayload {
            time: (self.rtc.current_time_us() / 1_000_000) as u32,
            data: data.into(),
        };

        let msg_timeout = self
            .mesh
            .read()
            .await
            .send_anon_req::<RequestPayload>(&contact.as_identity(), contact.path.clone(), &req)
            .await?;
        Ok(responses::MsgSent {
            is_flood: contact.path.is_none(),
            expected_ack: req.time.to_le_bytes(),
            suggested_timeout: msg_timeout.as_millis() as u32,
        })
    }

    async fn import_contact(&mut self, data: &[u8]) -> CompanionProtoResult<responses::Ok> {
        let payload = Advert::decode(data).map_err(|_| responses::Err { code: None })?;
        let Some(appdata) = payload.appdata.as_ref() else {
            return Err(responses::Err { code: None });
        };

        let Some(name) = appdata
            .name
            .as_ref()
            .and_then(|v| core::str::from_utf8(v).ok())
        else {
            return Err(responses::Err { code: None });
        };

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

        Ok(responses::Ok { code: None })
    }
}
