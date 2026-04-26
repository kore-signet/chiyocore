use chiyocore::meshcore::{DecodeResult, Path, payloads::TextType};
use chiyocore::simple_mesh::storage::contact::Contact;
use meshcore_companion_protocol::commands::HostCommand;
use meshcore_companion_protocol::responses::{CompanionProtoResult, CustomVars};
use meshcore_companion_protocol::{CompanionSer, responses};
use smallvec::SmallVec;

pub trait CompanionSink {
    fn write_packet(&mut self, packet: &impl CompanionSer) -> impl Future<Output = ()>;
}

#[derive(Clone)]
pub struct ChannelCompanionSink {
    tx: thingbuf::mpsc::Sender<SmallVec<[u8; 256]>>,
}

impl ChannelCompanionSink {
    pub fn new(tx: thingbuf::mpsc::Sender<SmallVec<[u8; 256]>>) -> Self {
        ChannelCompanionSink {
            tx,
            // scratch: Vec::with_capacity(32),
        }
    }
}

impl CompanionSink for ChannelCompanionSink {
    async fn write_packet(&mut self, packet: &impl CompanionSer) {
        let Ok(mut slot) = self.tx.try_send_ref() else {
            return;
        };

        let size = packet.ser_size();
        slot.push(b'\x3e');
        slot.extend_from_slice(&(size as u16).to_le_bytes());
        slot.resize(size + 3, 0);
        packet.companion_serialize(&mut slot[3..]);
        // slot.extend_from_slice(data);
        drop(slot);
    }
}

