use core::ops::Deref;

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use arrayref::array_ref;
use embassy_embedded_hal::adapter::BlockingAsync;
use esp_storage::FlashStorage;
use meshcore::io::SliceWriter;
use sequential_storage::{
    cache::HeapKeyPointerCache,
    map::{MapConfig, MapStorage},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use xxhash_rust::{
    const_xxh32, xxh32,
};

use crate::{EspMutex, FirmwareError, FirmwareResult};

pub struct FsPartition {
    pub storage: Arc<esp_sync::NonReentrantMutex<FlashStorage<'static>>>,
    pub partition_size: usize,
    pub partition_offset: usize,
}

impl FsPartition {
    pub fn in_range(&self, address: usize) -> bool {
        address >= self.partition_offset && address <= (self.partition_offset + self.partition_size)
    }

    pub fn map_offset(&self, offset: usize) -> Option<usize> {
        let mapped = self.partition_offset + offset;
        if self.in_range(mapped) {
            Some(mapped)
        } else {
            None
        }
    }

    pub fn size(&self) -> usize {
        self.partition_size
    }
}

impl embedded_storage::nor_flash::ErrorType for FsPartition {
    type Error = esp_storage::FlashStorageError;
}

impl embedded_storage::nor_flash::ReadNorFlash for FsPartition {
    const READ_SIZE: usize = 4;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        let Some(mapped) = self.map_offset(offset as usize) else {
            return Err(esp_storage::FlashStorageError::OutOfBounds);
        };

        if !self.in_range(mapped + bytes.len()) {
            return Err(esp_storage::FlashStorageError::OutOfBounds);
        };

        self.storage.with(|storage| {
            embedded_storage::nor_flash::ReadNorFlash::read(storage, mapped as u32, bytes)
        })
    }

    fn capacity(&self) -> usize {
        self.partition_size
    }
}

impl embedded_storage::nor_flash::NorFlash for FsPartition {
    const WRITE_SIZE: usize = 4;

    const ERASE_SIZE: usize = 4096;

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        let Some(mapped_start) = self.map_offset(from as usize) else {
            return Err(esp_storage::FlashStorageError::OutOfBounds);
        };

        let Some(mapped_end) = self.map_offset(to as usize) else {
            return Err(esp_storage::FlashStorageError::OutOfBounds);
        };

        self.storage.with(|storage| {
            embedded_storage::nor_flash::NorFlash::erase(
                storage,
                mapped_start as u32,
                mapped_end as u32,
            )
        })
    }

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        let Some(mapped) = self.map_offset(offset as usize) else {
            return Err(esp_storage::FlashStorageError::OutOfBounds);
        };

        if !self.in_range(mapped + bytes.len()) {
            return Err(esp_storage::FlashStorageError::OutOfBounds);
        };

        self.storage.with(|storage| {
            embedded_storage::nor_flash::NorFlash::write(storage, mapped as u32, bytes)
        })
    }
}

impl embedded_storage::nor_flash::MultiwriteNorFlash for FsPartition {}

const HASH_SEED: u32 = 10_01_10_01;
const DIR_INDEX_HASH: u32 = const_xxh32::xxh32(b"_CHIYO_INTERNAL_DIRINDEX", HASH_SEED);
pub const GLOBAL_DIR: DirKey = DirKey::const_new(b"chiyoglobal");

#[derive(Copy, Clone, defmt::Format, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct DirKey(u32);

impl DirKey {
    pub const fn const_new(key: &'static [u8]) -> DirKey {
        DirKey(const_xxh32::xxh32(key, HASH_SEED))
    }

    // pub fn new(key: &[u8]) -> DirKey {
    //     DirKey(xxh64::xxh64(key, HASH_SEED))
    // }

    pub fn file(&self, key: &[u8]) -> FileKey {
        FileKey(self.0, xxh32::xxh32(key, HASH_SEED))
    }
}

// #[repr(transparent)]
#[derive(Copy, Clone, defmt::Format, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct FileKey(u32, u32);

