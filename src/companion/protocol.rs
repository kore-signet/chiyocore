use alloc::string::String;
use meshcore::{
    DecodeError, DecodeResult, Path,
    io::{SliceWriter, TinyReadExt},
    payloads::TextType,
};
use modular_bitfield::Specifier as _;
use strum::FromRepr;

use crate::companion::{
    handler::storage::Contact,
    protocol::responses::{CompanionProtoResult, CustomVars},
};

pub trait CompanionSer {
    fn ser_size(&self) -> usize;
    fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8];
}

pub mod responses {
    use alloc::borrow::Cow;

    use lora_phy::mod_params::PacketStatus;
    use meshcore::{io::SliceWriter, payloads::TextType};
    use serde::{Deserialize, Serialize};
    use smallvec::SmallVec;

    use crate::FirmwareError;
    use crate::companion::handler::simple_companion::CompanionError;

    use super::CompanionSer;
    use super::ResponseCodes;

    use super::NullPaddedSlice;

    pub enum GetMessageRes<'a> {
        Contact(ContactMsgRecv<'a>),
        Channel(ChannelMsgRecv<'a>),
        NoMoreMessages,
    }

    impl<'a> CompanionSer for GetMessageRes<'a> {
        fn ser_size(&self) -> usize {
            match self {
                GetMessageRes::Contact(contact_msg_recv) => contact_msg_recv.ser_size(),
                GetMessageRes::Channel(channel_msg_recv) => channel_msg_recv.ser_size(),
                GetMessageRes::NoMoreMessages => 1,
            }
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            match self {
                GetMessageRes::Contact(contact_msg_recv) => {
                    contact_msg_recv.companion_serialize(out)
                }
                GetMessageRes::Channel(channel_msg_recv) => {
                    channel_msg_recv.companion_serialize(out)
                }
                GetMessageRes::NoMoreMessages => {
                    let mut out = SliceWriter::new(out);
                    out.write_u8(ResponseCodes::NoMoreMessages as u8);
                    out.finish()
                }
            }
        }
    }

    pub type CompanionProtoResult<T> = Result<T, Err>;
    // Res(T),
    // Err(Err),
    // }

    impl<T: CompanionSer> CompanionSer for CompanionProtoResult<T> {
        fn ser_size(&self) -> usize {
            match self {
                CompanionProtoResult::Ok(r) => r.ser_size(),
                CompanionProtoResult::Err(e) => e.ser_size(),
            }
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            match self {
                CompanionProtoResult::Ok(r) => r.companion_serialize(out),
                CompanionProtoResult::Err(e) => e.companion_serialize(out),
            }
        }
    }

    pub struct Ok {
        pub code: Option<u32>,
    }

    impl CompanionSer for Ok {
        fn ser_size(&self) -> usize {
            1 + if self.code.is_some() { 4 } else { 0 }
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);
            out.write_u8(ResponseCodes::Ok as u8);
            if let Some(code) = self.code {
                out.write_u32_le(code);
            }
            out.finish()
        }
    }

    pub struct Err {
        pub code: Option<u8>,
    }