pub async fn parse_packet(
    packet: &[u8],
    handler: &mut impl CompanionHandler,
    out: &mut impl CompanionSink,
) -> DecodeResult<()> {
    use HostCommand::*;
    use meshcore_companion_protocol::commands;
    handler.start_req();

    match HostCommand::companion_deserialize(packet)? {
        AppStart(commands::AppStart {
            app_ver,
            reserved,
            app_name,
        }) => {
            out.write_packet(&handler.app_start(app_ver, reserved, &app_name).await)
                .await
        }
        SendTxtMsg(commands::SendTxtMsg {
            txt_type,
            attempt,
            timestamp,
            pubkey,
            text,
        }) => {
            out.write_packet(
                &handler
                    .send_contact_message(txt_type, attempt, timestamp, &pubkey, &text)
                    .await,
            )
            .await
        }
        SendChannelTxtMsg(commands::SendChannelTxtMsg {
            txt_type,
            channel_idx,
            timestamp,
            text,
        }) => {
            out.write_packet(
                &handler
                    .send_channel_message(txt_type, channel_idx, timestamp, &text)
                    .await,
            )
            .await
        }
        GetContacts(commands::GetContacts { since }) => {
            let res = handler.get_contacts(since, out).await;
            out.write_packet(&res).await;
        }
        GetDeviceTime => out.write_packet(&handler.get_time().await).await,
        SetDeviceTime(commands::SetDeviceTime { timestamp }) => {
            out.write_packet(&handler.set_time(timestamp).await).await
        }
        SendSelfAdvert(commands::SendSelfAdvert { flood }) => {
            out.write_packet(&handler.send_self_advert(flood).await)
                .await
        }
        SetAdvertName(commands::SetAdvertName { name }) => {
            out.write_packet(&handler.set_advert_name(&name).await)
                .await
        }
        AddUpdateContact(commands::AddUpdateContact(contact)) => {
            out.write_packet(&handler.add_update_contact(contact).await)
                .await
        }
        SyncNextMessage => out.write_packet(&handler.sync_next_message().await).await,
        SetRadioParams(commands::SetRadioParams { freq, bw, sf, cr }) => {
            out.write_packet(&handler.set_radio_params(freq, bw, sf, cr).await)
                .await
        }
        SetTxPower(commands::SetTxPower { tx_power }) => {
            out.write_packet(&handler.set_tx_power(tx_power).await)
                .await
        }
        ResetPath(commands::ResetPath { pubkey }) => {
            out.write_packet(&handler.reset_path(&pubkey).await).await
        }
        SetAdvertLatLon(commands::SetAdvertLatLon { lat, lon }) => {
            out.write_packet(&handler.set_lat_long(lat as u32, lon as u32).await)
                .await
        } // this is probably wrong in conversion i need to figure out how it actually sends lat/long
        RemoveContact(commands::RemoveContact { pubkey }) => {
            out.write_packet(&handler.remove_contact(&pubkey).await)
                .await
        }
        ShareContact(commands::ShareContact { pubkey: _ }) => todo!(),
        ExportContact(_export_contact) => todo!(),
        ImportContact(_import_contact) => todo!(),
        Reboot => todo!(),
        GetBatteryVoltage => out.write_packet(&handler.get_battery().await).await,
        SetTuningParams => todo!(),
        DeviceQuery(commands::DeviceQuery { app_target_ver }) => {
            out.write_packet(&handler.device_query(app_target_ver).await)
                .await
        }
        ExportPrivateKey => out.write_packet(&handler.export_private_key().await).await,
        ImportPrivateKey(_import_private_key) => todo!(),
        SendRawData(_send_raw_data) => todo!(),
        SendLogin(commands::SendLogin { pubkey, password }) => {
            out.write_packet(&handler.send_login(&pubkey, password.as_bytes()).await)
                .await
        }
        SendStatusReq(commands::SendStatusReq { pubkey: _ }) => todo!(),
        GetChannel(commands::GetChannel { idx }) => {
            out.write_packet(&handler.channel_info(idx).await).await
        }
        SetChannel(commands::SetChannel { idx, name, secret }) => {
            out.write_packet(&handler.set_channel(idx, &name, &secret).await)
                .await
        }
        SignStart => out.write_packet(&handler.sign_start().await).await,
        SignData(commands::SignData(data)) => {
            out.write_packet(&handler.sign_data(&data).await).await
        }
        SignFinish => out.write_packet(&handler.sign_finish().await).await,
        SendTracePath(commands::SendTracePath {
            tag,
            auth,
            flags,
            path,
        }) => {
            out.write_packet(&handler.send_trace(tag, auth, flags, path).await)
                .await
        }
        SetOtherParams => todo!(),
        SendTelemetryReq => todo!(),
        SendBinaryReq(commands::SendBinaryReq {
            pubkey,
            req_code_params,
        }) => {
            out.write_packet(&handler.send_binary_req(&pubkey, &req_code_params).await)
                .await
        }
        SetFloodScope(_set_flood_scope) => {},
        GetCustomVars => {
            out.write_packet(&handler.get_custom_vars().await).await
        }
        SetCustomVar(commands::SetCustomVar { key, value }) => {
            out.write_packet(&handler.set_custom_var(&key, &value).await)
                .await
        }
        SendControlData(commands::SendControlData(data)) => {
            out.write_packet(&handler.send_control_data(&data).await)
                .await
        }
        GetStats(commands::GetStats { stats_type }) => match stats_type {
            responses::StatTypes::Core => out.write_packet(&handler.get_core_stats().await).await,
            responses::StatTypes::Radio => out.write_packet(&handler.get_radio_stats().await).await,
            responses::StatTypes::Packets => {
                out.write_packet(&handler.get_packet_stats().await).await
            }
        },
        SendAnonReq(commands::SendAnonReq { pubkey, data }) => {
            out.write_packet(&handler.send_anon_req(&pubkey, &data).await)
                .await
        }
    }

    Ok(())
}

pub trait CompanionHandler {
    fn start_req(&mut self);

