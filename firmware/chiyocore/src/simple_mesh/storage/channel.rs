use alloc::vec::Vec;
use chiyo_hal::meshcore::crypto::ChannelKeys;
use chiyo_hal::{
    FirmwareError,
    storage::{ChiyoFilesystem, DirKey},
};
use litemap::LiteMap;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::{CompanionResult, FirmwareResult};

/// A stored channel, with an assigned name and index/slot.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
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

pub const CHANNEL_KEY: DirKey = DirKey::const_new(b"channels");

/// Flash-backed storage for channels. Channels are stored by three keys:
/// - hash -> first byte of the sha256 digest of the channel's secret key
/// - name -> registered name for the channel
/// - idx/slot -> index/slot the channel will be stored in, usually just increases monotonically
pub struct ChannelStorage {
    by_hash: LiteMap<u8, u8>,
    by_name: LiteMap<SmolStr, u8>,
    cache: Vec<Channel>,
    db: ChiyoFilesystem,
}

impl ChannelStorage {
    pub async fn new(fs: ChiyoFilesystem) -> CompanionResult<ChannelStorage> {
        // let db = SimpleFileDb::new(fs, littlefs2::path!("/channels/")).await;

        let entry_cache = if let Some(entries) = fs.directory_entries(CHANNEL_KEY).await? {
            let mut entry_cache: Vec<Channel> = Vec::with_capacity(entries.len());
            let mut entry_reader = entries.reader(&fs);
            while let Some(entry) = entry_reader.next_file().await {
                let entry = entry.unwrap();
                entry_cache.push(postcard::from_bytes(entry).map_err(FirmwareError::Postcard)?);
            }

            entry_cache.sort_unstable_by_key(|v| v.idx);
            entry_cache
        } else {
            Vec::new()
        };

        let by_hash = LiteMap::from_iter(entry_cache.iter().map(|v| (v.hash, v.idx)));
        let by_name = LiteMap::from_iter(entry_cache.iter().map(|v| (v.name.clone(), v.idx)));

        Ok(ChannelStorage {
            by_hash,
            by_name,
            cache: entry_cache,
            db: fs,
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
        if let Some(existing) = self.get(channel.idx)
            && &channel == existing {
                return Ok(());
            }

        self.db
            .set(
                CHANNEL_KEY,
                CHANNEL_KEY.file(&[channel.idx]),
                &postcard::to_vec::<_, 128>(&channel)?,
            )
            .await?;

        self.by_hash.insert(channel.hash, channel.idx);
        self.by_name.insert(channel.name.clone(), channel.idx);

        match self.cache.binary_search_by_key(&channel.idx, |v| v.idx) {
            Ok(v) => self.cache[v] = channel,
            Err(v) => self.cache.insert(v, channel),
        }

        Ok(())
    }
}