    impl CompanionSer for Err {
        fn ser_size(&self) -> usize {
            1 + if self.code.is_some() { 1 } else { 0 }
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);
            out.write_u8(ResponseCodes::Ok as u8);
            if let Some(code) = self.code {
                out.write_u8(code);
            }
            out.finish()
        }
    }

    impl From<FirmwareError> for Err {
        fn from(_value: FirmwareError) -> Self {
            Err { code: None }
        }
    }

    impl From<CompanionError> for Err {
        fn from(_value: CompanionError) -> Self {
            Err { code: None }
        }
    }

    impl From<esp_hal::aes::Error> for Err {
        fn from(_value: esp_hal::aes::Error) -> Self {
            Err { code: None }
        }
    }

    pub struct SelfInfo<'a> {
        pub advertisement_type: u8,
        pub tx_power: u8,
        pub max_tx_power: u8,
        pub public_key: &'a [u8; 32],
        pub lat: u32,
        pub long: u32,
        pub multi_acks: u8,
        pub adv_loc_policy: u8,
        pub telemetry_mode: u8,
        pub manual_add_contacts: bool,
        pub radio_freq: u32,
        pub radio_bandwidth: u32,
        pub radio_sf: u8,
        pub radio_cr: u8,
        pub device_name: &'a str,
    }

    impl<'a> CompanionSer for SelfInfo<'a> {
        fn ser_size(&self) -> usize {
            4 // packet ty + adv ty + tx power + max tx power
            + 32 // pk
            + 4 * 2 // lat,long
            + 4 // multi acks + adv loc policy + telemetry mode + manual add contacts
            + 4 * 2 // radio freq + band
            + 2 // spreading factor coding rate
            + self.device_name.len()
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);

            out.write_slice(&[
                ResponseCodes::SelfInfo as u8,
                self.advertisement_type,
                self.tx_power,
                self.max_tx_power,
            ]);

            out.write_slice(self.public_key);
            out.write_u32_le(self.lat);
            out.write_u32_le(self.long);

            out.write_slice(&[
                self.multi_acks,
                self.adv_loc_policy,
                self.telemetry_mode,
                self.manual_add_contacts as u8,
            ]);

            out.write_u32_le(self.radio_freq);
            out.write_u32_le(self.radio_bandwidth);

            out.write_slice(&[self.radio_sf, self.radio_cr]);

            out.write_slice(self.device_name.as_bytes());

            out.finish()
        }
    }

    pub struct DeviceInfo<'a> {
        pub fw_version: u8,
        pub max_contacts: u8,
        pub max_channels: u8,
        pub ble_pin: u32,
        pub firmware_build: NullPaddedSlice<'a, 12>,
        pub model: NullPaddedSlice<'a, 40>,
        pub version: NullPaddedSlice<'a, 20>,
        pub client_repeat_enabled: bool,
        pub path_hash_mode: u8,
    }

    impl<'a> CompanionSer for DeviceInfo<'a> {
        fn ser_size(&self) -> usize {
            4 // packet ty, firmware ver, max contacts, max channels
            + 4 // ble pin
            + 12 // firmware build
            + 40 // model
            + 20 // version
            + 1 // client repeat enabled
            + 1 // path hash mode
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);

            out.write_slice(&[
                ResponseCodes::DeviceInfo as u8,
                self.fw_version,
                self.max_contacts,
                self.max_channels,
            ]);

            out.write_u32_le(self.ble_pin);
            self.firmware_build.encode_to(&mut out);
            self.model.encode_to(&mut out);
            self.version.encode_to(&mut out);

            out.write_slice(&[self.client_repeat_enabled as u8, self.path_hash_mode]);

            out.finish()
        }
    }

    pub struct ChannelInfo<'a> {
        pub idx: u8,
        pub name: NullPaddedSlice<'a, 32>,
        pub secret: [u8; 16],
    }

    impl<'a> CompanionSer for ChannelInfo<'a> {
        fn ser_size(&self) -> usize {
            2 // packet ty, channel idx
            + 32 // channel name
            + 16 // secret
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);
            out.write_slice(&[ResponseCodes::ChannelInfo as u8, self.idx]);
            self.name.encode_to(&mut out);
            out.write_slice(&self.secret);

            out.finish()
        }
    }

    pub struct Battery {
        pub battery_voltage: u16,
        pub used_storage: u32,
        pub total_storage: u32,
    }

    impl CompanionSer for Battery {
        fn ser_size(&self) -> usize {
            1 // packet ty
            + 2 // battery voltage
            + 4 // used storage
            + 4 // total storage
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);

            out.write_u8(ResponseCodes::Battery as u8);
            out.write_u16_le(self.battery_voltage);
            out.write_u32_le(self.used_storage);
            out.write_u32_le(self.total_storage);

            out.finish()
        }
    }

    pub struct MsgSent {
        pub is_flood: bool,
        pub expected_ack: [u8; 4],
        pub suggested_timeout: u32,
    }

    impl CompanionSer for MsgSent {
        fn ser_size(&self) -> usize {
            1 // packet ty
            + 1 // routing type
            + 4 // expected ack
            + 4 // suggested timeout
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);

            out.write_u8(ResponseCodes::MsgSent as u8);
            out.write_u8(self.is_flood as u8);
            out.write_slice(&self.expected_ack);
            out.write_u32_le(self.suggested_timeout);

            out.finish()
        }
    }

    #[derive(Serialize, Deserialize, Debug)]
    pub struct ContactMsgRecv<'a> {
        pub snr: i8,
        pub reserved: [u8; 2],
        pub pk_prefix: [u8; 6],
        pub path_len: u8,
        pub text_ty: TextType,
        pub timestamp: u32,
        pub signature: Option<[u8; 4]>,
        pub data: Cow<'a, [u8]>,
    }

    impl<'a> CompanionSer for ContactMsgRecv<'a> {
        fn ser_size(&self) -> usize {
            1 // packet ty
            + 1 // snr
            + 2 // reserved
            + 6 // pk prefix
            + 1 // path len
            + 1 // text type
            + 4 // timestamp
            + if self.signature.is_some() { 4 } else { 0 }
            + self.data.len()
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);

            out.write_u8(ResponseCodes::ContactMsgRecvV3 as u8);
            out.write_i8(self.snr);
            out.write_slice(&self.reserved);
            out.write_slice(&self.pk_prefix);
            out.write_u8(self.path_len);
            out.write_u8(self.text_ty as u8);
            out.write_u32_le(self.timestamp);

            if let Some(signature) = self.signature {
                out.write_slice(&signature);
            }

            out.write_slice(&self.data);

            out.finish()
        }
    }

    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct ChannelMsgRecv<'a> {
        pub snr: i8,
        pub reserved: [u8; 2],
        pub idx: u8,
        pub path_len: u8,
        pub text_ty: TextType,
        pub timestamp: u32,
        pub data: Cow<'a, [u8]>,
    }

    impl<'a> CompanionSer for ChannelMsgRecv<'a> {
        fn ser_size(&self) -> usize {
            1 // packet ty
            + 1 // snr
            + 2 // reserved
            + 1 // channel idx
            + 1 // path len
            + 1 // text ty
            + 4 // timestamp
            + self.data.len()
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);

            out.write_u8(ResponseCodes::ChannelMsgRecvV3 as u8);
            out.write_i8(self.snr);
            out.write_slice(&self.reserved);
            out.write_slice(&[self.idx, self.path_len, self.text_ty as u8]);
            out.write_u32_le(self.timestamp);
            out.write_slice(&self.data);

            out.finish()
        }
    }

    pub struct ContactStart {
        pub contacts: u32,
    }

    impl CompanionSer for ContactStart {
        fn ser_size(&self) -> usize {
            1 + 4 // packet_ty, len
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);
            out.write_u8(ResponseCodes::ContactsStart as u8);
            out.write_u32_le(self.contacts);
            out.finish()
        }
    }

    pub struct ContactEnd {
        pub last_mod: u32,
    }

    impl CompanionSer for ContactEnd {
        fn ser_size(&self) -> usize {
            1 + 4 // packet_ty, last_mod
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);

            out.write_u8(ResponseCodes::EndOfContacts as u8);
            out.write_u32_le(self.last_mod);

            out.finish()
        }
    }

    pub struct Ack {
        pub code: [u8; 4],
    }

    impl CompanionSer for Ack {
        fn ser_size(&self) -> usize {
            1 + 4 // packet_ty, last_mod
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);

            out.write_u8(ResponseCodes::Ack as u8);
            out.write_slice(&self.code);

            out.finish()
        }
    }

    pub struct RfLogData<'a> {
        pub snr: i8,
        pub rssi: i8,
        pub data: &'a [u8],
    }

    impl<'a> RfLogData<'a> {
        pub fn new(status: PacketStatus, data: &'a [u8]) -> RfLogData<'a> {
            RfLogData {
                snr: status.snr as i8,
                rssi: status.rssi as i8,
                data,
            }
        }
    }

    impl<'a> CompanionSer for RfLogData<'a> {
        fn ser_size(&self) -> usize {
            3 + self.data.len()
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);
            out.write_u8(ResponseCodes::LogData as u8);
            out.write_i8(self.snr);
            out.write_i8(self.rssi);
            out.write_slice(self.data);
            out.finish()
        }
    }

    pub struct CurrentTime {
        pub time: u32,
    }

    impl CompanionSer for CurrentTime {
        fn ser_size(&self) -> usize {
            5
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);
            out.write_u8(ResponseCodes::CurrTime as u8);
            out.write_u32_le(self.time);
            out.finish()
        }
    }

    pub struct LoginSuccess {
        pub reserved: u8,
        pub prefix: [u8; 6],
    }

    impl CompanionSer for LoginSuccess {
        fn ser_size(&self) -> usize {
            8
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);
            out.write_u8(ResponseCodes::LoginSuccess as u8);
            out.write_u8(self.reserved);
            out.write_slice(&self.prefix);
            out.finish()
        }
    }

    pub struct SignStart {
        pub reserved: u8,
        pub max_len: u32,
    }

    impl CompanionSer for SignStart {
        fn ser_size(&self) -> usize {
            6
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);
            out.write_u8(ResponseCodes::SignStart as u8);
            out.write_u8(self.reserved);
            out.write_u32_le(self.max_len);
            out.finish()
        }
    }

    pub struct SignatureResponse {
        pub signature: [u8; 64],
    }

    impl CompanionSer for SignatureResponse {
        fn ser_size(&self) -> usize {
            65
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);
            out.write_u8(ResponseCodes::Signature as u8);
            out.write_slice(&self.signature);
            out.finish()
        }
    }

    pub struct PrivateKeyResponse {
        pub key: [u8; 64],
    }

    impl CompanionSer for PrivateKeyResponse {
        fn ser_size(&self) -> usize {
            65
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);
            out.write_u8(ResponseCodes::ExportPrivateKey as u8);
            out.write_slice(&self.key);
            out.finish()
        }
    }

    pub struct CustomVars<'a>(pub SmallVec<[(&'a str, &'a str); 8]>);

    impl<'a> CompanionSer for CustomVars<'a> {
        fn ser_size(&self) -> usize {
            1 + self
                .0
                .iter()
                .map(|(k, v)| k.len() + v.len() + 1 /* for the ':' */)
                .sum::<usize>()
                + (self.0.len().saturating_sub(1)/* for the ','s */)
        }

        fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
            let mut out = SliceWriter::new(out);
            out.write_u8(ResponseCodes::CustomVars as u8);
            let mut iter = self.0.iter().peekable();
            while let Some((k, v)) = iter.next() {
                out.write_slice(k.as_bytes());
                out.write_u8(b':');
                out.write_slice(v.as_bytes());
                if iter.peek().is_some() {
                    out.write_u8(b',');
                }
            }

            out.finish()
        }
    }
}

