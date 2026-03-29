use core::{ffi::CStr, ops::Deref};

use alloc::{
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use base64::{Engine, prelude::BASE64_URL_SAFE};
use embedded_storage::nor_flash::ReadNorFlash;
use esp_storage::FlashStorage;
use littlefs2::{consts::U256, fs::Filesystem, path::Path};
use ouroboros::self_referencing;
use serde::{Serialize, de::DeserializeOwned};

pub const FS_SIZE: usize = partition_table::MESHCORE_DATA.size as usize;

use crate::{EspMutex, FirmwareError, FirmwareResult, partition_table};

pub struct FsPartition<const SIZE: usize> {
    pub storage: Arc<esp_sync::NonReentrantMutex<FlashStorage<'static>>>,
    pub partition_offset: usize,
}

impl<const SIZE: usize> FsPartition<SIZE> {
    pub fn new(
        storage: &Arc<esp_sync::NonReentrantMutex<FlashStorage<'static>>>,
        partition: &partition_table::Partition,
    ) -> Self {
        assert_eq!(SIZE as u32, partition.size);
        FsPartition {
            storage: Arc::clone(storage),
            partition_offset: partition.offset as usize,
        }
    }

    pub fn in_range(&self, address: usize) -> bool {
        address >= self.partition_offset && address <= (self.partition_offset + SIZE)
    }

    pub fn map_offset(&self, offset: usize) -> Option<usize> {
        let mapped = self.partition_offset + offset;
        if self.in_range(mapped) {
            Some(mapped)
        } else {
            None
        }
    }
}

fn esp_err_to_littlefs(err: esp_storage::FlashStorageError) -> littlefs2::io::Error {
    use esp_storage::FlashStorageError;
    use littlefs2::io::Error as LittlefsError;
    match err {
        FlashStorageError::IoError => LittlefsError::IO,
        FlashStorageError::IoTimeout => LittlefsError::IO,
        FlashStorageError::CantUnlock => LittlefsError::IO,
        FlashStorageError::NotAligned => LittlefsError::INVALID,
        FlashStorageError::OutOfBounds => LittlefsError::INVALID,
        FlashStorageError::OtherCoreRunning => LittlefsError::IO,
        FlashStorageError::Other(_) => LittlefsError::INVALID,
        _ => LittlefsError::INVALID,
    }
}

impl<const SIZE: usize> littlefs2::driver::Storage for FsPartition<SIZE> {
    const READ_SIZE: usize = 4;
    const WRITE_SIZE: usize = 4;
    const BLOCK_SIZE: usize = 4096;
    const BLOCK_COUNT: usize = { SIZE / 4096 };
    const BLOCK_CYCLES: isize = 100;
    type CACHE_SIZE = U256;
    type LOOKAHEAD_SIZE = U256;

    fn read(&mut self, off: usize, buf: &mut [u8]) -> littlefs2::io::Result<usize> {
        let Some(mapped) = self.map_offset(off) else {
            return Err(littlefs2::io::Error::INVALID);
        };

        if !self.in_range(mapped + buf.len()) {
            return Err(littlefs2::io::Error::INVALID);
        };

        self.storage
            .with(|fs| fs.read(mapped as u32, buf))
            .map_err(esp_err_to_littlefs)?;
        Ok(buf.len())
    }

    fn write(&mut self, off: usize, data: &[u8]) -> littlefs2::io::Result<usize> {
        let Some(mapped) = self.map_offset(off) else {
            return Err(littlefs2::io::Error::INVALID);
        };

        if !self.in_range(mapped + data.len()) {
            return Err(littlefs2::io::Error::INVALID);
        };

        self.storage
            .with(|storage| embedded_storage::Storage::write(storage, mapped as u32, data))
            .map_err(esp_err_to_littlefs)?;
        Ok(data.len())
    }

    fn erase(&mut self, off: usize, len: usize) -> littlefs2::io::Result<usize> {
        let Some(mapped_start) = self.map_offset(off) else {
            return Err(littlefs2::io::Error::INVALID);
        };

        let Some(mapped_end) = self.map_offset(off + len) else {
            return Err(littlefs2::io::Error::INVALID);
        };

        self.storage
            .with(|storage| {
                embedded_storage::nor_flash::NorFlash::erase(
                    storage,
                    mapped_start as u32,
                    mapped_end as u32,
                )
            })
            .map_err(esp_err_to_littlefs)?;
        Ok(len)
    }
}

impl<const SIZE: usize> embedded_storage::nor_flash::ErrorType for FsPartition<SIZE> {
    type Error = esp_storage::FlashStorageError;
}

impl<const SIZE: usize> embedded_storage::nor_flash::ReadNorFlash for FsPartition<SIZE> {
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
        SIZE
    }
}

