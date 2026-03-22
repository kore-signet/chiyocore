use core::ops::Deref;

use crate::companion::protocol::CompanionSer;
use crate::companion::protocol::NullPaddedSlice;
use crate::{
    EspMutex, FirmwareResult,
    companion::protocol::ResponseCodes,
    storage::{ActiveFilesystem, Cacheable, CachedFileDb, CachedVersion, FS_SIZE},
};
use alloc::{string::String, sync::Arc};
use litemap::LiteMap;
use meshcore::Path;
use meshcore::identity::LocalIdentity;
use meshcore::{identity::ForeignIdentity, io::SliceWriter, payloads::AppdataFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use smallvec::SmallVec;
use smol_str::SmolStr;

#[derive(Serialize, Deserialize)]
pub struct StoredIdentity {
    pub keys: LocalIdentity,
    pub name: String,
}

impl Deref for StoredIdentity {
    type Target = LocalIdentity;

    fn deref(&self) -> &Self::Target {
        &self.keys
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct CompanionConfig {
    pub wifi_ssid: String,
    pub wifi_password: String,
}

impl CompanionConfig {
    pub fn as_vars(&self) -> SmallVec<[(&str, &str); 8]> {
        smallvec::smallvec![
            ("wifi_ssid", self.wifi_ssid.as_str()),
            ("wifi_password", self.wifi_password.as_str())
        ]
    }

    pub fn set(&mut self, key: &str, val: &str) {
        match key {
            "wifi_ssid" => self.wifi_ssid = val.into(),
            "wifi_password" => self.wifi_password = val.into(),
            _ => {}
        }
    }
}

pub struct CachedContact {
    pub path_to: Option<Path<'static>>,
    pub ident: ForeignIdentity,
}

impl Deref for CachedContact {
    type Target = ForeignIdentity;

    fn deref(&self) -> &Self::Target {
        &self.ident
    }
}

impl CachedVersion<[u8; 32]> for CachedContact {
    fn key(&self) -> &[u8; 32] {
        &self.ident.verify_key
    }

    fn size(&self) -> usize {
        self.path_to.as_ref().map_or(1, |v| v.raw_bytes().len())
            + core::mem::size_of::<CachedContact>()
    }
}

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

impl Cacheable for Contact {
    type Cached = CachedContact;
    type Key = [u8; 32];

    fn key(&self) -> &Self::Key {
        &self.key
    }

    fn as_cached(&self) -> Self::Cached {
        CachedContact {
            // name: self.name.clone(),
            path_to: self.path_to.clone(),
            ident: ForeignIdentity::new(self.key),
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

#[derive(Serialize, Deserialize, Clone)]
pub struct Channel {
    pub name: SmolStr,
    pub key: [u8; 16],
    pub hash: u8,
    pub idx: u8,
}

impl Channel {
    pub fn public() -> Channel {
        Channel {
            name: "public".into(),
            key: hex_lit::hex!("8b3387e9c5cdea6ac9e5edbaa115cd72"),
            hash: 0x11,
            idx: 0,
        }
    }

    pub fn from_name(name: &str, idx: u8) -> Channel {
        let master_key: [u8; 16] = Sha256::digest(name)[0..16].try_into().unwrap();
        let digest_of_key = Sha256::digest(master_key);
        Channel {
            name: name.into(),
            key: master_key,
            hash: digest_of_key[0],
            idx,
        }
    }
}

impl Cacheable for Channel {
    type Key = u8; // IDX
    type Cached = Channel;

    fn key(&self) -> &Self::Key {
        &self.idx
    }

    fn as_cached(&self) -> Self::Cached {
        self.clone()
    }
}

impl CachedVersion<u8> for Channel {
    fn key(&self) -> &u8 {
        &self.idx
    }

    fn size(&self) -> usize {
        core::mem::size_of::<Channel>()
            + if self.name.is_heap_allocated() {
                self.name.len()
            } else {
                0
            }
    }
}

pub struct ChannelManager {
    pub by_name: LiteMap<SmolStr, u8>,
    pub by_hash: LiteMap<u8, u8>,
    pub db: CachedFileDb<FS_SIZE, Channel>,
}

static FS_CHANNEL_PREFIX: &littlefs2::path::Path = littlefs2::path!("/channels/");

impl ChannelManager {
    pub async fn new(storage: Arc<EspMutex<ActiveFilesystem<FS_SIZE>>>) -> ChannelManager {
        let db = CachedFileDb::<FS_SIZE, Channel>::init(storage, FS_CHANNEL_PREFIX).await;

        let by_name = LiteMap::from_iter(
            db.cache
                .iter()
                .map(|channel| (channel.name.clone(), channel.idx)),
        );
        let by_hash =
            LiteMap::from_iter(db.cache.iter().map(|channel| (channel.hash, channel.idx)));

        ChannelManager {
            by_hash,
            by_name,
            db,
        }
    }

    pub async fn register(&mut self, entry: &Channel) -> FirmwareResult<()> {
        self.db.insert(entry).await
    }

    pub fn by_hash(&self, hash: u8) -> Option<&Channel> {
        self.by_hash
            .get(&hash)
            .and_then(|v| self.db.get_cached(&[*v]))
    }

    pub fn by_name(&self, name: &str) -> Option<&Channel> {
        self.by_name
            .get(name)
            .and_then(|v| self.db.get_cached(&[*v]))
    }
}
