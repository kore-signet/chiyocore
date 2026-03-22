#![no_std]

pub mod ntp;

pub mod boards;
pub mod crypto;
pub mod lora;
pub mod partition_table;
pub mod ping_bot;
pub mod storage;

use core::fmt::Debug;

// use embassy_time::Duration;
// use littlefs_rust::RamStorage;
use lora_phy::mod_params::PacketStatus;
use meshcore::Packet;

// pub mod base_handler;
pub mod companion;
// pub mod contacts_manager;
extern crate alloc;

// pub type LittleFs = Arc<EspMutex<littlefs2::>;

pub type EspMutex<T> = embassy_sync::mutex::Mutex<esp_sync::RawMutex, T>;
pub type SyncEspMutex<T> = esp_sync::NonReentrantMutex<T>;

#[derive(Debug)]
pub enum FirmwareError {
    // #[error("storage: {0:?}")]
    Storage(littlefs2::io::Error),
    // #[error("radio: {0:?}")]
    LoRa(lora_phy::mod_params::RadioError),
    Postcard(postcard::Error),
}

impl From<littlefs2::io::Error> for FirmwareError {
    fn from(value: littlefs2::io::Error) -> Self {
        FirmwareError::Storage(value)
    }
}

impl From<lora_phy::mod_params::RadioError> for FirmwareError {
    fn from(value: lora_phy::mod_params::RadioError) -> Self {
        FirmwareError::LoRa(value)
    }
}

impl From<postcard::Error> for FirmwareError {
    fn from(value: postcard::Error) -> Self {
        FirmwareError::Postcard(value)
    }
}

pub type FirmwareResult<T> = Result<T, FirmwareError>;

#[derive(Clone)]
pub struct DataWithSnr(pub heapless::Vec<u8, 256>, pub PacketStatus);

impl Default for DataWithSnr {
    fn default() -> Self {
        Self(Default::default(), PacketStatus { rssi: 0, snr: 0 })
    }
}

pub trait MeshcoreHandler {
    type Error: Debug;

    fn packet(
        &mut self,
        packet: &Packet<'_>,
        packet_status: PacketStatus,
        bytes: &[u8],
    ) -> impl core::future::Future<Output = Result<(), Self::Error>>;
}
