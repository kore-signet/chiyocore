#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
// #![deny(clippy::large_stack_frames)]

extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use embassy_executor::Spawner;
use embassy_time::Duration;
// use embedded_storage::ReadStorage;
use chiyocore::companion::handler::simple_companion::SimpleCompanion;
use chiyocore::companion::handler::storage::{Channel, CompanionConfig};
use chiyocore::companion::tcp::{TCP_COMPANION_CHANNEL, TcpCompanionSink};
use chiyocore::ping_bot::PingBot;
use esp_backtrace as _;
use esp_bootloader_esp_idf::partitions::{DataPartitionSubType, PartitionType};
use esp_hal::clock::CpuClock;
use esp_hal::peripherals::WIFI;
use esp_hal::rng::TrngSource;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;
use esp_radio::wifi::ControllerConfig;
use esp_radio::wifi::sta::StationConfig;
use esp_sync::NonReentrantMutex;
use log::info;
// use chiyocore::handler::{BasicHandlerManager, ContactManager, HandlerStorage};
use chiyocore::storage::{ActiveFilesystem, FsPartition, SimpleFileDb};
use chiyocore::{EspMutex, companion};

esp_bootloader_esp_idf::esp_app_desc!();

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write($val);
        x
    }};
}

#[embassy_executor::task]
async fn heap_log(handler: Arc<EspMutex<SimpleCompanion<PingBot>>>) {
    loop {
        embassy_time::Timer::after_secs(30).await;
        let stats = esp_alloc::HEAP.stats();
        let handler = handler.lock().await;
        let pct = (stats.current_usage as f32 / stats.size as f32) * 100.0;
        log::info!("heap: {}/{} ({pct:.2})", stats.current_usage, stats.size);
        log::info!("scratch: {}", handler.scratch.allocated_bytes());
        log::info!(
            "contacts cache: {} ({} contacts)",
            handler.contacts.cache_size(),
            handler.contacts.cache.len()
        );
        log::info!("channel cache: {}", handler.channels.db.cache_size());
        // log::info!("message log: {}", )
        // log::info!("")
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.2.0

    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 73744);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    let rtc = esp_hal::rtc_cntl::Rtc::new(peripherals.LPWR);
    rtc.set_current_time_us(1772833902705344);
    info!("Embassy initialized!");

    // map.store_item(data_buffer, key, item)

    let mut sha = esp_hal::sha::ShaBackend::new(peripherals.SHA);
    let _sha_backend = sha.start();

    let mut aes = esp_hal::aes::dma::AesDmaBackend::new(peripherals.AES, peripherals.DMA_CH0);
    let _aes_backend = aes.start();

    let _trng_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);

    // let mut trng = TrngSource::new(rng, adc)

    // embassy_time::Timer::after_secs(10).await;

    let mut flash = esp_storage::FlashStorage::new(peripherals.FLASH);

    let mut buffer = [0u8; esp_bootloader_esp_idf::partitions::PARTITION_TABLE_MAX_LEN];
    let partition_table =
        esp_bootloader_esp_idf::partitions::read_partition_table(&mut flash, &mut buffer).unwrap();

    // List all partitions - this is just FYI
    for part in partition_table.iter() {
        log::info!("partition: {:?}", part);
    }

    let lora_pins = chiyocore::XIAO_S3!(peripherals);
    let lora = chiyocore::lora::lora_init(lora_pins).await;

    let (channel, lora_rx) = chiyocore::lora::LoraTaskChannel::start_lora(lora, &spawner);

    // let flash = flash;
    let partition = partition_table
        .find_partition(PartitionType::Data(DataPartitionSubType::LittleFs))
        .unwrap()
        .unwrap();

    let flash = Arc::new(NonReentrantMutex::new(flash));
    let fs_part = FsPartition {
        storage: Arc::clone(&flash),
        partition_offset: partition.offset() as usize,
    };
    let log_part: FsPartition<{ companion::handler::message_log::MESSAGE_LOG_SIZE }> =
        FsPartition {
            storage: Arc::clone(&flash),
            partition_offset: chiyocore::partition_table::LOGS.offset as usize,
        };

    let fs = Arc::new(EspMutex::new(ActiveFilesystem::build(fs_part)));

    let config: CompanionConfig = SimpleFileDb::new(Arc::clone(&fs))
        .get(littlefs2::path!("/companion/config"))
        .await
        .unwrap()
        .unwrap_or_else(|| CompanionConfig {
            wifi_ssid: String::from("your-wifi-here"),
            wifi_password: String::from("password-here"),
        });

    let net_stack = wifi_init(
        peripherals.WIFI,
        &spawner,
        &rtc,
        config.wifi_ssid.clone(),
        config.wifi_password.clone(),
    )
    .await;

    let (tcp_tx, tcp_rx) = TCP_COMPANION_CHANNEL.split();

    let mut handler = SimpleCompanion::load(
        &fs,
        log_part,
        channel.clone(),
        rtc,
        TcpCompanionSink::new(tcp_tx),
        PingBot,
    )
    .await;
    handler.channels.register(&Channel::public()).await.unwrap();
    handler
        .channels
        .register(&Channel::from_name("#emitestcorner", 1))
        .await
        .unwrap();
    handler
        .channels
        .register(&Channel::from_name("#test", 2))
        .await
        .unwrap();
    handler
        .channels
        .register(&Channel::from_name("#wardriving", 3))
        .await
        .unwrap();
    handler
        .channels
        .register(&Channel::from_name("#jokes", 4))
        .await
        .unwrap();

    log::info!("starting handler");

    let handler = Arc::new(EspMutex::new(handler));

    spawner.spawn(heap_log(Arc::clone(&handler))).unwrap();

    spawner
        .spawn(companion::tcp::tcp_companion(
            net_stack,
            tcp_rx,
            Arc::clone(&handler),
        ))
        .unwrap();

    channel.run_handler(lora_rx, Arc::clone(&handler)).await;

    loop {
        embassy_time::block_for(Duration::from_secs(10));
    }
}

async fn wifi_init(
    wifi: WIFI<'static>,
    spawner: &Spawner,
    rtc: &Rtc<'_>,
    ssid: String,
    password: String,
) -> embassy_net::Stack<'static> {
    let station_config = esp_radio::wifi::Config::Station(
        StationConfig::default()
            .with_ssid(ssid)
            .with_password(password),
    );

    println!("Starting wifi");
    let (controller, interfaces) = esp_radio::wifi::new(
        wifi,
        ControllerConfig::default().with_initial_config(station_config),
    )
    .unwrap();
    println!("Wifi configured and started!");

    let wifi_interface = interfaces.station;

    let rng = esp_hal::rng::Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    let embassy_net_config = embassy_net::Config::dhcpv4(Default::default());
    let (stack, runner) = embassy_net::new(
        wifi_interface,
        embassy_net_config,
        mk_static!(
            embassy_net::StackResources<6>,
            embassy_net::StackResources::<6>::new()
        ),
        seed,
    );

    spawner.spawn(chiyocore::ntp::connection(controller)).ok();
    spawner.spawn(chiyocore::ntp::net_task(runner)).ok();
    stack.wait_config_up().await;

    chiyocore::ntp::ntp(stack, rtc).await;

    stack
}
