#![no_std]
#![no_main]

extern crate alloc;

use alloc::sync::Arc;
use chiyocore::builder::{Chiyocore, ChiyocorePeripherals, ChiyocoreSetupData};
use embassy_executor::Spawner;

use chiyocore::simple_mesh::storage::channel::Channel;
use meshcore::crypto::ChannelKeys;

use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::rng::Trng;
use esp_hal::system::Stack;
use esp_hal::timer::timg::TimerGroup;
use esp_rtos::embassy::Executor;
use litemap::LiteMap;
// use chiyocore::handler::{BasicHandlerManager, ContactManager, HandlerStorage};
use alloc::borrow::Cow;
use chiyocore::builder::{BuildChiyocoreLayer, BuildChiyocoreSet, ChiyocoreNode};
use chiyocore::storage::SimpleFileDb;
use core::ffi::CStr;
use meshcore::identity::LocalIdentity;
use smol_str::SmolStr;
use static_cell::StaticCell;
esp_bootloader_esp_idf::esp_app_desc!();

async fn load_node_slot<const FS_SIZE: usize, T: chiyocore::builder::BuildChiyocoreLayer>(
    slot: &CStr,
    slot_db: &SimpleFileDb<FS_SIZE>,
) -> ChiyocoreNode<T> {
    let identity = if let Some(id) = slot_db.get::<LocalIdentity>(slot).await.unwrap() {
        id
    } else {
        let bytes = rand::Rng::random(&mut Trng::try_new().unwrap());
        let seed = ed25519_compact::Seed::new(bytes);
        let sk = ed25519_compact::KeyPair::from_seed(seed);
        let id = LocalIdentity::from_sk(*sk.sk);
        slot_db.insert(slot, &id).await.unwrap();
        id
    };
    let node: ChiyocoreNode<T> = ChiyocoreNode::new(identity);
    node
}

#[embassy_executor::task]
async fn run_handler(
    chiyocore: Chiyocore<
        <(
            ChiyocoreNode<(
                chiyocore_companion::companionv2::Companion,
                chiyocore::ping_bot::PingBot,
            )>,
            ChiyocoreNode<chiyocore_companion::companionv2::Companion>,
        ) as BuildChiyocoreSet>::Output,
        (),
    >,
) {
    chiyocore.run().await;
}

#[allow(clippy::large_stack_frames)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 73744);
    esp_alloc::heap_allocator!(size: 1024 * 32);
    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    let mut chiyocore: Chiyocore<(), ChiyocoreSetupData> = Chiyocore::setup(
        &spawner,
        ChiyocorePeripherals {
            lpwr: peripherals.LPWR,
            sha: peripherals.SHA,
            aes: peripherals.AES,
            rng: peripherals.RNG,
            adc: peripherals.ADC1,
            dma: peripherals.DMA_CH0,
            flash: peripherals.FLASH,
        },
        chiyocore::lora::LoraPinBundle {
            sclk: peripherals.GPIO7,
            mosi: peripherals.GPIO9,
            miso: peripherals.GPIO8,
            cs: peripherals.GPIO41,
            reset: peripherals.GPIO42,
            busy: peripherals.GPIO40,
            dio1: peripherals.GPIO39,
            rx_en: peripherals.GPIO38,
            spi: peripherals.SPI2,
        },
    )
    .await;

    {
        let mesh_storage = chiyocore.mesh_storage();
        let mut channels = mesh_storage.channels.write().await;

        channels
            .insert(Channel::from_keys(0, "public", ChannelKeys::public()))
            .await;
        channels
            .insert(Channel::from_keys(
                1,
                "#test",
                ChannelKeys::from_hashtag("#test"),
            ))
            .await;
        channels
            .insert(Channel::from_keys(
                2,
                "#emitestcorner",
                ChannelKeys::from_hashtag("#emitestcorner"),
            ))
            .await;
        channels
            .insert(Channel::from_keys(
                3,
                "#wardriving",
                ChannelKeys::from_hashtag("#wardriving"),
            ))
            .await;
    }

    let slot_db = SimpleFileDb::new(
        Arc::clone(chiyocore.main_fs()),
        littlefs2::path!("/node_slots/"),
    )
    .await;

    let global_conf = chiyocore
        .config_db()
        .get_persistable::<LiteMap<SmolStr, SmolStr>>(c"general", || {
            litemap::LiteMap::from_iter([
                ("wifi.pw".into(), "nya".into()),
                ("wifi.ssid".into(), "nya".into()),
            ])
        })
        .await
        .unwrap();

    let net_stack = chiyocore
        .add_network(
            &spawner,
            peripherals.WIFI,
            &global_conf[&"wifi.ssid".into()],
            &global_conf[&"wifi.pw".into()],
        )
        .await;

    net_stack.wait_config_up().await;
    log::info!(
        "network connected - ip {}",
        net_stack.config_v4().unwrap().address
    );

    let chiyocore = chiyocore
        .add_node(
            &spawner,
            (
                load_node_slot::<
                    _,
                    (
                        chiyocore_companion::companionv2::Companion,
                        chiyocore::ping_bot::PingBot,
                    ),
                >(c"chiyo0", &slot_db)
                .await,
                load_node_slot::<_, chiyocore_companion::companionv2::Companion>(
                    c"chiyo1", &slot_db,
                )
                .await,
            ),
            &(
                (
                    chiyocore_config::CompanionConfig {
                        id: Cow::Borrowed("companion-0"),
                        tcp_port: 5000,
                    },
                    chiyocore_config::PingBotConfig {
                        name: Cow::Borrowed("cafe / chiyobot 🌃☕"),
                        channels: Cow::Owned(["#test".into(), "#emitestcorner".into()].into()),
                    },
                ),
                (chiyocore_config::CompanionConfig {
                    id: Cow::Borrowed("companion-1"),
                    tcp_port: 3000,
                }),
            ),
        )
        .await;

    let chiyocore = chiyocore.build();

    static APP_CORE_STACK: StaticCell<Stack<32768>> = StaticCell::new();
    let app_core_stack = APP_CORE_STACK.init(Stack::new());

    esp_rtos::start_second_core(
        peripherals.CPU_CTRL,
        sw_int.software_interrupt1,
        app_core_stack,
        move || {
            static EXECUTOR: StaticCell<Executor> = StaticCell::new();
            let executor = EXECUTOR.init(Executor::new());
            executor.run(|spawner| {
                spawner.spawn(run_handler(chiyocore)).unwrap();
            });
        },
    );
    loop {
        embassy_time::Timer::after_secs(10).await;
    }
}
