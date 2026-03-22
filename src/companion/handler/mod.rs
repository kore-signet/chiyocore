use crate::companion::handler::{
    simple_companion::{CompanionResult, SimpleCompanion},
    storage::Channel,
};
use lora_phy::mod_params::PacketStatus;
use meshcore::{Packet, payloads::TextMessageData};

pub mod companion_handler;
pub mod message_log;
pub mod simple_companion;
pub mod storage;

pub trait BotLayer {
    fn channel_message(
        &self,
        scratch: &bumpalo::Bump,
        channel: &Channel,
        packet: (&Packet<'_>, PacketStatus),
        text: &TextMessageData,
        handler: &SimpleCompanion<Self>,
    ) -> impl Future<Output = CompanionResult<()>>
    where
        Self: Send + Sized;
}
