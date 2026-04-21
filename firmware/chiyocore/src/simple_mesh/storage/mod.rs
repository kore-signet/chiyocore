use alloc::sync::Arc;
use chiyo_hal::{embassy_sync, esp_sync, storage::ChiyoFilesystem};
use embassy_sync::rwlock::RwLock;

pub mod channel;
pub mod contact;
pub mod message_log;
pub mod packet_log;
pub mod shared_key_cache;

use crate::simple_mesh::storage::{channel::ChannelStorage, contact::ContactStorage};

#[derive(Clone)]
pub struct MeshStorage {
    pub fs: ChiyoFilesystem,
    pub contacts: Arc<RwLock<esp_sync::RawMutex, ContactStorage>>,
    pub channels: Arc<RwLock<esp_sync::RawMutex, ChannelStorage>>,
}

impl MeshStorage {
    pub async fn new(fs: &ChiyoFilesystem) -> Self {
        MeshStorage {
            fs: fs.clone(),
            contacts: Arc::new(RwLock::new(ContactStorage::new(fs.clone()).await)),
            channels: Arc::new(RwLock::new(ChannelStorage::new(fs.clone()).await.unwrap())),
        }
    }
}
