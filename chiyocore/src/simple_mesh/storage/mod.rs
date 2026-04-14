use alloc::sync::Arc;
use chiyo_hal::{EspMutex, embassy_sync, esp_sync};
use embassy_sync::rwlock::RwLock;

pub mod channel;
pub mod contact;
pub mod message_log;
pub mod packet_log;
pub mod shared_key_cache;

use crate::{
    simple_mesh::storage::{channel::ChannelStorage, contact::ContactStorage},
    storage::{ActiveFilesystem, FS_SIZE},
};

#[derive(Clone)]
pub struct MeshStorage {
    pub contacts: Arc<RwLock<esp_sync::RawMutex, ContactStorage>>,
    pub channels: Arc<RwLock<esp_sync::RawMutex, ChannelStorage>>,
}

impl MeshStorage {
    pub async fn new(fs: &Arc<EspMutex<ActiveFilesystem<FS_SIZE>>>) -> Self {
        MeshStorage {
            contacts: Arc::new(RwLock::new(ContactStorage::new(Arc::clone(fs)).await)),
            channels: Arc::new(RwLock::new(
                ChannelStorage::new(Arc::clone(fs)).await.unwrap(),
            )),
        }
    }
}
