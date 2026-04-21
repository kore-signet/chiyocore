use core::net::SocketAddr;

use alloc::sync::Arc;
use chiyo_hal::{embassy_executor, embassy_net, embassy_time, esp_hal};
use defmt::trace;
use embassy_net::{
    dns::{self},
    udp::{PacketMetadata, UdpSocket},
};
use embassy_time::{Duration, Timer};
use esp_hal::rtc_cntl::Rtc;
// use esp_radio::wifi::{ClientConfig, ScanConfig, WifiController, WifiDevice, WifiEvent};
use sntpc::{NtpContext, NtpTimestampGenerator, get_time};
use sntpc_net_embassy::UdpSocketWrapper;

const NTP_SERVER: &str = "time.google.com";

/// Microseconds in a second
const USEC_IN_SEC: u64 = 1_000_000;

#[derive(Clone, Copy)]
struct Timestamp<'a> {
    rtc: &'a Rtc<'a>,
    current_time_us: u64,
}

impl NtpTimestampGenerator for Timestamp<'_> {
    fn init(&mut self) {
        self.current_time_us = self.rtc.current_time_us();
    }

    fn timestamp_sec(&self) -> u64 {
        self.current_time_us / 1_000_000
    }

    fn timestamp_subsec_micros(&self) -> u32 {
        (self.current_time_us % 1_000_000) as u32
    }
}

/// Regularly sync ESP32 RTC using NTP.
#[embassy_executor::task]
pub async fn ntp_task(stack: embassy_net::Stack<'static>, rtc: Arc<Rtc<'static>>) {
    const NTP_DELAY: embassy_time::Duration = embassy_time::Duration::from_secs(30);

    loop {
        'inner: loop {
            let res =
                embassy_time::with_timeout(Duration::from_millis(5000), ntp_once(stack, &rtc))
                    .await;
            if res.is_ok() {
                break 'inner;
            }
        }

        embassy_time::Timer::after(NTP_DELAY).await;
    }
}

/// Do a single sync'ing of the ESP32 RTC using NTP.
pub async fn ntp_once(stack: embassy_net::Stack<'static>, rtc: &Rtc<'_>) {
    loop {
        if stack.is_link_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    // println!("Waiting to get IP address...");
    loop {
        if let Some(_config) = stack.config_v4() {
            // println!("Got IP: {}", config.address);
            break;
        }
        Timer::after(Duration::from_millis(1000)).await;
    }

    const BUFF_SZ: usize = 1024;

    trace!("Prepare NTP lookup");
    let mut ip_addr = stack
        .dns_query(NTP_SERVER, dns::DnsQueryType::A)
        .await
        .unwrap();
    let addr = ip_addr.pop().unwrap();
    trace!("NTP DNS: {:?}", addr);

    let s_addr = SocketAddr::from((addr, 123));

    let mut rx_meta = alloc::vec![PacketMetadata::EMPTY; 16];
    let mut rx_buffer = alloc::vec![0; BUFF_SZ];
    let mut tx_meta = alloc::vec![PacketMetadata::EMPTY; 16];
    let mut tx_buffer = alloc::vec![0; BUFF_SZ];

    let mut socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );
    socket.bind(1234).expect("Unable to bind to UDP socket");

    let socket_wrapper = UdpSocketWrapper::new(socket);
    let context = NtpContext::new(Timestamp {
        rtc,
        current_time_us: rtc.current_time_us(),
    });

    stack.wait_config_up().await;

    let result = get_time(s_addr, &socket_wrapper, context).await.unwrap();
    rtc.set_current_time_us(
        (result.sec() as u64 * USEC_IN_SEC) + ((result.sec_fraction() as u64 * USEC_IN_SEC) >> 32),
    );

    trace!("ntp sync'd to: {}", result.seconds);
}
