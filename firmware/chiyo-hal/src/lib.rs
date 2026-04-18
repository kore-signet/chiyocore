#![no_std]

// this crate is just a helper to centralize all the bundle of deps from embassy & esp-hal

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

pub type EspMutex<T> = embassy_sync::mutex::Mutex<esp_sync::RawMutex, T>;
pub type SyncEspMutex<T> = esp_sync::NonReentrantMutex<T>;
pub type EspWatch<T, const N: usize> = embassy_sync::watch::Watch<esp_sync::RawMutex, T, N>;
pub type EspRwLock<T> = embassy_sync::rwlock::RwLock<esp_sync::RawMutex, T>;