pub struct NullPaddedSlice<'a, const SIZE: usize>(pub &'a [u8]);

impl<'a, const SIZE: usize> From<&'a [u8]> for NullPaddedSlice<'a, SIZE> {
    fn from(value: &'a [u8]) -> Self {
        debug_assert!(value.len() <= SIZE);
        NullPaddedSlice(value)
    }
}

impl<'a, const SIZE: usize> From<&'a str> for NullPaddedSlice<'a, SIZE> {
    fn from(value: &'a str) -> Self {
        debug_assert!(value.len() <= SIZE);
        NullPaddedSlice(value.as_bytes())
    }
}

impl<'a, const SIZE: usize> NullPaddedSlice<'a, SIZE> {
    pub fn encode_to(&self, out: &mut SliceWriter<'_>) {
        let to_pad = SIZE - self.0.len();
        out.write_slice(self.0);
        out.write_repeated(0, to_pad);
    }
}

#[repr(u8)]
pub enum ResponseCodes {
    Ok = 0x00,
    Err = 0x01,
    ContactsStart = 0x02,
    Contact = 0x03,
    EndOfContacts = 0x04,
    SelfInfo = 0x05,
    MsgSent = 0x06,
    ContactMsgRecv = 0x07,
    ChannelMsgRecv = 0x08,
    ContactMsgRecvV3 = 0x10,
    ChannelMsgRecvV3 = 0x11,
    CurrTime = 0x09,
    NoMoreMessages = 0x0A,
    Battery = 0x0C,
    DeviceInfo = 0x0D,
    ChannelInfo = 0x12,
    Advertisement = 0x80,
    Ack = 0x82, // ?
    MessagesWaiting = 0x83,
    LogData = 0x88,
    LoginSuccess = 0x85,
    SignStart = 0x13,
    Signature = 0x20,
    ExportPrivateKey = 0xe,
    CustomVars = 0x15,
}

