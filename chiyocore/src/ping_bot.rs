use alloc::{borrow::Cow, format, sync::Arc, vec::Vec};
use chiyo_hal::{EspMutex, embassy_sync, esp_hal, esp_sync};
// use chiyocore_config::PingBotConfig;
use esp_hal::rtc_cntl::Rtc;
use meshcore::payloads::TextMessageData;
use serde::{Deserialize, Serialize};
use smol_str::{SmolStr, ToSmolStr};

use crate::{CompanionResult, builder::BuildChiyocoreLayer, simple_mesh::SimpleMeshLayer};

#[derive(Serialize, Deserialize)]
pub struct PingBotConfig {
    pub name: Cow<'static, str>,
    pub channels: Cow<'static, [SmolStr]>
}

pub struct PingBot {
    pub rtc: Arc<Rtc<'static>>,
    pub name: SmolStr,
    pub channels: Vec<SmolStr>,
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
        if !self.channels.contains(&channel.name) {
            return Ok(());
        }

        let msg = message.as_utf8()?.trim_end_matches('\0').trim();
        let Some((user, msg)) = msg.split_once(':') else {
            return Ok(());
        };

        if msg.trim() == "!ping" {
            let message = format!(
                "{}: pong @[{user}] ＼(≧▽≦)／\npath: {:?}",
                self.name, packet.path
            );
            let message = TextMessageData::plaintext(
                (self.rtc.current_time_us() / 1_000_000) as u32,
                message.as_bytes(),
            );
            let delay = crate::timing::rx_retransmit_delay(packet) * 2;
            mesh.send_channel_message(&channel.as_keys(), &message, Some(delay))
                .await?;
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

impl BuildChiyocoreLayer for PingBot {
    type Input = (&'static str, PingBotConfig);
    type Output = Arc<EspMutex<PingBot>>;

    fn build<T: 'static>(
        _spawner: &embassy_executor::Spawner,
        chiyocore: &crate::builder::Chiyocore<T, crate::builder::ChiyocoreSetupData>,
        _mesh: &Arc<
            embassy_sync::rwlock::RwLock<esp_sync::RawMutex, crate::simple_mesh::SimpleMesh>,
        >,
        cfg: &Self::Input,
    ) -> impl Future<Output = Self::Output> {
        let (_, cfg) = cfg;
        
        core::future::ready(Arc::new(EspMutex::new(PingBot {
            rtc: Arc::clone(chiyocore.rtc()),
            name: cfg.name.to_smolstr(),
            channels: cfg.channels.clone().into_owned(),
        })))
    }
}