impl sequential_storage::map::Key for FileKey {
    fn serialize_into(
        &self,
        buffer: &mut [u8],
    ) -> Result<usize, sequential_storage::map::SerializationError> {
        if buffer.len() < 8 {
            return Err(sequential_storage::map::SerializationError::BufferTooSmall);
        }

        buffer[0..4].copy_from_slice(&self.0.to_le_bytes());
        buffer[4..8].copy_from_slice(&self.1.to_le_bytes());

        Ok(8)
    }

    fn deserialize_from(
        buffer: &[u8],
    ) -> Result<(Self, usize), sequential_storage::map::SerializationError> {
        if buffer.len() < 8 {
            return Err(sequential_storage::map::SerializationError::BufferTooSmall);
        }

        let a = array_ref![buffer, 0, 4];
        let b = array_ref![buffer, 4, 4];

        Ok((FileKey(u32::from_le_bytes(*a), u32::from_le_bytes(*b)), 8))
    }

    fn get_len(_buffer: &[u8]) -> Result<usize, sequential_storage::map::SerializationError> {
        Ok(8)
    }
}

pub struct Directory<'a> {
    len: u16,
    dir_key: DirKey,
    entries: &'a [[u8; 4]],
}

impl<'a> Directory<'a> {
    pub fn contains(&self, key: &FileKey) -> bool {
        self.entries.contains(&key.1.to_le_bytes())
    }
}

impl<'a> sequential_storage::map::Value<'a> for Directory<'a> {
    fn serialize_into(
        &self,
        buffer: &mut [u8],
    ) -> Result<usize, sequential_storage::map::SerializationError> {
        if buffer.len() < 2 + 4 + (self.entries.len() * 4) {
            return Err(sequential_storage::map::SerializationError::BufferTooSmall);
        }

        let mut writer = SliceWriter::new(buffer);
        writer.write_u16_le(self.len);
        writer.write_u32_le(self.dir_key.0);
        for entry in self.entries {
            writer.write_slice(&entry[..]);
        }

        Ok(writer.finish().len())
    }

    fn deserialize_from(
        buffer: &'a [u8],
    ) -> Result<(Self, usize), sequential_storage::map::SerializationError>
    where
        Self: Sized,
    {
        if buffer.len() < 6 {
            return Err(sequential_storage::map::SerializationError::BufferTooSmall);
        }

        let len = u16::from_le_bytes(*array_ref![buffer, 0, 2]);
        let dir_key = u32::from_le_bytes(*array_ref![buffer, 2, 4]);
        let bytes_len = (len * 4) as usize;
        if buffer.len() < 6 + bytes_len {
            return Err(sequential_storage::map::SerializationError::BufferTooSmall);
        }

        Ok((
            Directory {
                len,
                dir_key: DirKey(dir_key),
                entries: bytemuck::cast_slice(&buffer[6..6 + bytes_len]),
            },
            6 + bytes_len,
        ))
    }
}


/// *very* simple 'filesystem'. supports non-nestable, iterable directories
/// each item can only be, at most, 4096 bytes long. directories can contain ~500 entries
#[derive(Clone)]
pub struct ChiyoFilesystem {
    inner: Arc<
        EspMutex<MapStorage<FileKey, BlockingAsync<FsPartition>, HeapKeyPointerCache<FileKey>>>,
    >,
}

impl ChiyoFilesystem {
    pub async fn new(partition: FsPartition) -> FirmwareResult<Self> {
        let flash_range = 0..partition.size() as u32;
        let map = MapStorage::new(
            BlockingAsync::new(partition),
            MapConfig::new(flash_range),
            HeapKeyPointerCache::new(8, 16),
        );

        Ok(ChiyoFilesystem {
            inner: Arc::new(EspMutex::new(map)),
        })
    }