impl<const SIZE: usize> embedded_storage::nor_flash::NorFlash for FsPartition<SIZE> {
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

impl<const SIZE: usize> embedded_storage::nor_flash::MultiwriteNorFlash for FsPartition<SIZE> {}

#[self_referencing]
pub struct ActiveFilesystem<const SIZE: usize> {
    pub partition: FsPartition<SIZE>,
    pub allocation: littlefs2::fs::Allocation<FsPartition<SIZE>>,
    #[borrows(mut partition, mut allocation)]
    #[not_covariant]
    pub fs: Filesystem<'this, FsPartition<SIZE>>,
}

impl<const SIZE: usize> ActiveFilesystem<SIZE> {
    pub fn build(mut partition: FsPartition<SIZE>) -> Self {
        let alloc = littlefs2::fs::Allocation::new();
        if !Filesystem::is_mountable(&mut partition) {
            Filesystem::format(&mut partition).unwrap();
        }

        ActiveFilesystemBuilder {
            partition,
            allocation: alloc,
            fs_builder:
                |part: &mut FsPartition<SIZE>,
                 alloc: &mut littlefs2::fs::Allocation<FsPartition<SIZE>>| {
                    Filesystem::mount(alloc, part).unwrap()
                },
        }
        .build()
    }
}

// pub type ThreadSafeFS = Arc<EspMutex<ActiveFilesystem<SIZE>>>;

pub trait FileDbKey: Eq + Ord + Copy + core::fmt::Debug {
    fn encode(&self, out: &mut String);
    fn prefix_matches(&self, prefix: &[u8]) -> bool;
    fn prefix_cmp(&self, prefix: &[u8]) -> core::cmp::Ordering;
}

impl FileDbKey for u8 {
    fn encode(&self, out: &mut String) {
        out.push_str(&self.to_string())
    }

    fn prefix_matches(&self, prefix: &[u8]) -> bool {
        *self == prefix[0]
    }

    fn prefix_cmp(&self, prefix: &[u8]) -> core::cmp::Ordering {
        [*self][..].cmp(prefix)
    }
}

impl<const N: usize> FileDbKey for [u8; N] {
    fn encode(&self, out: &mut String) {
        BASE64_URL_SAFE.encode_string(self, out);
    }

    fn prefix_matches(&self, prefix: &[u8]) -> bool {
        self.starts_with(prefix)
    }

    fn prefix_cmp(&self, prefix: &[u8]) -> core::cmp::Ordering {
        self[..].cmp(prefix)
    }
}

pub trait Cacheable: Serialize + DeserializeOwned {
    type Key: FileDbKey;
    type Cached: CachedVersion<Self::Key>;
    // type Key= [u8; N];