    fn app_start<'a>(
        &'a mut self,
        app_ver: u8,
        reserved: [u8; 6],
        app_name: &str,
    ) -> impl Future<Output = CompanionProtoResult<responses::SelfInfo<'a>>>;

    fn device_query<'a>(
        &'a mut self,
        app_ver: u8,
    ) -> impl Future<Output = CompanionProtoResult<responses::DeviceInfo<'a>>>;

    fn channel_info(
        &mut self,
        idx: u8,
    ) -> impl Future<Output = CompanionProtoResult<responses::ChannelInfo<'_>>>;

    fn set_channel(
        &mut self,
        idx: u8,
        name: &str,
        secret: &[u8; 16],
    ) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;

    fn send_channel_message(
        &mut self,
        text_type: TextType,
        idx: u8,
        timestamp: u32,
        txt: &str,
    ) -> impl Future<Output = CompanionProtoResult<chiyocore::simple_mesh::MsgSent>>;

    fn send_contact_message(
        &mut self,
        text_type: TextType,
        attempt: u8,
        timestamp: u32,
        destination: &[u8; 6],
        text: &str,
    ) -> impl Future<Output = CompanionProtoResult<chiyocore::simple_mesh::MsgSent>>;

    fn get_time(&mut self) -> impl Future<Output = CompanionProtoResult<responses::CurrentTime>>;
    fn set_time(&mut self, time: u32) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;

    // fn get_time(&mut self) -> CompanionResult<>

    fn send_self_advert(
        &mut self,
        flood: bool,
    ) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;

    fn set_advert_name(
        &mut self,
        name: &str,
    ) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;

    fn set_radio_params(
        &mut self,
        freq: u32,
        bandwidth: u32,
        spreading_factor: u8,
        coding_rate: u8,
    ) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;

    fn set_tx_power(
        &mut self,
        power: u8,
    ) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;

    fn reset_path(
        &mut self,
        pk: &[u8; 32],
    ) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;

    fn set_lat_long(
        &mut self,
        lat: u32,
        long: u32,
    ) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;

    fn add_update_contact(
        &mut self,
        contact: Contact,
    ) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;

    fn remove_contact(
        &mut self,
        contact: &[u8; 32],
    ) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;

    fn sync_next_message<'a>(
        &'a mut self,
    ) -> impl Future<Output = CompanionProtoResult<responses::GetMessageRes<'a>>>;

    fn get_battery(&mut self) -> impl Future<Output = CompanionProtoResult<responses::Battery>>;

    fn send_login(
        &mut self,
        pk: &[u8; 32],
        password: &[u8],
    ) -> impl Future<Output = CompanionProtoResult<chiyocore::simple_mesh::MsgSent>>;

    fn get_contacts(
        &mut self,
        since: Option<u32>,
        out: &mut impl CompanionSink,
    ) -> impl Future<Output = CompanionProtoResult<responses::ContactEnd>>;

    fn sign_start(&mut self) -> impl Future<Output = CompanionProtoResult<responses::SignStart>>;

    fn sign_data(
        &mut self,
        data: &[u8],
    ) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;

    fn sign_finish(
        &mut self,
    ) -> impl Future<Output = CompanionProtoResult<responses::SignatureResponse>>;

    fn export_private_key(
        &mut self,
    ) -> impl Future<Output = CompanionProtoResult<responses::PrivateKeyResponse>>;

    fn get_custom_vars(&mut self) -> impl Future<Output = CompanionProtoResult<CustomVars>>;

    fn set_custom_var(
        &mut self,
        key: &str,
        val: &str,
    ) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;

    fn get_core_stats(
        &mut self,
    ) -> impl Future<Output = CompanionProtoResult<responses::CoreStats>>;
    fn get_radio_stats(
        &mut self,
    ) -> impl Future<Output = CompanionProtoResult<responses::RadioStats>>;
    fn get_packet_stats(
        &mut self,
    ) -> impl Future<Output = CompanionProtoResult<responses::PacketStats>>;

    fn send_control_data(
        &mut self,
        data: &[u8],
    ) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;

    fn send_trace(
        &mut self,
        tag: [u8; 4],
        auth_code: [u8; 4],
        flags: u8,
        path: Path<'_>,
    ) -> impl Future<Output = CompanionProtoResult<chiyocore::simple_mesh::MsgSent>>;

    fn send_binary_req(
        &mut self,
        pub_key: &[u8; 32],
        data: &[u8],
    ) -> impl Future<Output = CompanionProtoResult<chiyocore::simple_mesh::MsgSent>>;

    fn send_anon_req(
        &mut self,
        pub_key: &[u8; 32],
        data: &[u8],
    ) -> impl Future<Output = CompanionProtoResult<chiyocore::simple_mesh::MsgSent>>;

    fn import_contact(
        &mut self,
        data: &[u8],
    ) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;
}
