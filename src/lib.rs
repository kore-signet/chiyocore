#![no_std]

pub mod ntp;

pub mod boards;
pub mod companion_protocol;
pub mod companionv2;
pub mod crypto;
pub mod lora;
pub mod partition_table;
pub mod ping_bot;
pub mod simple_mesh;
pub mod storage;
pub mod wifi;

use core::fmt::Debug;

// use embassy_time::Duration;
// use littlefs_rust::RamStorage;
use lora_phy::mod_params::PacketStatus;
use meshcore::{DecodeError, Packet};

use crate::simple_mesh::SimpleMeshLayer;

// pub mod base_handler;
// pub mod companion;
// pub mod contacts_manager;
extern crate alloc;

// pub type LittleFs = Arc<EspMutex<littlefs2::>;

pub type EspMutex<T> = embassy_sync::mutex::Mutex<esp_sync::RawMutex, T>;
pub type SyncEspMutex<T> = esp_sync::NonReentrantMutex<T>;
pub type BumpaloVec<'a, T> = bumpalo::collections::Vec<'a, T>;

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
        layers: &mut impl SimpleMeshLayer,
    ) -> impl core::future::Future<Output = Result<(), Self::Error>>;
}

#[macro_export]
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write($val);
        x
    }};
}

#[derive(Debug)]
pub enum CompanionError {
    NoKnownChannel,
    NoKnownContact,
    DecryptFailure,
    VerifyFailure,
    AesFailure(esp_hal::aes::Error),
    DecodeFailure(DecodeError),
    Firmware(FirmwareError),
}

impl From<FirmwareError> for CompanionError {
    fn from(value: FirmwareError) -> Self {
        Self::Firmware(value)
    }
}

impl From<DecodeError> for CompanionError {
    fn from(value: DecodeError) -> Self {
        Self::DecodeFailure(value)
    }
}

impl From<esp_hal::aes::Error> for CompanionError {
    fn from(value: esp_hal::aes::Error) -> Self {
        Self::AesFailure(value)
    }
}

pub type CompanionResult<T> = Result<T, CompanionError>;