#[derive(FromRepr)]
#[repr(u8)]
pub enum HostCommandType {
    AppStart = 1,
    SendTxtMsg = 2,
    SendChannelTxtMsg = 3,
    GetContacts = 4,
    GetDeviceTime = 5,
    SetDeviceTime = 6,
    SendSelfAdvert = 7,
    SetAdvertName = 8,
    AddUpdateContact = 9,
    SyncNextMessage = 10,
    SetRadioParams = 11,
    SetTxPower = 12,
    ResetPath = 13,
    SetAdvertLatLon = 14,
    RemoveContact = 15,
    ShareContact = 16,
    ExportContact = 17,
    ImportContact = 18,
    Reboot = 19,
    GetBatteryVoltage = 20,
    SetTuningParams = 21,
    DeviceQuery = 22,
    ExportPrivateKey = 23,
    ImportPrivateKey = 24,
    SendRawData = 25,
    SendLogin = 26,
    SendStatusReq = 27,
    GetChannel = 31,
    SetChannel = 32,
    SignStart = 33,
    SignData = 34,
    SignFinish = 35,
    SendTracePath = 36,
    SetOtherParams = 38,
    SendTelemtryReq = 39,
    SendBinaryReq = 50,
    SetFloodScope = 54,
    GetCustomVars = 40,
    SetCustomVar = 41,
}

