use crate::{
    CompanionResult,
    simple_mesh::{
        SimpleMesh,
        storage::{channel::Channel, contact::CachedContact},
    },
};
use alloc::sync::Arc;
use chiyo_hal::embassy_sync;
use chiyo_hal::{EspMutex, esp_sync};
use core::ops::DerefMut;
use embassy_sync::rwlock::RwLock;
use futures_util::FutureExt;
use lora_phy::mod_params::PacketStatus;
use meshcore::{
    Packet,
    identity::ForeignIdentity,
    payloads::{
        Ack, Advert, ControlPayload, RequestPayload, ReturnedPath, TextMessageData, TracePacket,
    },
};

/// A SimpleMeshLayer is a high-level handler that is 'layered' onto a SimpleMesh node. Several layers can be composed on a single node (e.g, both a Companion and a Ping Bot).
/// Layer methods are run on every packet receive, so they should *never* block and ideally not take a significant amount of time. Consider using tasks for longer workloads.
pub trait SimpleMeshLayer {
    /// Called on every packet, irrespective of whether the payload is decodable or not.
    fn packet<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_bytes: &'f [u8],
        packet_status: PacketStatus,
    ) -> impl Future<Output = CompanionResult<()>>;

    /// Handle a single MeshCore direct message. The passed contact is the sender for the message.
    fn text_message<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        message: &'f TextMessageData<'_>,
    ) -> impl Future<Output = CompanionResult<()>>;

    /// Handle a group/channel message.
    fn group_text<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        channel: &'f Channel,
        message: &'f TextMessageData<'_>,
    ) -> impl Future<Output = CompanionResult<()>>;

    /// Handle an ACK. Since ACK packets can be contained in a ReturnedPath message, sometimes the referenced packet will not be a simple ack but rather the full path packet.
    fn ack<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        ack: &'f Ack,
    ) -> impl Future<Output = CompanionResult<()>>;

    /// Handle an inbound advert.
    fn advert<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        advert: &'f Advert<'_>,
    ) -> impl Future<Output = CompanionResult<()>>;

    /// Handle a returned path message. If this message contains a sub-payload (such as an ACK), you do not need to handle that; the methods for the sub-payload will be automatically called.
    fn returned_path<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        path: &'f ReturnedPath<'_>,
    ) -> impl Future<Output = CompanionResult<()>>;

    /// Handle a response message.
    fn response<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        response: &'f [u8],
    ) -> impl Future<Output = CompanionResult<()>>;

    /// Handle a request message.
    fn request<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        request: &'f RequestPayload<'_>,
    ) -> impl Future<Output = CompanionResult<()>>;

    /// Handle an anonymous (i.e, not requiring a pre-existing shared contact) message.
    fn anonymous_request<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f ForeignIdentity,
        data: &'f [u8],
    ) -> impl Future<Output = CompanionResult<()>>;

    /// Handle a trace packet.
    fn trace_packet<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        snrs: &'f [i8],
        trace: &'f TracePacket<'_>,
    ) -> impl Future<Output = CompanionResult<()>>;

    /// Handle a control packet.
    fn control_packet<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        payload: &'f ControlPayload,
    ) -> impl Future<Output = CompanionResult<()>>;
}

/* da tuple zone */

impl<A> SimpleMeshLayer for &mut A
where
    A: SimpleMeshLayer,
{
    fn packet<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_bytes: &'f [u8],
        packet_status: PacketStatus,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::packet(self, mesh, packet, packet_bytes, packet_status)
    }

    fn text_message<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        message: &'f TextMessageData<'_>,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::text_message(self, mesh, packet, packet_status, contact, message)
    }

    fn group_text<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        channel: &'f Channel,
        message: &'f TextMessageData<'_>,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::group_text(self, mesh, packet, packet_status, channel, message)
    }

    fn ack<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        ack: &'f Ack,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::ack(self, mesh, packet, packet_status, ack)
    }

    fn advert<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        advert: &'f Advert<'_>,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::advert(self, mesh, packet, packet_status, advert)
    }

    fn returned_path<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        path: &'f ReturnedPath<'_>,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::returned_path(self, mesh, packet, packet_status, contact, path)
    }

    fn response<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        response: &'f [u8],
    ) -> impl Future<Output = CompanionResult<()>> {
        A::response(self, mesh, packet, packet_status, contact, response)
    }

    fn request<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f CachedContact,
        request: &'f RequestPayload<'_>,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::request(self, mesh, packet, packet_status, contact, request)
    }

    fn anonymous_request<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        contact: &'f ForeignIdentity,
        data: &'f [u8],
    ) -> impl Future<Output = CompanionResult<()>> {
        A::anonymous_request(self, mesh, packet, packet_status, contact, data)
    }

    fn trace_packet<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        snrs: &'f [i8],
        trace: &'f TracePacket<'_>,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::trace_packet(self, mesh, packet, packet_status, snrs, trace)
    }

    fn control_packet<'f>(
        &'f mut self,
        mesh: &'f SimpleMesh,
        packet: &'f Packet<'_>,
        packet_status: PacketStatus,
        payload: &'f ControlPayload,
    ) -> impl Future<Output = CompanionResult<()>> {
        A::control_packet(self, mesh, packet, packet_status, payload)
    }
}

