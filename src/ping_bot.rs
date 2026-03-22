pub struct PingBot;

use lora_phy::mod_params::PacketStatus;
use meshcore::{Packet, payloads::TextMessageData};

use crate::companion::handler::{
    BotLayer,
    simple_companion::{CompanionError, CompanionResult, SimpleCompanion},
    storage::Channel,
};

impl BotLayer for PingBot {
    async fn channel_message(
        &self,
        scratch: &bumpalo::Bump,
        channel: &Channel,
        (packet, packet_status): (&Packet<'_>, PacketStatus),
        text: &TextMessageData<'_>,
        handler: &SimpleCompanion<PingBot>,
    ) -> CompanionResult<()> {
        if !(channel.name == "#test" || channel.name == "#emitestcorner") {
            return Ok(());
        }

        let message = core::str::from_utf8(&text.message)
            .map_err(|e| CompanionError::DecodeFailure(e.into()))?;
        let Some((username, msg)) = message.split_once(':') else {
            return Ok(());
        };

        if msg.trim() == "!ping" {
            let ping_message = bumpalo::format!(in scratch, "cafe / chiyobot 🌃☕: pong @[{}] ＼(≧▽≦)／\npath: {:?}\nsnr {} db | rssi {} dBm", username, packet.path, packet_status.snr, packet_status.rssi);

            handler
                .send_to_channel(scratch, channel.idx, &ping_message, None)
                .await?;
        }

        Ok(())
    }
}