pub trait CompanionSink {
    fn write_packet(&mut self, packet: &impl CompanionSer) -> impl Future<Output = ()>;
}

pub async fn parse_packet(
    mut packet: &[u8],
    handler: &mut impl CompanionHandler,
    out: &mut impl CompanionSink,
) -> DecodeResult<()> {
    use HostCommandType::*;
    handler.start_req();
    match HostCommandType::from_repr(packet.read_u8()?).ok_or(DecodeError::InvalidBitPattern)? {
        AppStart => {
            let app_ver = packet.read_u8()?;
            let reserved = packet.read_chunk::<6>()?;
            let name = core::str::from_utf8(packet)?;
            out.write_packet(&handler.app_start(app_ver, *reserved, name).await)
                .await;
        }
        SendTxtMsg => {
            let txt_type = TextType::from_bytes(packet.read_u8()?)
                .map_err(|_| DecodeError::InvalidBitPattern)?;
            let attempt = packet.read_u8()?;
            let timestamp = packet.read_u32_le()?;
            let destination = packet.read_chunk::<6>()?;
            let text = core::str::from_utf8(packet)?;
            out.write_packet(
                &handler
                    .send_contact_message(txt_type, attempt, timestamp, destination, text)
                    .await,
            )
            .await;
        }
        SendChannelTxtMsg => {
            let txt_type = TextType::from_bytes(packet.read_u8()?)
                .map_err(|_| DecodeError::InvalidBitPattern)?;
            let idx = packet.read_u8()?;
            let timestamp = packet.read_u32_le()?;
            let text = core::str::from_utf8(packet)?;
            out.write_packet(
                &handler
                    .send_channel_message(txt_type, idx, timestamp, text)
                    .await,
            )
            .await;
        }
        GetContacts => {
            // todo: impl since
            let (contacts_len, contacts) = handler.get_contacts(None).await;
            out.write_packet(&responses::ContactStart {
                contacts: contacts_len,
            })
            .await;

            for contact in contacts {
                out.write_packet(&contact).await;
            }

            out.write_packet(&responses::ContactEnd { last_mod: 0 })
                .await; // todo: impl last_end
        }
        GetCustomVars => {
            out.write_packet(&handler.get_custom_vars().await).await;
        }
        SetCustomVar => {
            let Ok(packet) = core::str::from_utf8(packet) else {
                out.write_packet(&responses::Err { code: None }).await;
                return Ok(());
            };

            let Some((key, val)) = packet.split_once(':') else {
                out.write_packet(&responses::Err { code: None }).await;
                return Ok(());
            };

            out.write_packet(&handler.set_custom_var(key, val).await)
                .await;
        }
        GetDeviceTime => {
            out.write_packet(&handler.get_time().await).await;
        }
        SetDeviceTime => {
            let timestamp = packet.read_u32_le()?;
            out.write_packet(&handler.set_time(timestamp).await).await;
        }
        SendSelfAdvert => {
            let is_flood = !packet.is_empty() && packet.read_u8()? > 0;
            // let is_flood = packet.read_u8()? > 0; // technically accepts invalid bit patterns (1.. interpreted as flood instead of just 1, but like. c'mon)
            out.write_packet(&handler.send_self_advert(is_flood).await)
                .await;
        }
        SetAdvertName => {
            let s = core::str::from_utf8(packet)?;
            out.write_packet(&handler.set_advert_name(s).await).await;
        }
        AddUpdateContact => {
            let pk = packet.read_chunk::<32>()?;
            let _ty = packet.read_u8()?;
            let flags = packet.read_u8()?;
            let out_path_len = packet.read_u8()?;
            let path = packet.read_slice(out_path_len as usize)?;
            let name = core::str::from_utf8(packet.read_slice(32)?)?.trim_end_matches('\x00');
            let last_adv = packet.read_u32_le()?;
            let lat = packet.read_u32_le()?;
            let long = packet.read_u32_le()?;
            let contact = Contact {
                key: *pk,
                name: String::from(name),
                path_to: Some(Path::from_bytes(meshcore::PathHashMode::OneByte, path).to_owned()),
                flags,
                latitude: lat,
                longitude: long,
                last_heard: last_adv,
            };
            out.write_packet(&handler.add_update_contact(contact).await)
                .await;
        }
        SyncNextMessage => {
            out.write_packet(&handler.sync_next_message().await).await; // todo: convert this to an iter like the contacts
        }
        SetRadioParams => {
            let freq = packet.read_u32_le()?;
            let bandwidth = packet.read_u32_le()?;
            let spreading_factor = packet.read_u8()?;
            let coding_rate = packet.read_u8()?;
            out.write_packet(
                &handler
                    .set_radio_params(freq, bandwidth, spreading_factor, coding_rate)
                    .await,
            )
            .await;
        }
        SetTxPower => {
            let power = packet.read_u8()?;
            out.write_packet(&handler.set_tx_power(power).await).await;
        }
        ResetPath => {
            let pk = packet.read_chunk::<32>()?;
            out.write_packet(&handler.reset_path(pk).await).await;
        }
        SetAdvertLatLon => {
            let lat = packet.read_u32_le()?;
            let long = packet.read_u32_le()?;
            out.write_packet(&handler.set_lat_long(lat, long).await)
                .await;
        }
        RemoveContact => {
            let pk = packet.read_chunk::<32>()?;
            out.write_packet(&handler.remove_contact(pk).await).await;
        }
        ShareContact => todo!(),
        ExportContact => todo!(),
        ImportContact => todo!(),
        Reboot => todo!(),
        GetBatteryVoltage => {
            out.write_packet(&handler.get_battery().await).await;
        }
        SetTuningParams => todo!(),
        DeviceQuery => {
            let app_ver = packet.read_u8()?;
            out.write_packet(&handler.device_query(app_ver).await).await;
        }
        ExportPrivateKey => {
            out.write_packet(&handler.export_private_key().await).await;
        }
        ImportPrivateKey => todo!(),
        SendRawData => {
            out.write_packet(&responses::Ok { code: None }).await;
        }
        SendLogin => {
            let pk = packet.read_chunk::<32>()?;
            let password = packet;
            handler.send_login(pk, password).await;
        }
        SendStatusReq => todo!(),
        GetChannel => {
            let idx = packet.read_u8()?;
            out.write_packet(&handler.channel_info(idx).await).await;
        }
        SetChannel => {
            let idx = packet.read_u8()?;
            let name = core::str::from_utf8(packet.read_chunk::<32>()?)?.trim_end_matches('\x00');
            let secret = packet.read_chunk::<16>()?;
            out.write_packet(&handler.set_channel(idx, name, secret).await)
                .await;
        }
        SignStart => {
            out.write_packet(&handler.sign_start().await).await;
        }
        SignData => {
            out.write_packet(&handler.sign_data(packet).await).await;
        }
        SignFinish => {
            out.write_packet(&handler.sign_finish().await).await;
        }
        SendTracePath => todo!(),
        SetOtherParams => {
            out.write_packet(&responses::Ok { code: None }).await;
        }
        SendTelemtryReq => todo!(),
        SendBinaryReq => todo!(),
        SetFloodScope => {
            out.write_packet(&responses::Ok { code: None }).await;
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

    fn channel_info<'a>(
        &'a mut self,
        idx: u8,
    ) -> impl Future<Output = CompanionProtoResult<responses::ChannelInfo<'a>>>;

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
    ) -> impl Future<Output = CompanionProtoResult<responses::MsgSent>>;

    fn send_contact_message(
        &mut self,
        text_type: TextType,
        attempt: u8,
        timestamp: u32,
        destination: &[u8; 6],
        text: &str,
    ) -> impl Future<Output = CompanionProtoResult<responses::MsgSent>>;

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
    ) -> impl Future<Output = CompanionProtoResult<responses::MsgSent>>;

    fn get_contacts<'s>(
        &'s mut self,
        since: Option<u32>,
    ) -> impl Future<Output = (u32, impl Iterator<Item = Contact> + 's)>; // (len, iter)

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

    fn get_custom_vars<'s>(
        &'s mut self,
    ) -> impl Future<Output = CompanionProtoResult<CustomVars<'s>>>;

    fn set_custom_var(
        &mut self,
        key: &str,
        val: &str,
    ) -> impl Future<Output = CompanionProtoResult<responses::Ok>>;
}
