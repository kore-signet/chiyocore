#![no_std]

// this crate is just a helper to centralize all the bundle of deps from embassy & esp-hal

use core::alloc::Layout;

use alloc::boxed::Box;
pub use esp_alloc;
pub use esp_hal;
pub use esp_println;
pub use esp_radio;
pub use esp_rtos;
pub use esp_storage;
pub use esp_sync;

pub use embassy_embedded_hal;
pub use embassy_executor;
pub use embassy_futures;
pub use embassy_net;
pub use embassy_sync;
pub use embassy_time;
pub use embedded_io_async;
pub use embedded_storage;
pub use esp_backtrace;
pub use meshcore;
pub use postcard;
pub use static_cell;

pub mod storage;

extern crate alloc;

pub type EspMutex<T> = embassy_sync::mutex::Mutex<esp_sync::RawMutex, T>;
pub type SyncEspMutex<T> = esp_sync::NonReentrantMutex<T>;
pub type EspWatch<T, const N: usize> = embassy_sync::watch::Watch<esp_sync::RawMutex, T, N>;
pub type EspRwLock<T> = embassy_sync::rwlock::RwLock<esp_sync::RawMutex, T>;

#[derive(Debug, defmt::Format)]
pub enum FirmwareError {
    LoRa(#[defmt(Debug2Format)] lora_phy::mod_params::RadioError),
    Postcard(#[defmt(Debug2Format)] postcard::Error),
    SequentialStorage(
        #[defmt(Debug2Format)] sequential_storage::Error<esp_storage::FlashStorageError>,
    ),
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

impl From<sequential_storage::Error<esp_storage::FlashStorageError>> for FirmwareError {
    fn from(value: sequential_storage::Error<esp_storage::FlashStorageError>) -> Self {
        FirmwareError::SequentialStorage(value)
    }
}

impl From<FirmwareError> for meshcore_companion_protocol::responses::Err {
    fn from(val: FirmwareError) -> Self {
        meshcore_companion_protocol::responses::Err { code: None }
    }
}

pub type FirmwareResult<T> = Result<T, FirmwareError>;

#[macro_export]
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: $crate::static_cell::StaticCell<$t> =
            $crate::static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write($val);
        x
    }};
}

pub fn box_array<T: Copy, const SIZE: usize>(val: T) -> Box<[T; SIZE]> {
    unsafe {
        let ptr = alloc::alloc::alloc(Layout::new::<[T; SIZE]>()) as *mut T;
        if core::mem::size_of::<T>() > 0 {
            for i in 0..SIZE {
                ptr.add(i).write(val);
            }
        }
        Box::from_raw(ptr as *mut [T; SIZE])
    }
}
