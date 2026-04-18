use alloc::{sync::Arc, vec::Vec};
use chiyo_hal::EspMutex;
use heapless::CString;
use litemap::LiteMap;
use meshcore::crypto::ChannelKeys;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::{
    CompanionResult, FirmwareResult,
    storage::{ActiveFilesystem, FS_SIZE, SimpleFileDb},
};

/// A stored channel, with an assigned name and index/slot.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Channel {
    pub name: SmolStr,
    pub key: [u8; 16],
    pub hash: u8,
    pub idx: u8,
}

impl Channel {
    pub fn as_keys(&self) -> ChannelKeys {
        ChannelKeys {
            hash: self.hash,
            secret: self.key,
        }
    }

    pub fn from_keys(idx: u8, name: impl Into<SmolStr>, keys: ChannelKeys) -> Channel {
        Channel {
            name: name.into(),
            key: keys.secret,
            hash: keys.hash,
            idx,
        }
    }
}

/// Flash-backed storage for channels. Channels are stored by three keys:
/// - hash -> first byte of the sha256 digest of the channel's secret key
/// - name -> registered name for the channel
/// - idx/slot -> index/slot the channel will be stored in, usually just increases monotonically
pub struct ChannelStorage {
    by_hash: LiteMap<u8, u8>,
    by_name: LiteMap<SmolStr, u8>,
    cache: Vec<Channel>,
    db: SimpleFileDb<FS_SIZE>,
}

impl ChannelStorage {
    pub async fn new(
        fs: Arc<EspMutex<ActiveFilesystem<FS_SIZE>>>,
    ) -> CompanionResult<ChannelStorage> {
        let db = SimpleFileDb::new(fs, littlefs2::path!("/channels/")).await;

        let entries = db.entries::<Channel, Channel>(|c| c).await?;
        let by_hash = LiteMap::from_iter(entries.iter().map(|v| (v.hash, v.idx)));
        let by_name = LiteMap::from_iter(entries.iter().map(|v| (v.name.clone(), v.idx)));

        Ok(ChannelStorage {
            by_hash,
            by_name,
            cache: entries,
            db,
        })
    }

    pub fn get(&self, idx: u8) -> Option<&Channel> {
        self.cache
            .binary_search_by_key(&idx, |v| v.idx)
            .ok()
            .map(|v| &self.cache[v])
    }

    pub fn get_by_hash(&self, hash: u8) -> Option<&Channel> {
        self.by_hash.get(&hash).and_then(|i| self.get(*i))
    }

    pub fn get_by_name(&self, name: &str) -> Option<&Channel> {
        self.by_name.get(name).and_then(|i| self.get(*i))
    }

    pub async fn insert(&mut self, channel: Channel) -> FirmwareResult<()> {
        let mut key = CString::<4>::new();
        let _ = key.extend_from_bytes(itoa::Buffer::new().format(channel.idx).as_bytes());

        self.db.insert(&key, &channel).await?;

        self.by_hash.insert(channel.hash, channel.idx);
        self.by_name.insert(channel.name.clone(), channel.idx);

        match self.cache.binary_search_by_key(&channel.idx, |v| v.idx) {
            Ok(v) => self.cache[v] = channel,
            Err(v) => self.cache.insert(v, channel),
        }

        Ok(())
    }
}
