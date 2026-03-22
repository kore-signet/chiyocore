use alloc::{string::ToString, vec::Vec};
use bumpalo::collections::Vec as BumpaloVec;
use ed25519_compact::Noise;
use meshcore::{
    Packet, Path, RouteType, SerDeser,
    identity::ForeignIdentity,
    payloads::{
        Advert, AdvertisementExtraData, AnonymousRequest, AppdataFlags, RepeaterLogin, TextType,
    },
};
use sha2::Digest;

use crate::{
    companion::{
        handler::{
            BotLayer,
            message_log::SavedMessage,
            simple_companion::SimpleCompanion,
            storage::{Channel, Contact},
        },
        protocol::{
            CompanionHandler, NullPaddedSlice,
            responses::{self, ChannelInfo, CompanionProtoResult, CustomVars, DeviceInfo},
        },
    },
    crypto::HardwareAES,
    storage::CachedVersion,
};

impl<B: BotLayer + Send> CompanionHandler for SimpleCompanion<B> {
    fn start_req(&mut self) {
        self.scratch.reset();
    }

    async fn app_start<'a>(
        &'a mut self,
        _app_ver: u8,
        _reserved: [u8; 6],
        _app_name: &str,
    ) -> CompanionProtoResult<responses::SelfInfo<'a>> {
        CompanionProtoResult::Ok(responses::SelfInfo {
            advertisement_type: 0,
            tx_power: 22,
            max_tx_power: 22,
            public_key: self.identity.pubkey(),
            lat: 0,
            long: 0,
            multi_acks: 0,
            adv_loc_policy: 0,
            telemetry_mode: 0,
            manual_add_contacts: false,
            radio_freq: 910_525_000,
            radio_bandwidth: 62_500,
            radio_sf: 7,
            radio_cr: 5,
            device_name: &self.identity.name,
        })
    }

    async fn device_query<'a>(
        &'a mut self,
        _app_ver: u8,
    ) -> CompanionProtoResult<responses::DeviceInfo<'a>> {
        CompanionProtoResult::Ok(DeviceInfo {
            fw_version: 10,
            max_contacts: 40,
            max_channels: 40,
            ble_pin: 4,
            firmware_build: NullPaddedSlice::from("nya"),
            model: NullPaddedSlice::from("nya"),
            version: NullPaddedSlice::from("nya"),
            client_repeat_enabled: false,
            path_hash_mode: 0,
        })
    }

    async fn channel_info<'a>(
        &'a mut self,
        idx: u8,
    ) -> CompanionProtoResult<responses::ChannelInfo<'a>> {
        if let Some(channel) = self.channels.db.get_cached(&[idx]) {
            CompanionProtoResult::Ok(ChannelInfo {
                idx,
                name: NullPaddedSlice::from(&*channel.name),
                secret: channel.key,
            })
        } else {
            CompanionProtoResult::Err(responses::Err { code: None })
        }
    }

    async fn set_channel(
        &mut self,
        idx: u8,
        name: &str,
        secret: &[u8; 16],
    ) -> CompanionProtoResult<responses::Ok> {
        let hash = sha2::Sha256::digest(&secret[..]);
        self.channels
            .register(&Channel {
                name: name.into(),
                key: *secret,
                hash: hash[0],
                idx,
            })
            .await?;
        CompanionProtoResult::Ok(responses::Ok { code: None })
    }

    async fn send_channel_message(
        &mut self,
        _text_type: TextType,
        idx: u8,
        timestamp: u32,
        txt: &str,
    ) -> CompanionProtoResult<responses::MsgSent> {
        let mut scratch = core::mem::take(&mut self.scratch);
        let message = bumpalo::format!(in &scratch, "{}: {}", self.identity.name, txt);
        let res = self
            .send_to_channel(&scratch, idx, &message, Some(timestamp))
            .await
            .map_err(|_| responses::Err { code: None })?;
        drop(message);
        scratch.reset();
        self.scratch = scratch;
        Ok(res)
    }

    async fn send_contact_message(
        &mut self,
        text_type: TextType,
        attempt: u8,
        timestamp: u32,
        destination: &[u8; 6],
        text: &str,
    ) -> CompanionProtoResult<responses::MsgSent> {
        let mut scratch = core::mem::take(&mut self.scratch);
        let res = self
            .send_direct_message(
                &scratch,
                destination,
                text,
                attempt,
                text_type,
                Some(timestamp),
            )
            .await?;
        scratch.reset();
        self.scratch = scratch;
        Ok(res)
    }

    async fn get_time(&mut self) -> CompanionProtoResult<responses::CurrentTime> {
        CompanionProtoResult::Ok(responses::CurrentTime {
            time: (self.rtc.current_time_us() / 1_000_000) as u32,
        })
    }

    async fn set_time(&mut self, time: u32) -> CompanionProtoResult<responses::Ok> {
        self.rtc.set_current_time_us(time as u64 * 1_000_000);
        CompanionProtoResult::Ok(responses::Ok { code: None })
    }

    async fn send_self_advert(&mut self, flood: bool) -> CompanionProtoResult<responses::Ok> {
        // let advert = self.identity.make_advert(timestamp, appdata, random_bytes)
        let mut rng = esp_hal::rng::Trng::try_new().unwrap();
        let noise_bytes: [u8; 16] = rand::Rng::random(&mut rng);

        let advert_data = AdvertisementExtraData {
            flags: AppdataFlags::HAS_NAME | AppdataFlags::IS_CHAT_NODE,
            latitude: None,
            longitude: None,
            feature_1: None,
            feature_2: None,
            name: Some(self.identity.name.as_bytes().into()),
        };

        let advert = self.identity.make_advert(
            (self.rtc.current_time_us() / 1_000_000) as u32,
            advert_data,
            noise_bytes,
        );

        if flood {
            self.lora_tx
                .send_flood::<Advert>(&advert, Path::empty(meshcore::PathHashMode::OneByte))
                .await;
        } else {
            self.lora_tx
                .send_direct::<Advert>(&advert, Path::empty(meshcore::PathHashMode::OneByte))
                .await;
        }

        CompanionProtoResult::Ok(responses::Ok { code: None })
    }

    async fn set_advert_name(&mut self, name: &str) -> CompanionProtoResult<responses::Ok> {
        self.identity.name = name.to_string();
        self.config_db
            .insert(littlefs2::path!("/companion/identity"), &self.identity)
            .await?;
        Ok(responses::Ok { code: None })
    }

    async fn set_radio_params(
        &mut self,
        _freq: u32,
        _bandwidth: u32,
        _spreading_factor: u8,
        _coding_rate: u8,
    ) -> CompanionProtoResult<responses::Ok> {
        todo!()
    }

    async fn set_tx_power(&mut self, _power: u8) -> CompanionProtoResult<responses::Ok> {
        todo!()
    }

    async fn reset_path(&mut self, pk: &[u8; 32]) -> CompanionProtoResult<responses::Ok> {
        let Some(_contact) = self.contacts.get_cached(&pk[..]) else {
            return CompanionProtoResult::Err(responses::Err { code: None });
        };

        let mut full_contact = self.contacts.get_full(pk).await.unwrap().unwrap();
        full_contact.path_to = None;
        self.contacts.insert(&full_contact).await?;

        CompanionProtoResult::Ok(responses::Ok { code: None })
    }

    async fn set_lat_long(&mut self, _lat: u32, _long: u32) -> CompanionProtoResult<responses::Ok> {
        todo!()
    }

    async fn add_update_contact(
        &mut self,
        contact: Contact,
    ) -> CompanionProtoResult<responses::Ok> {
        self.contacts.insert(&contact).await?;
        CompanionProtoResult::Ok(responses::Ok { code: None })
    }

    async fn remove_contact(&mut self, contact: &[u8; 32]) -> CompanionProtoResult<responses::Ok> {
        self.contacts.delete(contact).await?;
        CompanionProtoResult::Ok(responses::Ok { code: None })
    }

    async fn sync_next_message<'a>(
        &'a mut self,
    ) -> CompanionProtoResult<responses::GetMessageRes<'a>> {
        CompanionProtoResult::Ok(match self.log.get_mut().pop().await {
            Some(SavedMessage::Channel(c)) => responses::GetMessageRes::Channel(c),
            Some(SavedMessage::Contact(c)) => responses::GetMessageRes::Contact(c),
            None => responses::GetMessageRes::NoMoreMessages,
        })
    }

    async fn get_battery(&mut self) -> CompanionProtoResult<responses::Battery> {
        CompanionProtoResult::Ok(responses::Battery {
            battery_voltage: 0,
            used_storage: 0,
            total_storage: 0,
        })
    }

    async fn get_contacts<'s>(
        &'s mut self,
        _since: Option<u32>,
    ) -> (u32, impl Iterator<Item = Contact> + 's) {
        let len = self.contacts.cache.len();
        let mut contacts = Vec::with_capacity(len);
        for cached_contact in &self.contacts.cache {
            contacts.push(
                self.contacts
                    .get_full(cached_contact.key())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }

        (len as u32, contacts.into_iter())
        // let entries = self.contacts.cache.iter(|cached| );
    }

    async fn send_login(
        &mut self,
        pk: &[u8; 32],
        password: &[u8],
    ) -> CompanionProtoResult<responses::MsgSent> {
        let login = RepeaterLogin {
            timestamp: (self.rtc.current_time_us() / 1_000_000) as u32,
            password: password.into(),
        };

        let mut encrypt_scratch = BumpaloVec::new_in(&self.scratch);
        let mut encode_scratch = BumpaloVec::new_in(&self.scratch);
        let res = self
            .identity
            .make_anon_req::<RepeaterLogin, HardwareAES>(
                &login,
                &ForeignIdentity::new(*pk),
                &mut encrypt_scratch,
            )
            .await?;

        // let ack = Ack::calculate(msg, sender)
        let res = AnonymousRequest::encode_into_vec(&res, &mut encode_scratch).unwrap();

        let packet = if let Some(path) = self
            .contacts
            .get_cached(pk)
            .and_then(|v| v.path_to.as_ref())
        {
            Packet::direct::<AnonymousRequest<RepeaterLogin>>(path.clone(), res)
        } else {
            Packet::flood::<AnonymousRequest<RepeaterLogin>>(
                Path::empty(meshcore::PathHashMode::OneByte),
                res,
            )
        };

        self.lora_tx.send_packet(&packet).await;
        // todo fix msgsent ack code
        CompanionProtoResult::Ok(responses::MsgSent {
            is_flood: matches!(
                packet.header.route_type(),
                RouteType::Flood | RouteType::TransportFlood
            ),
            expected_ack: [0; 4],
            suggested_timeout: packet
                .timeout_est(&packet.path, packet.header.route_type())
                .as_secs() as u32,
        })
    }

    async fn sign_start(&mut self) -> CompanionProtoResult<responses::SignStart> {
        let noise = Noise::new(rand::Rng::random(&mut esp_hal::rng::Rng::new()));
        self.signature_in_progress = Some(self.identity.signing_keys.sk.sign_incremental(noise));

        CompanionProtoResult::Ok(responses::SignStart {
            reserved: 0,
            max_len: 16384,
        })
    }

    async fn sign_data(&mut self, data: &[u8]) -> CompanionProtoResult<responses::Ok> {
        let Some(signature) = self.signature_in_progress.as_mut() else {
            return CompanionProtoResult::Err(responses::Err { code: None });
        };

        signature.absorb(data);

        CompanionProtoResult::Ok(responses::Ok { code: None })
    }

    async fn sign_finish(&mut self) -> CompanionProtoResult<responses::SignatureResponse> {
        let Some(signature) = self.signature_in_progress.take() else {
            return CompanionProtoResult::Err(responses::Err { code: None });
        };

        CompanionProtoResult::Ok(responses::SignatureResponse {
            signature: *signature.sign(),
        })
    }

    async fn export_private_key(&mut self) -> CompanionProtoResult<responses::PrivateKeyResponse> {
        CompanionProtoResult::Ok(responses::PrivateKeyResponse {
            key: *self.identity.signing_keys.sk,
        })
    }

    async fn get_custom_vars<'s>(&'s mut self) -> CompanionProtoResult<responses::CustomVars<'s>> {
        Ok(CustomVars(self.config.as_vars()))
    }

    async fn set_custom_var(
        &mut self,
        key: &str,
        val: &str,
    ) -> CompanionProtoResult<responses::Ok> {
        self.config.set(key, val);
        self.config_db
            .insert(littlefs2::path!("/companion/config"), &self.config)
            .await?;
        Ok(responses::Ok { code: None })
    }
}
