#![no_std]
#![feature(allocator_api)]
#![feature(impl_trait_in_bindings)]

pub mod ntp;

pub mod builder;
pub mod crypto;
pub mod lora;
pub mod partition_table;
pub mod ping_bot;
pub mod psram_vec;
pub mod simple_mesh;
pub mod timing;
pub mod wifi;

use alloc::vec::Vec;
pub use chiyo_hal::meshcore;
use chiyo_hal::{esp_hal, storage::DirKey};
pub use lora_phy::mod_params::PacketStatus;
use thingbuf::{Recycle, recycling::WithCapacity};

use core::fmt::Debug;

// use embassy_time::Duration;
// use littlefs_rust::RamStorage;
// use lora_phy::mod_params::PacketStatus;
use meshcore::{DecodeError, Packet};

use crate::simple_mesh::SimpleMeshLayer;

// pub mod base_handler;
// pub mod companion;
// pub mod contacts_manager;
extern crate alloc;

// pub type LittleFs = Arc<EspMutex<littlefs2::>;

pub type BumpaloVec<'a, T> = bumpalo::collections::Vec<'a, T>;

pub use chiyo_hal::{FirmwareError, FirmwareResult};
pub use static_cell;

#[derive(Clone)]
pub struct DataWithSnr(pub Vec<u8>, pub PacketStatus);

impl Default for DataWithSnr {
    fn default() -> Self {
        Self(Default::default(), PacketStatus { rssi: 0, snr: 0 })
    }
}

impl Recycle<DataWithSnr> for WithCapacity {
    fn new_element(&self) -> DataWithSnr {
        DataWithSnr(self.new_element(), PacketStatus { rssi: 0, snr: 0 })
    }

    fn recycle(&self, element: &mut DataWithSnr) {
        self.recycle(&mut element.0);
        element.1 = PacketStatus { rssi: 0, snr: 0 };
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

#[derive(Debug, defmt::Format)]
pub enum CompanionError {
    NoKnownChannel,
    NoKnownContact,
    DecryptFailure,
    VerifyFailure,
    AesFailure(esp_hal::aes::Error),
    DecodeFailure(#[defmt(Debug2Format)] DecodeError),
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

pub const GLOBAL_VARS_DIR: DirKey = DirKey::const_new(b"chiyoglobalvars");