    pub async fn get<'a>(
        &self,
        key: FileKey,
        buf: &'a mut [u8],
    ) -> FirmwareResult<Option<&'a [u8]>> {
        let mut fs = self.inner.lock().await;
        fs.fetch_item(buf, &key)
            .await
            .map_err(FirmwareError::SequentialStorage)
        
    }

    pub async fn get_deser<const BUF_SIZE: usize, T: DeserializeOwned>(
        &self,
        key: FileKey,
    ) -> FirmwareResult<Option<T>> {
        let mut buf = [0u8; BUF_SIZE];
        let Some(read_val) = self.get(key, &mut buf).await? else {
            return Ok(None);
        };
        Ok(Some(
            postcard::from_bytes(read_val).map_err(FirmwareError::Postcard)?,
        ))
    }

    pub async fn get_or_insert<const BUF_SIZE: usize, T: Serialize + DeserializeOwned>(
        &self,
        dir: DirKey,
        key: FileKey,
        val: T,
    ) -> FirmwareResult<T> {
        let mut buf = [0u8; BUF_SIZE];
        if let Some(read_val) = self.get(key, &mut buf).await? {
            postcard::from_bytes(read_val).map_err(FirmwareError::Postcard)
        } else {
            let v = postcard::to_allocvec(&val).map_err(FirmwareError::Postcard)?;
            self.set(dir, key, &v).await?;
            Ok(val)
        }
    }

    pub async fn set<'a>(
        &self,
        directory_key: DirKey,
        key: FileKey,
        value: &[u8],
    ) -> FirmwareResult<()> {
        let mut scratch_buf = alloc::vec![0u8; 4096];

        let mut fs = self.inner.lock().await;
        let directory_idx_key = FileKey(directory_key.0, DIR_INDEX_HASH);

        let directory = fs
            .fetch_item::<Directory>(&mut scratch_buf[..], &directory_idx_key)
            .await
            .map_err(FirmwareError::SequentialStorage)?;

        if let Some(directory) = directory {
            if !directory.contains(&key) {
                let mut entries: Vec<[u8; 4]> = bytemuck::cast_slice(directory.entries).to_vec();
                entries.push(key.1.to_le_bytes());
                entries.sort_unstable();
                let dir = Directory {
                    len: entries.len() as u16,
                    dir_key: directory_key,
                    entries: bytemuck::cast_slice(&entries[..]),
                };

                fs.store_item(&mut scratch_buf[..], &directory_idx_key, &dir)
                    .await
                    .map_err(FirmwareError::SequentialStorage)?;
            }
        } else {
            let fkey = key.1.to_le_bytes();
            let directory = Directory {
                len: 1,
                dir_key: directory_key,
                entries: &[fkey],
            };

            fs.store_item(&mut scratch_buf[..], &directory_idx_key, &directory)
                .await
                .map_err(FirmwareError::SequentialStorage)?;
        }

        fs.store_item::<&[u8]>(&mut scratch_buf[..], &key, &value)
            .await
            .map_err(FirmwareError::SequentialStorage)?;

        Ok(())
    }

    pub async fn delete(&self, key: FileKey) -> FirmwareResult<()> {
        let mut fs = self.inner.lock().await;
        let mut buf = alloc::vec![0u8; 4096];
        
        let dir = fs
            .fetch_item::<Directory>(&mut buf[..], &FileKey(key.0, DIR_INDEX_HASH))
            .await
            .map_err(FirmwareError::SequentialStorage)?;

        if let Some(dir) = dir {
            let mut entries = dir.entries.to_vec();
            if let Some(idx) = entries.iter().position(|v| u32::from_le_bytes(*v) == key.1) {
                entries.remove(idx);
                fs.store_item(&mut buf[..], &FileKey(key.0, DIR_INDEX_HASH), &Directory {
                    len: entries.len() as u16,
                    dir_key: DirKey(key.0),
                    entries: &entries[..],
                }).await.map_err(FirmwareError::SequentialStorage)?;
            }
        }
        
        self.inner
            .lock()
            .await
            .remove_item(&mut buf[..], &key)
            .await
            .map_err(FirmwareError::SequentialStorage)
    }

    pub async fn directory_entries(
        &self,
        directory: DirKey,
    ) -> FirmwareResult<Option<DirectoryEntries>> {
        let mut fs = self.inner.lock().await;
        let mut buf = alloc::vec![0u8; 4096];
        let dir = fs
            .fetch_item::<Directory>(&mut buf[..], &FileKey(directory.0, DIR_INDEX_HASH))
            .await
            .map_err(FirmwareError::SequentialStorage)?;
        let Some(dir) = dir else { return Ok(None) };

        Ok(Some(DirectoryEntries {
            cursor: 0,
            key: directory,
            len: dir.len as usize,
            entries: dir.entries.into(),
        }))
    }

    /// NOTE: Holding more than one persistableobject of the same key will cause them to go out of sync! fixing that is in the 'maybe' zone since it'd require some fancy arc'swapping trickery
    pub async fn get_persistable<T: Serialize + DeserializeOwned>(
        &self,
        directory: DirKey,
        key: FileKey,
        default: impl FnOnce() -> T,
    ) -> FirmwareResult<PersistedObject<T>> {
        match self.get_deser::<512, T>(key).await {
            Ok(Some(v)) => Ok(PersistedObject {
                key,
                dir: directory,
                data: v,
                db: self.clone(),
            }),
            Ok(None) => Ok({
                let v = default();
                let v_ser = postcard::to_allocvec(&v)?;
                self.set(directory, key, &v_ser).await?;
                PersistedObject {
                    key,
                    dir: directory,
                    data: v,
                    db: self.clone(),
                }
            }),
            Err(e) => Err(e),
        }
    }
}

