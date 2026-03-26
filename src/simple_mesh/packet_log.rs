use alloc::borrow::Cow;
use arrayref::array_ref;
use esp_hal::sha::Sha1Context;
use meshcore::{Packet, payloads::TextMessageData};
use serde::{Deserialize, Serialize};

use crate::{
    EspMutex,
    companion_protocol::protocol::{
        CompanionSer,
        responses::{ChannelMsgRecv, ContactMsgRecv},
    },
    simple_mesh::storage::{channel::Channel, contact::CachedContact},
};

// use crate::companion::handler::message_log::HashableMessage;

impl<'a> HashableMessage for Packet<'a> {
    async fn hash_into(&self, hasher: &mut Sha1Context, out: &mut [u8; 20]) {
        hasher.update(&self.header.into_bytes()).wait().await;
        hasher.update(&self.payload).wait().await;

        hasher.finalize(out).wait().await;
    }
}

pub struct HashLog<const CAPACITY: usize> {
    log: EspMutex<heapless::Deque<[u8; 20], CAPACITY>>,
}

impl<const CAPACITY: usize> Default for HashLog<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAPACITY: usize> HashLog<CAPACITY> {
    pub fn new() -> HashLog<CAPACITY> {
        HashLog {
            log: EspMutex::new(heapless::Deque::new()),
        }
    }

    pub async fn contains(&self, message: &impl HashableMessage) -> bool {
        let hash = message.hash().await;
        self.contains_hash(&hash).await
    }

    pub async fn contains_hash(&self, hash: &[u8; 20]) -> bool {
        let log = self.log.lock().await;
        let (front, back) = log.as_slices();
        back.contains(hash) || front.contains(hash)
    }

    /// returns true if message is new
    pub async fn push(&self, message: &impl HashableMessage) -> bool {
        let hash = message.hash().await;

        if self.contains_hash(&hash).await {
            return false;
        }

        let mut log = self.log.lock().await;
        if log.is_full() {
            log.pop_back();
        }

        let _ = log.push_front(hash);
        true
    }
}

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
            pk_prefix: *array_ref![contact.key, 0, 6],
            path_len: packet.path.len() as u8,
            text_ty: message.header.text_type(),
            timestamp: message.timestamp,
            signature: None,
            data: Cow::Borrowed(trim_slice_nils(message.message.as_ref())),
        })
    }
}

pub trait HashableMessage {
    fn hash(&self) -> impl core::future::Future<Output = [u8; 20]> {
        async {
            let mut out = [0u8; 20];
            let mut sha = Sha1Context::new();
            self.hash_into(&mut sha, &mut out).await;
            out
        }
    }

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