pub trait WithLayer {
    fn call_me(&self, layer: impl SimpleMeshLayer) -> impl Future<Output = ()>;
}

pub trait MeshLayerGet: Send {
    type Layer<'a>: SimpleMeshLayer + Send;

    fn with_layer(&self, f: impl WithLayer) -> impl Future<Output = ()>;
}

impl<L: SimpleMeshLayer + Send + 'static> MeshLayerGet for Arc<RwLock<esp_sync::RawMutex, L>> {
    type Layer<'a> = &'a mut L;

    fn with_layer(&self, f: impl WithLayer) -> impl Future<Output = ()> {
        self.write()
            .then(|mut v| async move { f.call_me(&mut *v).await })
    }
}

impl<L: SimpleMeshLayer + Send + 'static> MeshLayerGet for Arc<EspMutex<L>> {
    type Layer<'a> = &'a mut L;

    fn with_layer(&self, f: impl WithLayer) -> impl Future<Output = ()> {
        self.lock()
            .then(|mut v| async move { f.call_me(&mut *v).await })
    }
}

macro_rules! impl_mesh_layer_tuple {
    ($join:path; $($var:ident),*) => {
        #[allow(non_snake_case)]
        impl <$($var),*> MeshLayerGet for ($(Arc<RwLock<chiyo_hal::esp_sync::RawMutex, $var>>),*) where $($var: SimpleMeshLayer + Send + 'static),* {
            type Layer<'a> = ($(&'a mut $var),*);

                async fn with_layer(&self, f: impl WithLayer) {
                    let ($($var),*) = self;
                    let ($(mut $var),*) = $join(
                        $(
                           $var.write()
                        ),*
                    ).await;

                    f.call_me(($($var.deref_mut()),*)).await;
                }
        }

        #[allow(non_snake_case)]
        impl <$($var),*> MeshLayerGet for ($(Arc<chiyo_hal::EspMutex<$var>>),*) where $($var: SimpleMeshLayer + Send + 'static),* {
            type Layer<'a> = ($(&'a mut $var),*);

                async fn with_layer(&self, f: impl WithLayer) {
                    let ($($var),*) = self;
                    let ($(mut $var),*) = $join(
                        $(
                           $var.lock()
                        ),*
                    ).await;

                    f.call_me(($($var.deref_mut()),*)).await;
                }
        }

        #[allow(non_snake_case)]
        impl <$($var),*> SimpleMeshLayer for ($(&mut $var),*) where $($var: SimpleMeshLayer),* {
                async fn packet<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_bytes: &'f [u8],
                    packet_status: PacketStatus,
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.packet(mesh, packet, packet_bytes, packet_status)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn text_message<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    contact: &'f CachedContact,
                    message: &'f TextMessageData<'_>,
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.text_message(mesh, packet, packet_status, contact, message)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn group_text<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    channel: &'f Channel,
                    message: &'f TextMessageData<'_>,
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.group_text(mesh, packet, packet_status, channel, message)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn ack<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    ack: &'f Ack,
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.ack(mesh, packet, packet_status, ack)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn advert<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    advert: &'f Advert<'_>,
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.advert(mesh, packet, packet_status, advert)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn returned_path<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    contact: &'f CachedContact,
                    path: &'f ReturnedPath<'_>,
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.returned_path(mesh, packet, packet_status, contact, path)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn response<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    contact: &'f CachedContact,
                    response_bytes: &'f [u8]
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.response(mesh, packet, packet_status, contact, response_bytes)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn request<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    contact: &'f CachedContact,
                    request: &'f RequestPayload<'_>
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.request(mesh, packet, packet_status, contact, request)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn anonymous_request<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    contact: &'f ForeignIdentity,
                    request: &'f [u8]
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.anonymous_request(mesh, packet, packet_status, contact, request)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }


                async fn trace_packet<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    snrs: &'f [i8],
                    trace: &'f TracePacket<'_>
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.trace_packet(mesh, packet, packet_status, snrs, trace)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }

                async fn control_packet<'f>(
                    &'f mut self,
                    mesh: &'f SimpleMesh,
                    packet: &'f Packet<'_>,
                    packet_status: PacketStatus,
                    payload: &'f ControlPayload
                ) -> CompanionResult<()> {
                    let ($($var),*) = self;
                    let ($($var),*) = $join(
                        $(
                           $var.control_packet(mesh, packet, packet_status, payload)
                        ),*
                    ).await;
                    $(
                        $var?;
                    )*
                    Ok(())
                }
        }
    };
}

impl_mesh_layer_tuple!(
     chiyo_hal::embassy_futures::join::join;
    A,B
);
impl_mesh_layer_tuple!(
     chiyo_hal::embassy_futures::join::join3;
    A,B,C
);
impl_mesh_layer_tuple!(
    chiyo_hal::embassy_futures::join::join4;
    A,B,C,D
);
impl_mesh_layer_tuple!(
     chiyo_hal::embassy_futures::join::join5;
    A,B,C,D,F
);