pub struct DirectoryEntries {
    cursor: usize,
    key: DirKey,
    len: usize,
    entries: Box<[[u8; 4]]>,
}

impl DirectoryEntries {
    pub fn reader(self, fs: &ChiyoFilesystem) -> DirectoryReader<'_> {
        DirectoryReader {
            entries: self,
            fs,
            buf: alloc::vec![0u8; 4096],
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

impl Iterator for DirectoryEntries {
    type Item = FileKey;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.len {
            None
        } else {
            let entry = FileKey(self.key.0, u32::from_le_bytes(self.entries[self.cursor]));
            self.cursor += 1;
            Some(entry)
        }
    }
}

pub struct DirectoryReader<'a> {
    entries: DirectoryEntries,
    fs: &'a ChiyoFilesystem,
    buf: Vec<u8>,
}

impl<'a> DirectoryReader<'a> {
    pub async fn next_file<'b>(&'b mut self) -> Option<FirmwareResult<&'b [u8]>> {
        let next_file = self.entries.next()?;
        self.fs.get(next_file, &mut self.buf[..]).await.transpose()
    }
}

/// An object backed by an underlying SimpleFileDb (useful for, e.g, keeping a configuration value both in-memory and on-flash)
pub struct PersistedObject<T: Serialize + DeserializeOwned> {
    pub key: FileKey,
    pub dir: DirKey,
    data: T,
    db: ChiyoFilesystem,
}

impl<T: Serialize + DeserializeOwned> Deref for PersistedObject<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T: Serialize + DeserializeOwned> PersistedObject<T> {
    pub fn get(&self) -> &T {
        &self.data
    }

    /// Sync a new value to flash.
    pub async fn set(&mut self, new_val: T) -> FirmwareResult<()> {
        self.db
            .set(self.dir, self.key, &postcard::to_allocvec(&new_val)?)
            .await?;
        self.data = new_val;
        Ok(())
    }

    /// Mutate the stored data, and sync it to flash.
    pub async fn with_mut(&mut self, f: impl FnOnce(&mut T)) -> FirmwareResult<()> {
        f(&mut self.data);
        self.db
            .set(self.dir, self.key, &postcard::to_allocvec(&self.data)?)
            .await?;
        Ok(())
    }
}
