#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
// #![deny(clippy::large_stack_frames)]

extern crate alloc;

use alloc::sync::Arc;
use chiyocore::companion_protocol::tcp::TcpCompanionSink;
use chiyocore::companionv2::Companion;
use chiyocore::lora::LoraTaskChannel;
use chiyocore::ping_bot::PingBot;
use chiyocore::simple_mesh::SimpleMesh;
use chiyocore::simple_mesh::storage::MeshStorage;
use chiyocore::simple_mesh::storage::channel::Channel;
use embassy_executor::Spawner;

use embassy_sync::rwlock::RwLock;
use esp_backtrace as _;
use esp_bootloader_esp_idf::partitions::{DataPartitionSubType, PartitionType};
use esp_hal::clock::CpuClock;
use esp_hal::rng::{Trng, TrngSource};
use esp_hal::system::Stack;
use esp_hal::timer::timg::TimerGroup;
use esp_rtos::embassy::Executor;
use esp_sync::NonReentrantMutex;
use log::info;
// use chiyocore::handler::{BasicHandlerManager, ContactManager, HandlerStorage};
use chiyocore::storage::{ActiveFilesystem, FsPartition, SimpleFileDb};
use chiyocore::{DataWithSnr, EspMutex};
use meshcore::crypto::ChannelKeys;
use meshcore::identity::LocalIdentity;
use static_cell::StaticCell;
use thingbuf::recycling::DefaultRecycle;

esp_bootloader_esp_idf::esp_app_desc!();

// #[embassy_executor::task]
// async fn heap_log(handler: Arc<EspMutex<SimpleCompanion<PingBot>>>) {
//     loop {
//         embassy_time::Timer::after_secs(30).await;
//         let stats = esp_alloc::HEAP.stats();
//         let handler = handler.lock().await;
//         let pct = (stats.current_usage as f32 / stats.size as f32) * 100.0;
//         log::info!("heap: {}/{} ({pct:.2})", stats.current_usage, stats.size);
//         log::info!("scratch: {}", handler.scratch.allocated_bytes());
//         log::info!(
//             "contacts cache: {} ({} contacts)",
//             handler.storage.contacts.cache_size(),
//             handler.storage.contacts.cache.len()
//         );
//         log::info!("channel cache: {}", handler.storage.channels.cache_size());
//     }
// }

