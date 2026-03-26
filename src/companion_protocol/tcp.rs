use alloc::{sync::Arc, vec::Vec};
use embassy_net::tcp::{State, TcpSocket};
use embassy_time::Duration;
use embedded_io_async::Write;
use meshcore::io::TinyReadExt;
use smallvec::SmallVec;
use thingbuf::mpsc::StaticChannel;
use thingbuf::recycling::DefaultRecycle;

use crate::companion_protocol::protocol::{CompanionSer, CompanionSink};
use crate::companionv2::Companion;
// use crate::ping_bot::PingBot;
use crate::{EspMutex, companion_protocol};

pub static TCP_COMPANION_CHANNEL: StaticChannel<SmallVec<[u8; 256]>, 2> = StaticChannel::new();

struct TcpFrameSink<'a, 'b> {
    tx: &'a mut TcpSocket<'b>,
    scratch: &'a mut SmallVec<[u8; 256]>,
}

impl<'a, 'b> CompanionSink for TcpFrameSink<'a, 'b> {
    async fn write_packet(&mut self, packet: &impl CompanionSer) {
        let ser_size = packet.ser_size();
        self.scratch.resize(ser_size, 0);
        let data = packet.companion_serialize(self.scratch);

        let mut out_actual = Vec::with_capacity(3 + ser_size);
        out_actual.push(b'\x3e');
        out_actual.extend_from_slice(&(ser_size as u16).to_le_bytes());
        out_actual.extend_from_slice(data);

        self.tx.write_all(&out_actual).await;
    }
}

#[derive(Clone)]
pub struct TcpCompanionSink {
    tx: thingbuf::mpsc::StaticSender<SmallVec<[u8; 256]>, DefaultRecycle>,
    // scratch: Vec<u8>,
}

impl TcpCompanionSink {
    pub fn new(tx: thingbuf::mpsc::StaticSender<SmallVec<[u8; 256]>, DefaultRecycle>) -> Self {
        TcpCompanionSink {
            tx,
            // scratch: Vec::with_capacity(32),
        }
    }
}

impl CompanionSink for TcpCompanionSink {
    async fn write_packet(&mut self, packet: &impl CompanionSer) {
        let Ok(mut slot) = self.tx.try_send_ref() else {
            return;
        };

        let size = packet.ser_size();
        slot.push(b'\x3e');
        slot.extend_from_slice(&(size as u16).to_le_bytes());
        slot.resize(size + 3, 0);
        packet.companion_serialize(&mut slot[3..]);
        // slot.extend_from_slice(data);
        drop(slot);
    }
}

#[embassy_executor::task]
pub async fn tcp_companion(
    stack: embassy_net::Stack<'static>,
    packet_rx: thingbuf::mpsc::StaticReceiver<SmallVec<[u8; 256]>, DefaultRecycle>,
    handler: Arc<EspMutex<Companion>>,
) {
    let mut tcp_rx_buf = [0u8; 4096];
    let mut tcp_tx_buf = [0u8; 4096];

    let mut read_buf = [0u8; 255];
    let mut write_scratch: SmallVec<[u8; 256]> = SmallVec::new();

    let mut tcp = TcpSocket::new(stack, &mut tcp_rx_buf, &mut tcp_tx_buf);
    tcp.set_keep_alive(Some(Duration::from_millis(1000)));
    tcp.set_timeout(Some(Duration::from_secs(2)));
    tcp.set_nagle_enabled(false);

    loop {
        log::info!("connecting as companion...");
        tcp.accept(5000).await.unwrap();
        log::info!("companion connected!");

        'recv_loop: loop {
            let wait_closed = async {
                loop {
                    if tcp.state() == State::Closed || !tcp.may_send() || !tcp.may_recv() {
                        return;
                    }
                    embassy_time::Timer::after_millis(1000).await;
                }
            };
            match embassy_futures::select::select3(
                tcp.wait_read_ready(),
                packet_rx.recv_ref(),
                wait_closed,
            )
            .await
            {
                embassy_futures::select::Either3::First(_) => {
                    let bytes_read = tcp.read(&mut read_buf).await.unwrap();
                    if bytes_read == 0 {
                        break;
                    }
                    let mut recvd = &read_buf[..bytes_read];
                    while let Some(frame_start) = memchr::memchr(b'\x3c', recvd) {
                        recvd = &recvd[frame_start + 1..];
                        let len = recvd.read_u16_le().unwrap();
                        let frame_data = recvd.read_slice(len as usize).unwrap();
                        let mut handler = handler.lock().await;
                        if let Err(e) = companion_protocol::protocol::parse_packet(
                            frame_data,
                            &mut *handler,
                            &mut TcpFrameSink {
                                tx: &mut tcp,
                                scratch: &mut write_scratch,
                            },
                        )
                        .await
                        {
                            log::error!("err: {e:?}");
                        }

                        if let Err(e) = tcp.flush().await {
                            log::error!("err: {e:?}");
                            break 'recv_loop;
                        }
                    }
                }
                embassy_futures::select::Either3::Second(packet) => {
                    log::info!("tcp tx");
                    let Some(packet) = packet else { continue };
                    if let Err(e) = tcp.write_all(&packet[..]).await {
                        log::error!("err: {e:?}");
                        break 'recv_loop;
                    }

                    if let Err(e) = tcp.flush().await {
                        log::error!("err: {e:?}");
                        break 'recv_loop;
                    }
                }
                embassy_futures::select::Either3::Third(_) => {
                    log::error!("reset");
                    break 'recv_loop;
                }
            }
        }

        tcp.close();
        tcp.abort();
    }
}
