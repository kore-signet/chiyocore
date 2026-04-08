use alloc::sync::Arc;
use chiyo_hal::{embassy_sync, esp_sync};
use ed25519_compact::x25519::{self};
use embassy_sync::rwlock::RwLock;
use litemap::LiteMap;
use meshcore::identity::{ForeignIdentity, LocalIdentity};

use crate::psram_vec::PSRAMVec;

/// Cache for shared keys between a specific x25519 identity and other contacts.
#[derive(Clone)]
pub struct SharedKeyCache {
    // TODO: for now, this grows unbounded. we really don't want that for long-running nodes; impl eviction
    // TODO: is rwlock > mutex?
    self_key: x25519::SecretKey,
    cache: Arc<
        RwLock<esp_sync::RawMutex, LiteMap<[u8; 32], [u8; 32], PSRAMVec<([u8; 32], [u8; 32])>>>,
    >,
}

impl SharedKeyCache {
    pub fn new(identity: &LocalIdentity) -> Self {
        SharedKeyCache {
            self_key: identity.encryption_keys.sk.clone(),
            cache: Arc::new(RwLock::new(LiteMap::new())),
        }
    }

    pub async fn get_key(&self, contact: &ForeignIdentity) -> [u8; 32] {
        if let Some(k) = self.cache.read().await.get(&*contact.verify_key) {
            return *k;
        };

        let shared_key = contact.encrypt_key().dh(&self.self_key).unwrap(); // todo: no unwrapping

        self.cache
            .write()
            .await
            .insert(*contact.verify_key, *shared_key);

        *shared_key
    }
}
