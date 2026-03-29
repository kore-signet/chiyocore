use alloc::sync::Arc;
use embassy_sync::rwlock::RwLock;

pub mod channel;
pub mod contact;
pub mod shared_key_cache;

use crate::{
    EspMutex,
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