#[embassy_executor::task]
async fn run_handler(
    lora_channel: LoraTaskChannel,
    rx: thingbuf::mpsc::StaticReceiver<DataWithSnr, DefaultRecycle>,
    handler: Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>,
    layer: (Arc<EspMutex<Companion>>, Arc<EspMutex<PingBot>>),
) {
    lora_channel.run_handler(rx, handler, layer).await;
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.2.0

    esp_println::logger::init_logger_from_env();
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 73744);
    esp_alloc::heap_allocator!(size: 1024 * 32);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);
    let rtc = Arc::new(esp_hal::rtc_cntl::Rtc::new(peripherals.LPWR));

    info!("Embassy initialized!");

    let mut sha = esp_hal::sha::ShaBackend::new(peripherals.SHA);
    let _sha_backend = sha.start();

    let mut aes = esp_hal::aes::dma::AesDmaBackend::new(peripherals.AES, peripherals.DMA_CH0);
    let _aes_backend = aes.start();

    let _trng_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);

    let mut flash = esp_storage::FlashStorage::new(peripherals.FLASH).multicore_auto_park();

    let mut buffer = [0u8; esp_bootloader_esp_idf::partitions::PARTITION_TABLE_MAX_LEN];
    let partition_table =
        esp_bootloader_esp_idf::partitions::read_partition_table(&mut flash, &mut buffer).unwrap();

    for part in partition_table.iter() {
        log::info!("partition: {:?}", part);
    }

    let lora_pins = chiyocore::XIAO_S3!(peripherals);
    let lora = chiyocore::lora::lora_init(lora_pins).await;
    let (lora_app_channel, lora_rx) = LoraTaskChannel::start_lora(lora, &spawner);

    let partition = partition_table
        .find_partition(PartitionType::Data(DataPartitionSubType::LittleFs))
        .unwrap()
        .unwrap();

    let flash = Arc::new(NonReentrantMutex::new(flash));
    let fs_part = FsPartition {
        storage: Arc::clone(&flash),
        partition_offset: partition.offset() as usize,
    };
    let log_part: FsPartition<{ chiyocore::companionv2::message_log::MESSAGE_LOG_SIZE }> =
        FsPartition {
            storage: Arc::clone(&flash),
            partition_offset: chiyocore::partition_table::LOGS.offset as usize,
        };

    // let config: CompanionConfig = SimpleFileDb::new(Arc::clone(&fs))
    //     .get(littlefs2::path!("/companion/config"))
    //     .await
    //     .unwrap()
    //     .unwrap_or_else(|| CompanionConfig {
    //         wifi_ssid: String::from("your-wifi-here"),
    //         wifi_password: String::from("password-here"),
    //     });
    let net_stack = chiyocore::wifi::wifi_init(
        peripherals.WIFI,
        &spawner,
        "".into(),
        "".into(),
    )
    .await;

    chiyocore::ntp::ntp(net_stack, &rtc).await;

    let (tcp_tx, tcp_rx) = chiyocore::companion_protocol::tcp::TCP_COMPANION_CHANNEL.split();

    let fs = Arc::new(EspMutex::new(ActiveFilesystem::build(fs_part)));
    let companion_db = SimpleFileDb::new(Arc::clone(&fs), littlefs2::path!("/companion/")).await;
    let identity = if let Some(id) = companion_db
        .get::<LocalIdentity>(c"identity")
        .await
        .unwrap()
    {
        id
    } else {
        let bytes = rand::Rng::random(&mut Trng::try_new().unwrap());
        let seed = ed25519_compact::Seed::new(bytes);
        let sk = ed25519_compact::KeyPair::from_seed(seed);
        let id = LocalIdentity::from_sk(*sk.sk);
        companion_db.insert(c"identity", &id).await.unwrap();
        id
    };

    let mesh_storage = MeshStorage::new(&fs).await;
    {
        let mut channels = mesh_storage.channels.write().await;
        channels
            .insert(Channel::from_keys(0, "public", ChannelKeys::public()))
            .await
            .unwrap();
        channels
            .insert(Channel::from_keys(
                1,
                "#test",
                ChannelKeys::from_hashtag("#test"),
            ))
            .await
            .unwrap();
        channels
            .insert(Channel::from_keys(
                2,
                "#emitestcorner",
                ChannelKeys::from_hashtag("#emitestcorner"),
            ))
            .await
            .unwrap();
    }

    let mesh = Arc::new(RwLock::new(SimpleMesh::new(
        identity,
        mesh_storage.clone(),
        lora_app_channel.clone(),
    )));
    let companion = Arc::new(EspMutex::new(
        Companion::new(
            &rtc,
            mesh_storage,
            &fs,
            log_part,
            &mesh,
            TcpCompanionSink::new(tcp_tx),
        )
        .await
        .unwrap(),
    ));
    let ping_bot = Arc::new(EspMutex::new(PingBot {
        rtc: Arc::clone(&rtc),
    }));

    spawner
        .spawn(chiyocore::companion_protocol::tcp::tcp_companion(
            net_stack,
            tcp_rx,
            Arc::clone(&companion),
        ))
        .unwrap();

    // lora_app_channel.run_handler(lora_rx, mesh, companion).await;

    static APP_CORE_STACK: StaticCell<Stack<16384>> = StaticCell::new();
    let app_core_stack = APP_CORE_STACK.init(Stack::new());

    esp_rtos::start_second_core(
        peripherals.CPU_CTRL,
        sw_int.software_interrupt1,
        app_core_stack,
        move || {
            static EXECUTOR: StaticCell<Executor> = StaticCell::new();
            let executor = EXECUTOR.init(Executor::new());
            executor.run(|spawner| {
                spawner
                    .spawn(run_handler(
                        lora_app_channel,
                        lora_rx,
                        mesh,
                        (companion, ping_bot),
                    ))
                    .unwrap();
            });
        },
    );

    loop {
        embassy_time::Timer::after_secs(10).await;
    }
}
