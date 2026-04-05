use alloc::vec;
use alloc::vec::Vec;
use sequential_storage::{
    cache::PagePointerCache,
    queue::{QueueConfig, QueueStorage},
};

use crate::{
    partition_table, simple_mesh::storage::packet_log::SavedMessage, storage::FsPartition,
};

pub const MESSAGE_LOG_SIZE: usize = partition_table::LOGS.size as usize;

pub type MessageLogStorage = QueueStorage<
    embassy_embedded_hal::adapter::BlockingAsync<FsPartition<MESSAGE_LOG_SIZE>>,
    PagePointerCache<8>,
>;

pub struct MessageLog {
    storage: MessageLogStorage,
    scratch: Vec<u8>,
}

impl MessageLog {
    pub fn new(partition: FsPartition<MESSAGE_LOG_SIZE>) -> MessageLog {
        let storage = sequential_storage::queue::QueueStorage::new(
            embassy_embedded_hal::adapter::BlockingAsync::new(partition),
            QueueConfig::new(const { 0..(MESSAGE_LOG_SIZE - 4096) as u32 }),
            PagePointerCache::new(),
        );
        MessageLog {
            scratch: vec![0u8; 328],
            storage,
        }
    }

    // returns whether message is new
    pub async fn push(&mut self, message: &SavedMessage<'_>) {
        let data = postcard::to_slice(message, &mut self.scratch).unwrap();
        self.storage.push(data, true).await.unwrap(); // todo make result
        // true
    }

    pub async fn pop(&mut self) -> Option<SavedMessage<'_>> {
        let v = self.storage.pop(&mut self.scratch).await.unwrap()?;

        Some(postcard::from_bytes(v).unwrap())
    }
}
