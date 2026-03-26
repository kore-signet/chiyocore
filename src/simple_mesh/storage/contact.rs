use alloc::{string::String, sync::Arc, vec::Vec};
use base64::{Engine, prelude::BASE64_URL_SAFE};
use meshcore::{Path, identity::ForeignIdentity, io::SliceWriter, payloads::AppdataFlags};
use serde::{Deserialize, Serialize};

use crate::{
    EspMutex, FirmwareResult,
    companion_protocol::protocol::{CompanionSer, NullPaddedSlice, ResponseCodes},
    storage::{ActiveFilesystem, FS_SIZE, SimpleFileDb},
};

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
    BASE64_URL_SAFE.encode_slice(key, &mut s);
    s[CONTACT_KEY_SIZE] = 0x00;
    heapless::CString::from_bytes_with_nul(&s).unwrap()
}

pub struct ContactStorage {
    pub hot_cache: Vec<CachedContact>,
    pub fs: SimpleFileDb<FS_SIZE>,
}

impl ContactStorage {
    pub async fn new(fs: Arc<EspMutex<ActiveFilesystem<FS_SIZE>>>) -> ContactStorage {
        let fs = SimpleFileDb::new(fs, littlefs2::path!("/contacts/")).await;
        // fs.
        let mut cache = fs
            .entries::<Contact, CachedContact>(CachedContact::from_full)
            .await
            .unwrap();
        cache.sort_unstable_by_key(|k| k.key);

        ContactStorage {
            hot_cache: cache,
            fs,
        }
    }

    pub fn fast_get(&self, key: &[u8]) -> Option<&CachedContact> {
        self.find_idx(key).map(|v| &self.hot_cache[v]) // TODO: this does two indexes currently, should fix
    }

    pub async fn full_get(&self, key: [u8; 32]) -> FirmwareResult<Option<Contact>> {
        let fs_key = contact_b64_key(&key);
        self.fs.get::<Contact>(&fs_key).await
    }

    pub async fn insert(&mut self, contact: Contact) -> FirmwareResult<()> {
        let fs_key = contact_b64_key(&contact.key);
        self.fs.insert(&fs_key, &contact).await?;

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
        self.fs.delete(&contact_b64_key(&key)).await;
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

impl CompanionSer for Contact {
    fn ser_size(&self) -> usize {
        1 // packet_ty
        + 32 // pk
        + 1 // adv_ty
        + 1 // flags
        + 1 // path_to_len 
        + 64 // path_to
        + 32 // name
        + 4 // last_heard
        + 4 // latitude
        + 4 // longitude
        + 4 // last_mod 
    }

    fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
        let mut out = SliceWriter::new(out);

        out.write_u8(ResponseCodes::Contact as u8);
        out.write_slice(&self.key);
        let flags = AppdataFlags::from_bits(self.flags).unwrap();
        let adv_ty = if flags.contains(AppdataFlags::IS_CHAT_NODE) {
            1
        } else if flags.contains(AppdataFlags::IS_REPEATER) {
            2
        } else if flags.contains(AppdataFlags::IS_ROOM_SERVER) {
            3
        } else {
            0
        };

        out.write_u8(adv_ty);
        out.write_u8(flags.bits());
        if let Some(path) = self.path_to.as_ref() {
            out.write_u8(path.path_len_header().into_bytes()[0]);
            NullPaddedSlice::<64>::from(path.raw_bytes()).encode_to(&mut out);
        } else {
            // flood
            out.write_u8(0xFF);
            NullPaddedSlice::<64>(&[]).encode_to(&mut out);
        }

        NullPaddedSlice::<32>::from(self.name.as_str()).encode_to(&mut out);
        out.write_u32_le(self.last_heard);
        out.write_u32_le(self.latitude);
        out.write_u32_le(self.longitude);
        out.write_u32_le(0);

        out.finish()
    }
}
