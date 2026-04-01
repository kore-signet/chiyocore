#![cfg_attr(not(feature = "std"), no_std)]
#[cfg(not(feature = "std"))]
extern crate alloc;
#[cfg(not(feature = "std"))]
use alloc::{borrow::Cow, string::String, vec::Vec};
use smol_str::SmolStr;
#[cfg(feature = "std")]
use std::borrow::Cow;

#[cfg(feature = "codegen")]
pub mod codegen;

use litemap::LiteMap;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChiyocoreConfig {
    pub firmware: FirmwareConfig,
    pub nodes: Cow<'static, [Node]>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FirmwareConfig {
    pub stack_size: usize,
    pub config: LiteMap<String, String>,
    #[serde(default)]
    pub default_channels: Vec<String>
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Node {
    pub id: Cow<'static, str>,
    pub layers: Cow<'static, [LayerConfig]>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum LayerConfig {
    PingBot(PingBotConfig),
    Companion(CompanionConfig),
}

impl LayerConfig {
    pub fn kind(&self) -> &'static str {
        match self {
            LayerConfig::PingBot(_) => "chiyocore::ping_bot::PingBot",
            LayerConfig::Companion(_) => "chiyocore_companion::companionv2::Companion",
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CompanionConfig {
    pub id: Cow<'static, str>,
    pub tcp_port: u16,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PingBotConfig {
    pub name: Cow<'static, str>,
    pub channels: Cow<'static, [SmolStr]>,
}
