use alloc::vec;
use alloc::vec::Vec;
use chiyo_hal::{embassy_embedded_hal, storage::FsPartition};
use sequential_storage::{
    cache::HeapPagePointerCache,
    queue::{QueueConfig, QueueStorage},
};

use crate::{
    partition_table,
    simple_mesh::storage::packet_log::{HashLog, SavedMessage},
    // storage::FsPartition,
};

pub const MESSAGE_LOG_SIZE: usize = partition_table::LOGS.size as usize;

pub type MessageLogStorage =
    QueueStorage<embassy_embedded_hal::adapter::BlockingAsync<FsPartition>, HeapPagePointerCache>;

pub struct MessageLog {
    storage: MessageLogStorage,
    scratch: Vec<u8>,
    latest: HashLog<16>, // keep a rolling window to make sure messages aren't stored twice
}

impl MessageLog {
    pub fn new(partition: FsPartition) -> MessageLog {
        let part_size = partition.size();
        let storage = sequential_storage::queue::QueueStorage::new(
            embassy_embedded_hal::adapter::BlockingAsync::new(partition),
            QueueConfig::new(0..(part_size - 4096) as u32),
            HeapPagePointerCache::new(8),
        );
        MessageLog {
            scratch: vec![0u8; 328],
            storage,
            latest: HashLog::new(),
        }
    }

    // returns whether message is new
    pub async fn push(&mut self, message: &SavedMessage<'_>) {
        let msg_new = self.latest.push(message).await;
        if !msg_new {
            return;
        }

        let data = postcard::to_slice(message, &mut self.scratch).unwrap();
        self.storage.push(data, true).await.unwrap(); // todo make result
        // true
    }

    pub async fn pop(&mut self) -> Option<SavedMessage<'_>> {
        let v = self.storage.pop(&mut self.scratch).await.unwrap()?;

        Some(postcard::from_bytes(v).unwrap())
    }
}
