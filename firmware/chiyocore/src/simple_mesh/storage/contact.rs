use alloc::{string::String, vec::Vec};
use base64::{Engine, prelude::BASE64_URL_SAFE};
use chiyo_hal::meshcore::{Path, identity::ForeignIdentity};
use chiyo_hal::storage::{ChiyoFilesystem, DirKey};
use serde::{Deserialize, Serialize};

use crate::FirmwareResult;

#[derive(Serialize, Deserialize)]
pub struct Contact {
    pub key: [u8; 32],
    pub name: String,
    pub path_to: Option<Path<'static>>,
    pub flags: u8,
    pub latitude: u32,
    pub longitude: u32,
    pub last_heard: u32,
}

/// Limited contact information for contacts, kept in a in-memory cache instead of on flash.
#[derive(Clone)]
pub struct CachedContact {
    pub key: [u8; 32],
    pub path: Option<Path<'static>>,
}

impl CachedContact {
    pub fn from_full(contact: Contact) -> Self {
        CachedContact {
            key: contact.key,
            path: contact.path_to,
        }
    }

    pub fn as_identity(&self) -> ForeignIdentity {
        ForeignIdentity::new(self.key)
    }
}

const CONTACT_KEY_SIZE: usize = base64::encoded_len(32, true).unwrap();

fn contact_b64_key(key: &[u8; 32]) -> heapless::CString<{ CONTACT_KEY_SIZE + 1 }> {
    let mut s = [0u8; { CONTACT_KEY_SIZE + 1 }];
    let _ = BASE64_URL_SAFE.encode_slice(key, &mut s);
    s[CONTACT_KEY_SIZE] = 0x00;
    heapless::CString::from_bytes_with_nul(&s).unwrap()
}

pub const CONTACT_DIR: DirKey = DirKey::const_new(b"contacts");

/// Flash-backed storage for contacts. If you only need a contact's key and the path to reach them, prefer using the fast_get() method and hot_cache, since these do not involve costly flash reads.
pub struct ContactStorage {
    pub hot_cache: Vec<CachedContact>,
    pub fs: ChiyoFilesystem,
}

impl ContactStorage {
    pub async fn new(fs: ChiyoFilesystem) -> ContactStorage {
        // let fs = SimpleFileDb::new(fs, littlefs2::path!("/contacts/")).await;
        // fs.
        // let mut cache = fs
        //     .entries::<Contact, CachedContact>(CachedContact::from_full)
        //     .await
        //     .unwrap();

        let entries = if let Some(entries) = fs.directory_entries(CONTACT_DIR).await.unwrap() {
            let mut cache: Vec<CachedContact> = Vec::with_capacity(entries.len());
            let mut entries = entries.reader(&fs);

            while let Some(entry) = entries.next_file().await {
                let entry = entry.unwrap();
                let entry: Contact = postcard::from_bytes(entry).unwrap();

                cache.push(CachedContact::from_full(entry));
            }

            cache.sort_unstable_by_key(|k| k.key);
            cache
        } else {
            Vec::new()
        };

        ContactStorage {
            hot_cache: entries,
            fs,
        }
    }

    pub fn fast_get(&self, key: &[u8]) -> Option<&CachedContact> {
        self.find_idx(key).map(|v| &self.hot_cache[v]) // TODO: this does two indexes currently, should fix
    }

    pub async fn full_get(&self, key: [u8; 32]) -> FirmwareResult<Option<Contact>> {
        self.fs
            .get_deser::<512, Contact>(CONTACT_DIR.file(&key))
            .await
    }

    pub async fn insert(&mut self, contact: Contact) -> FirmwareResult<()> {
        // let fs_key = contact_b64_key(&contact.key);
        self.fs
            .set(
                CONTACT_DIR,
                CONTACT_DIR.file(&contact.key),
                &postcard::to_vec::<_, 512>(&contact)?,
            )
            .await?;

        let contact = CachedContact {
            key: contact.key,
            path: contact.path_to,
        };

        match self
            .hot_cache
            .binary_search_by(|probe| probe.key.cmp(&contact.key))
        {
            Ok(idx) => self.hot_cache[idx] = contact,
            Err(idx) => self.hot_cache.insert(idx, contact),
        };

        Ok(())
    }

    pub async fn delete(&mut self, key: [u8; 32]) {
        let Some(idx) = self.find_idx(&key) else {
            return;
        };
        let _cached = self.hot_cache.remove(idx);
        self.fs.delete(CONTACT_DIR.file(&key)).await;
        // self.fs.delete(&contact_b64_key(&key)).await;
    }

    pub fn find_idx(&self, prefix: &[u8]) -> Option<usize> {
        match self
            .hot_cache
            .binary_search_by(|probe| probe.key[..].cmp(prefix))
        {
            Ok(idx) => Some(idx),
            Err(idx) => {
                let idx = core::cmp::min(self.hot_cache.len().saturating_sub(1), idx);
                let v = &self.hot_cache.get(idx)?;
                if v.key.starts_with(prefix) {
                    Some(idx)
                } else {
                    None
                }
            }
        }
    }
}
