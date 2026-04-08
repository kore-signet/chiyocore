use core::{ffi::CStr, ops::Deref};

use alloc::{
    sync::Arc,
    vec::Vec,
};
use chiyo_hal::{embedded_storage, esp_storage, esp_sync};
use embedded_storage::nor_flash::ReadNorFlash;
use esp_storage::FlashStorage;
use littlefs2::{consts::U256, fs::Filesystem, path::Path};
use ouroboros::self_referencing;
use serde::{Serialize, de::DeserializeOwned};

/// Size of the main data fs.
pub const FS_SIZE: usize = partition_table::MESHCORE_DATA.size as usize;

use crate::{FirmwareError, FirmwareResult, partition_table};
use chiyo_hal::EspMutex;

/// A single flash partition, backed by a shared/locked reference to the flash driver.
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

/// A Littlefs2 filesystem, alongside its backing partition.
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

/// A flash-stored, key-value database, using serde-deserializable values and byte keys.
/// Each key is stored as a single file under a specified prefix.
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
        let _ = fs.lock().await.with_fs_mut(|fs| fs.create_dir_all(prefix));
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

/// An object backed by an underlying SimpleFileDb (useful for, e.g, keeping a configuration value both in-memory and on-flash)
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
    /// Sync a new value to flash.
    pub async fn set(&mut self, new_val: T) -> FirmwareResult<()> {
        self.db.insert(self.key, &new_val).await?;
        self.data = new_val;
        Ok(())
    }

    /// Mutate the stored data, and sync it to flash.
    pub async fn with_mut(&mut self, f: impl FnOnce(&mut T)) -> FirmwareResult<()> {
        f(&mut self.data);
        self.db.insert(self.key, &self.data).await?;
        Ok(())
    }
}