    fn key(&self) -> &Self::Key;
    fn as_cached(&self) -> Self::Cached;
}

pub trait CachedVersion<K: FileDbKey> {
    fn key(&self) -> &K;
    fn size(&self) -> usize;
}

pub struct CachedFileDb<const SIZE: usize, T: Cacheable> {
    prefix: &'static littlefs2::path::Path,
    fs: Arc<EspMutex<ActiveFilesystem<SIZE>>>,
    pub cache: Vec<T::Cached>,
}

impl<T: Cacheable, const SIZE: usize> CachedFileDb<SIZE, T> {
    pub async fn init(
        fs_handle: Arc<EspMutex<ActiveFilesystem<SIZE>>>,
        prefix: &'static littlefs2::path::Path,
    ) -> Self {
        let mut fs = fs_handle.lock().await;
        // fs.lock().await.read_dir(path).unwrap();

        // let prefix = littlefs2::path::Path::from_

        let cache = fs.with_fs_mut(|fs| {
            fs.create_dir_all(prefix);
            fs.read_dir_and_then(prefix, |dir| {
                let mut cache = Vec::new();
                let mut scratch = heapless::Vec::<u8, 512>::new();

                for entry in dir {
                    let entry = entry.unwrap();
                    if entry.metadata().is_dir() {
                        continue;
                    }

                    let data: T = fs
                        .open_file_and_then(entry.path(), |f| {
                            scratch.clear();
                            f.read_to_end(&mut scratch).unwrap();

                            Ok(postcard::from_bytes(&scratch).unwrap())
                        })
                        .unwrap();

                    cache.push(data.as_cached());
                }

                cache.sort_unstable_by_key(|v| *v.key());
                cache.shrink_to_fit();
                Ok(cache)
            })
            .unwrap()
        });

        drop(fs);

        CachedFileDb {
            prefix,
            fs: fs_handle,
            cache,
        }
    }

    fn path(&self, k: &T::Key) -> littlefs2::path::PathBuf {
        let mut path = String::new();
        k.encode(&mut path);
        path.push('\x00');

        self.prefix.join(Path::from_str_with_nul(&path).unwrap())
    }

    fn find_by_prefix(&self, prefix: &[u8]) -> Option<&T::Cached> {
        match self.cache.binary_search_by(|a| a.key().prefix_cmp(prefix)) {
            Ok(idx) => Some(&self.cache[idx]),
            Err(idx) => {
                let v = &self
                    .cache
                    .get(core::cmp::min(self.cache.len().saturating_sub(1), idx))?;
                if v.key().prefix_matches(prefix) {
                    Some(v)
                } else {
                    None
                }
            }
        }
    }

    pub fn get_cached(&self, prefix: &[u8]) -> Option<&T::Cached> {
        self.find_by_prefix(prefix)
        // self.cache
        // .iter()
        // .find(|v| v.key().prefix_matches(prefix))
        // .map(|v| v);

        // self.cache.iter().find(|v| v.key().prefix_matches(prefix))
    }

    pub fn contains(&self, prefix: &[u8]) -> bool {
        self.find_by_prefix(prefix).is_some()
        // self.cache.iter().any(|v| v.key().prefix_matches(prefix))
        // self.cache.iter().any(|(k, _)| k.prefix_matches(prefix))
    }

