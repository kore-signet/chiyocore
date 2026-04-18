use std::collections::HashMap;

use cargo_toml::DependencyDetail;
use litemap::LiteMap;
use meshcore::payloads::AdvertType;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FullConfig {
    pub firmware: FirmwareConfig,
    pub chiyocore: ChiyocoreBaseConf,
    pub stackup: Stackup,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChiyocoreBaseConf {
    pub config: LiteMap<String, String>,
    #[serde(default)]
    pub default_channels: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FirmwareConfig {
    pub stack_size: usize,
}

pub type Stackup = HashMap<String, NodeConfig>;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NodeConfig {
    pub name: String,
    pub role: AdvertType,
    pub id: String,
    pub layers: HashMap<String, LayerConfig>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LayerConfig {
    #[serde(default)]
    pub deps: HashMap<String, DependencyDetail>,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub values: HashMap<String, serde_json::Value>,
}

// pub struct NodeConfig {

// pub tcp_port: u16
// }
