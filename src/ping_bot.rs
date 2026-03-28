use alloc::{format, sync::Arc};
use esp_hal::rtc_cntl::Rtc;
use meshcore::payloads::TextMessageData;

use crate::{CompanionResult, simple_mesh::SimpleMeshLayer};

pub struct PingBot {
    pub rtc: Arc<Rtc<'static>>,
}

impl SimpleMeshLayer for PingBot {
    async fn packet<'f>(
        &'f mut self,
        _mesh: &'f crate::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_bytes: &'f [u8],
        _packet_status: lora_phy::mod_params::PacketStatus,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn text_message<'f>(
        &'f mut self,
        _mesh: &'f crate::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: lora_phy::mod_params::PacketStatus,
        _contact: &'f crate::simple_mesh::storage::contact::CachedContact,
        _message: &'f TextMessageData<'_>,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn group_text<'f>(
        &'f mut self,
        mesh: &'f crate::simple_mesh::SimpleMesh,
        packet: &'f meshcore::Packet<'_>,
        _packet_status: lora_phy::mod_params::PacketStatus,
        channel: &'f crate::simple_mesh::storage::channel::Channel,
        message: &'f meshcore::payloads::TextMessageData<'_>,
    ) -> CompanionResult<()> {
        // log::info!("nya: {}", message.as_utf8().unwrap());
        if !(channel.name == "#test" || channel.name == "#emitestcorner") {
            return Ok(());
        }

        let msg = message.as_utf8()?.trim_end_matches('\0').trim();
        let Some((user, msg)) = msg.split_once(':') else {
            return Ok(());
        };

        if msg.trim() == "!ping" {
            let message = format!(
                "cafe / chiyobot 🌃☕: pong @[{user}] ＼(≧▽≦)／\npath: {:?}",
                packet.path
            );
            let message = TextMessageData::plaintext(
                (self.rtc.current_time_us() / 1_000_000) as u32,
                message.as_bytes(),
            );
            mesh.send_channel_message(&channel.as_keys(), &message)
                .await;
        }

        Ok(())
    }

    async fn ack<'f>(
        &'f mut self,
        _mesh: &'f crate::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: lora_phy::mod_params::PacketStatus,
        _ack: &'f meshcore::payloads::Ack,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn advert<'f>(
        &'f mut self,
        _mesh: &'f crate::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: lora_phy::mod_params::PacketStatus,
        _advert: &'f meshcore::payloads::Advert<'_>,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn returned_path<'f>(
        &'f mut self,
        _mesh: &'f crate::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: lora_phy::mod_params::PacketStatus,
        _contact: &'f crate::simple_mesh::storage::contact::CachedContact,
        _path: &'f meshcore::payloads::ReturnedPath<'_>,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn response<'f>(
        &'f mut self,
        _mesh: &'f crate::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: lora_phy::mod_params::PacketStatus,
        _contact: &'f crate::simple_mesh::storage::contact::CachedContact,
        _response: &'f [u8],
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn request<'f>(
        &'f mut self,
        _mesh: &'f crate::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: lora_phy::mod_params::PacketStatus,
        _contact: &'f crate::simple_mesh::storage::contact::CachedContact,
        _request: &'f meshcore::payloads::RequestPayload<'_>,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn anonymous_request<'f>(
        &'f mut self,
        _mesh: &'f crate::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: lora_phy::mod_params::PacketStatus,
        _contact: &'f meshcore::identity::ForeignIdentity,
        _data: &'f [u8],
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn trace_packet<'f>(
        &'f mut self,
        _mesh: &'f crate::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: lora_phy::mod_params::PacketStatus,
        _snrs: &'f [i8],
        _trace: &'f meshcore::payloads::TracePacket<'_>,
    ) -> CompanionResult<()> {
        Ok(())
    }

    async fn control_packet<'f>(
        &'f mut self,
        _mesh: &'f crate::simple_mesh::SimpleMesh,
        _packet: &'f meshcore::Packet<'_>,
        _packet_status: lora_phy::mod_params::PacketStatus,
        _payload: &'f meshcore::payloads::ControlPayload,
    ) -> CompanionResult<()> {
        Ok(())
    }
}

// use esp_println::println;
// use lora_phy::mod_params::PacketStatus;
// use meshcore::{Packet, payloads::TextMessageData};

// use crate::companion::handler::{
//     CompanionLayer,
//     simple_companion::{CompanionResult, SimpleCompanion},
//     storage::Channel,
// };

// impl CompanionLayer for PingBot {
//     async fn channel_message(
//         &self,
//         scratch: &bumpalo::Bump,
//         channel: &Channel,
//         (packet, packet_status): (&Packet<'_>, PacketStatus),
//         text: &TextMessageData<'_>,
//         handler: &SimpleCompanion<impl CompanionLayer + Send>,
//     ) -> CompanionResult<()> {
//         if !(channel.name == "#test" || channel.name == "#emitestcorner") {
//             return Ok(());
//         }

//         let message = text.as_utf8().unwrap().trim_end_matches('\0');
//         let Some((username, msg)) = message.split_once(':') else {
//             return Ok(());
//         };

//         if msg.trim() == "!ping" {
//             let delay = (handler.rtc.current_time_us() / 1_000_000) as i32 - text.timestamp as i32;

//             let ping_message = bumpalo::format!(in scratch, "cafe / chiyobot 🌃☕: pong @[{}] ＼(≧▽≦)／\npath: {:?}\nsnr {} db | rssi {} dBm\ndelay: {delay}s", username, packet.path, packet_status.snr, packet_status.rssi);

//             handler
//                 .send_to_channel(scratch, channel.idx, &ping_message, None)
//                 .await?;
//         }

//         Ok(())
//     }

//     async fn contact_message(
//         &self,
//         scratch: &bumpalo::Bump,
//         contact: &crate::companion::handler::storage::Contact,
//         packet: (&Packet<'_>, PacketStatus),
//         text: &TextMessageData<'_>,
//         handler: &SimpleCompanion<impl CompanionLayer + Send>,
//     ) -> CompanionResult<()>
//     where
//         Self: Send + Sized,
//     {
//         Ok(())
//     }

//     async fn packet(
//         &self,
//         scratch: &bumpalo::Bump,
//         packet: (&Packet<'_>, PacketStatus),
//         handler: &SimpleCompanion<impl CompanionLayer + Send>,
//     ) -> CompanionResult<()>
//     where
//         Self: Send + Sized,
//     {
//         Ok(())
//     }
// }