    pub async fn get_full(&self, key: &T::Key) -> FirmwareResult<Option<T>> {
        let path = self.path(key);
        let res = self.fs.lock().await.with_fs_mut(|fs| fs.read::<512>(&path));

        match res {
            Ok(v) => Ok(Some(postcard::from_bytes(&v)?)),
            Err(littlefs2::io::Error::NO_SUCH_ENTRY) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn insert(&mut self, entry: &T) -> FirmwareResult<()> {
        let path = self.path(entry.key());
        let entry_data = postcard::to_allocvec(entry)?;

        self.fs
            .lock()
            .await
            .with_fs_mut(|fs| fs.write(&path, &entry_data))?;

        match self.cache.binary_search_by(|a| entry.key().cmp(a.key())) {
            Ok(idx) => self.cache[idx] = entry.as_cached(),
            Err(idx) => self.cache.insert(idx, entry.as_cached()),
        }

        Ok(())
    }

    pub async fn delete(&mut self, key: &T::Key) -> FirmwareResult<()> {
        let Ok(pos) = self.cache.binary_search_by(|a| key.cmp(a.key())) else {
            return Ok(());
        };

        self.cache.remove(pos);

        let path = self.path(key);
        self.fs.lock().await.with_fs_mut(|fs| fs.remove(&path))?;

        Ok(())
    }

    pub fn cache_size(&self) -> usize {
        self.cache.iter().map(|v| v.size()).sum()
    }
}

#[derive(Clone)]
pub struct SimpleFileDb<const SIZE: usize> {
    fs: Arc<EspMutex<ActiveFilesystem<SIZE>>>,
    prefix: littlefs2::path::PathBuf,
}

impl<const SIZE: usize> SimpleFileDb<SIZE> {
    pub async fn new(
        fs: Arc<EspMutex<ActiveFilesystem<SIZE>>>,
        prefix: &littlefs2::path::Path,
    ) -> SimpleFileDb<SIZE> {
        fs.lock().await.with_fs_mut(|fs| fs.create_dir_all(prefix));
        SimpleFileDb {
            fs,
            prefix: littlefs2::path::PathBuf::from_path(prefix),
        }
    }

    fn path(&self, path: &CStr) -> littlefs2::path::PathBuf {
        self.prefix.join(Path::from_cstr(path).unwrap())
    }

    pub async fn entries<S: DeserializeOwned, R>(
        &self,
        mapper: impl Fn(S) -> R,
    ) -> FirmwareResult<Vec<R>> {
        //    let cache = fs.with_fs_mut(|fs| {
        //         fs.create_dir_all(prefix);
        Ok(self.fs.lock().await.with_fs_mut(|fs| {
            fs.read_dir_and_then(&self.prefix, |dir| {
                let mut out = Vec::new();
                let mut scratch = heapless::Vec::<u8, 512>::new();
                for entry in dir {
                    let entry = entry.unwrap();
                    if entry.file_type().is_dir() {
                        continue;
                    }

                    let data: S = fs
                        .open_file_and_then(entry.path(), |f| {
                            scratch.clear();
                            f.read_to_end(&mut scratch).unwrap();

                            Ok(postcard::from_bytes(&scratch).unwrap())
                        })
                        .unwrap();

                    out.push(mapper(data));
                }

                Ok(out)
            })
            .unwrap()
        }))
    }

    pub async fn get<T: DeserializeOwned>(&self, key: &CStr) -> FirmwareResult<Option<T>> {
        let path = self.path(key);
        let res = self.fs.lock().await.with_fs_mut(|fs| fs.read::<512>(&path));

        match res {
            Ok(v) => Ok(Some(postcard::from_bytes(&v).unwrap())),
            Err(littlefs2::io::Error::NO_SUCH_ENTRY) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn insert<T: Serialize>(&self, key: &CStr, val: &T) -> FirmwareResult<()> {
        let path = self.path(key);
        let data = postcard::to_allocvec(val).unwrap();
        self.fs
            .lock()
            .await
            .with_fs_mut(|f| f.write(&path, &data))
            .map_err(FirmwareError::from)
    }

    pub async fn delete(&self, key: &CStr) -> FirmwareResult<()> {
        let path = self.path(key);
        self.fs
            .lock()
            .await
            .with_fs_mut(|fs| fs.remove(&path))
            .map_err(FirmwareError::from)
    }

    pub async fn get_persistable<T: Serialize + DeserializeOwned + Default>(
        &self,
        key: &'static CStr,
        def: impl FnOnce() -> T,
    ) -> FirmwareResult<PersistedObject<T, SIZE>> {
        let data = match self.get(key).await? {
            Some(v) => v,
            None => {
                let v = def();
                self.insert(key, &v).await?;
                v
            }
        };

        Ok(PersistedObject {
            key,
            data,
            db: self.clone(),
        })
    }
}

pub struct PersistedObject<T: Serialize + DeserializeOwned + Default, const SIZE: usize> {
    pub key: &'static CStr,
    data: T,
    db: SimpleFileDb<SIZE>,
}

impl<T: Serialize + DeserializeOwned + Default, const SIZE: usize> Deref
    for PersistedObject<T, SIZE>
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T: Serialize + DeserializeOwned + Default, const SIZE: usize> PersistedObject<T, SIZE> {
    pub async fn set(&mut self, new_val: T) -> FirmwareResult<()> {
        self.db.insert(self.key, &new_val).await?;
        self.data = new_val;
        Ok(())
    }

    pub async fn with_mut(&mut self, f: impl FnOnce(&mut T)) -> FirmwareResult<()> {
        f(&mut self.data);
        self.db.insert(self.key, &self.data).await?;
        Ok(())
    }
}
