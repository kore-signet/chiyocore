use alloc::vec::Vec;
use alloc::{borrow::Cow, vec};
use arrayref::array_ref;
use esp_hal::sha::Sha1Context;
use meshcore::{Packet, payloads::TextMessageData};
use sequential_storage::{
    cache::PagePointerCache,
    queue::{QueueConfig, QueueStorage},
};
use serde::{Deserialize, Serialize};

use crate::{
    companion::{
        handler::storage::{CachedContact, Channel},
        protocol::{
            CompanionSer,
            responses::{ChannelMsgRecv, ContactMsgRecv},
        },
    },
    partition_table,
    storage::FsPartition,
};

// copied off trim_ascii_end's impl
pub fn trim_slice_nils(data: &[u8]) -> &[u8] {
    let mut bytes = data;
    while let [rest @ .., last] = bytes {
        if *last == 0 {
            bytes = rest;
        } else {
            break;
        }
    }

    bytes
}

#[derive(Serialize, Deserialize, Debug)]
pub enum SavedMessage<'a> {
    Contact(ContactMsgRecv<'a>),
    Channel(ChannelMsgRecv<'a>),
}

impl<'a> CompanionSer for SavedMessage<'a> {
    fn ser_size(&self) -> usize {
        match self {
            SavedMessage::Contact(contact_msg_recv) => contact_msg_recv.ser_size(),
            SavedMessage::Channel(channel_msg_recv) => channel_msg_recv.ser_size(),
        }
    }

    fn companion_serialize<'d>(&self, out: &'d mut [u8]) -> &'d [u8] {
        match self {
            SavedMessage::Contact(contact_msg_recv) => contact_msg_recv.companion_serialize(out),
            SavedMessage::Channel(channel_msg_recv) => channel_msg_recv.companion_serialize(out),
        }
    }
}

impl<'a> SavedMessage<'a> {
    pub fn channel_msg(
        channel: &Channel,
        packet: &Packet,
        message: &'a TextMessageData<'a>,
    ) -> Self {
        SavedMessage::Channel(ChannelMsgRecv {
            snr: 0,
            reserved: [0u8; 2],
            idx: channel.idx,
            path_len: packet.path.len() as u8,
            text_ty: message.header.text_type(),
            timestamp: message.timestamp,
            data: Cow::Borrowed(trim_slice_nils(message.message.as_ref())),
        })
    }

    pub fn contact_msg(
        contact: &CachedContact,
        packet: &Packet,
        message: &'a TextMessageData<'a>,
    ) -> Self {
        SavedMessage::Contact(ContactMsgRecv {
            snr: 0,
            reserved: [0u8; 2],
            pk_prefix: *array_ref![contact.verify_key, 0, 6],
            path_len: packet.path.len() as u8,
            text_ty: message.header.text_type(),
            timestamp: message.timestamp,
            signature: None,
            data: Cow::Borrowed(trim_slice_nils(message.message.as_ref())),
        })
    }
}

pub trait HashableMessage {
    fn hash_into(
        &self,
        hasher: &mut Sha1Context,
        out: &mut [u8; 20],
    ) -> impl core::future::Future<Output = ()>;
}

pub struct HashableChannelMessage<'a> {
    pub idx: u8,
    pub timestamp: u32,
    pub data: &'a [u8],
}

impl<'a> HashableMessage for HashableChannelMessage<'a> {
    async fn hash_into(&self, hasher: &mut Sha1Context, out: &mut [u8; 20]) {
        let timestamp = self.timestamp.to_ne_bytes();
        hasher
            .update(&[
                1,
                self.idx,
                timestamp[0],
                timestamp[1],
                timestamp[2],
                timestamp[3],
            ])
            .wait()
            .await;
        hasher.update(self.data).wait().await;
        hasher.finalize(out).wait().await;
    }
}

pub struct HashableContactMessage<'a> {
    pub pk_prefix: &'a [u8; 6],
    pub timestamp: u32,
    pub data: &'a [u8],
}

impl<'a> HashableMessage for HashableContactMessage<'a> {
    async fn hash_into(&self, hasher: &mut Sha1Context, out: &mut [u8; 20]) {
        let timestamp = self.timestamp.to_ne_bytes();
        hasher
            .update(&[0, timestamp[0], timestamp[1], timestamp[2], timestamp[3]])
            .wait()
            .await;
        hasher.update(self.pk_prefix).wait().await;
        hasher.update(self.data).wait().await;
        hasher.finalize(out).wait().await;
    }
}

impl<'a> HashableMessage for SavedMessage<'a> {
    async fn hash_into(&self, hasher: &mut Sha1Context, out: &mut [u8; 20]) {
        match self {
            SavedMessage::Contact(contact_msg_recv) => {
                HashableContactMessage {
                    pk_prefix: &contact_msg_recv.pk_prefix,
                    timestamp: contact_msg_recv.timestamp,
                    data: &contact_msg_recv.data,
                }
                .hash_into(hasher, out)
                .await
            }
            SavedMessage::Channel(channel_msg_recv) => {
                HashableChannelMessage {
                    idx: channel_msg_recv.idx,
                    timestamp: channel_msg_recv.timestamp,
                    data: &channel_msg_recv.data,
                }
                .hash_into(hasher, out)
                .await
            }
        }
    }
}

pub const MESSAGE_LOG_SIZE: usize = partition_table::LOGS.size as usize;

pub type MessageLogStorage = QueueStorage<
    embassy_embedded_hal::adapter::BlockingAsync<FsPartition<MESSAGE_LOG_SIZE>>,
    PagePointerCache<8>,
>;

pub struct MessageLog {
    storage: MessageLogStorage,
    scratch: Vec<u8>,
    pub seen_messages: heapless::Deque<[u8; 20], 16>,
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
            seen_messages: heapless::Deque::new(),
            storage,
        }
    }

    async fn contains(&self, message: &SavedMessage<'_>) -> Option<[u8; 20]> {
        let mut hash = [0u8; 20];
        message.hash_into(&mut Sha1Context::new(), &mut hash).await;

        let (front, back) = self.seen_messages.as_slices();
        if front.contains(&hash) || back.contains(&hash) {
            None
        } else {
            Some(hash)
        }
    }

    pub async fn make_seen(&mut self, message: &impl HashableMessage) {
        let mut hash = [0u8; 20];
        message.hash_into(&mut Sha1Context::new(), &mut hash).await;

        if self.seen_messages.is_full() {
            self.seen_messages.pop_front();
        }
        let _ = self.seen_messages.push_back(hash);
    }

    // returns whether message is new
    pub async fn push(&mut self, message: &SavedMessage<'_>) -> bool {
        if let Some(msg_hash) = self.contains(message).await {
            if self.seen_messages.is_full() {
                self.seen_messages.pop_front();
            }

            let _ = self.seen_messages.push_back(msg_hash);
        } else {
            return false;
        }

        let data = postcard::to_slice(message, &mut self.scratch).unwrap();
        self.storage.push(data, true).await.unwrap(); // todo make result
        true
    }

    pub async fn pop(&mut self) -> Option<SavedMessage<'_>> {
        let v = self.storage.pop(&mut self.scratch).await.unwrap()?;

        Some(postcard::from_bytes(v).unwrap())
    }
}
