use std::{collections::HashMap, fs::File, path::Path};

use genco::{
    Tokens,
    lang::{Rust, rust},
    quote, quote_fn, quote_in,
    tokens::FormatInto,
};
use project_root::get_project_root;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
struct FirmwareConfiguration {
    app_stack_size: usize,
    board: String,
    global_conf: GlobalConf,
    nodes: Vec<Node>,
}

#[derive(Serialize, Deserialize, Debug)]
struct BoardFile {
    ram: BoardRam,
    pins: BoardPins,
}

#[derive(Serialize, Deserialize, Debug)]
struct BoardRam {
    reclaimed: String,
    main: String,
    psram: bool,
}

impl FormatInto<Rust> for BoardRam {
    fn format_into(self, tokens: &mut Tokens<Rust>) {
        let BoardRam {
            reclaimed,
            main,
            psram,
        } = self;

        quote_in! {*tokens =>
            esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: $reclaimed);
            esp_alloc::heap_allocator!(size: $main);
            $(if psram {
                esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);
            })
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct BoardPins {
    sclk: String,
    mosi: String,
    miso: String,
    cs: String,
    reset: String,
    busy: String,
    dio1: String,
    rx_en: String,
    spi: String,
}

impl FormatInto<Rust> for BoardPins {
    fn format_into(self, tokens: &mut genco::Tokens<Rust>) {
        let BoardPins {
            sclk,
            mosi,
            miso,
            cs,
            reset,
            busy,
            dio1,
            rx_en,
            spi,
        } = self;
        quote_in! { *tokens =>
            chiyocore::lora::LoraPinBundle {
                sclk: peripherals.$sclk,
                mosi: peripherals.$mosi,
                miso: peripherals.$miso,
                cs: peripherals.$cs,
                reset: peripherals.$reset,
                busy: peripherals.$busy,
                dio1: peripherals.$dio1,
                rx_en: peripherals.$rx_en,
                spi: peripherals.$spi
            }
        }
    }
}

type GlobalConf = HashMap<String, HashMap<String, String>>;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Node {
    slot: String,
    layers: Vec<String>,
}

fn load_and_add_node(Node { slot, layers }: Node) -> impl FormatInto<Rust> {
    quote_fn! {
            {
                let slot_name = c$[str]($[const](slot));
                let identity = if let Some(id) = slot_db
                    .get::<LocalIdentity>(slot_name)
                    .await
                    .unwrap()
                {
                    id
                } else {
                    let bytes = rand::Rng::random(&mut Trng::try_new().unwrap());
                    let seed = ed25519_compact::Seed::new(bytes);
                    let sk = ed25519_compact::KeyPair::from_seed(seed);
                    let id = LocalIdentity::from_sk(*sk.sk);
                    slot_db.insert(slot_name, &id).await.unwrap();
                    id
                };
                let node: ChiyocoreNode<($(for n in layers join (, ) => $n))> = ChiyocoreNode::new(identity);
                node
            }
    }
}

fn global_conf_fmt(global_conf: HashMap<String, HashMap<String, String>>) -> impl FormatInto<Rust> {
    let confs = global_conf.into_iter().flat_map(|(k,v)| {
        v.into_iter().map(move |(sub_k, sub_v)| -> Tokens<Rust> {
            let k = k.clone();
            quote! { (($[str]($[const](k).$[const](sub_k))).into(), ($[str]($[const](sub_v))).into()) }
        })
    });

    quote_fn! {
        chiyocore
            .config_db()
            .get_persistable::<LiteMap<SmolStr, SmolStr>>(c"general", || {
                LiteMap::from_iter([
                    $(for n in confs join (, ) => $n)
                ])
            }).await.unwrap()
    }
}

fn generate_task(nodes: Vec<Node>) -> impl FormatInto<Rust> {
    let node_type = nodes
        .into_iter()
        .map(|Node { layers, .. }| {
            quote! {
                ChiyocoreNode<($(for n in layers join (, ) => $n))>
            }
        })
        .collect::<Vec<Tokens<Rust>>>();

    quote_fn! {
        #[embassy_executor::task]
        async fn run_handler(chiyocore: Chiyocore<
                <($(for n in node_type join (, ) => $n)) as BuildChiyocoreSet>::Output,
                // ($(for n in layers join (, ) => ChiyocoreNode<$n>)) as BuildChiyocoreSet>::Output,
                ()
        >
        ) {
            chiyocore.run().await;
        }
    }
}

fn gen_imports() -> impl FormatInto<Rust> {
    quote_fn! {
        extern crate alloc;

        use alloc::sync::Arc;
        use chiyocore::builder::{Chiyocore, ChiyocorePeripherals, ChiyocoreSetupData};
        use chiyocore::ping_bot::PingBot;
        use chiyocore_companion::companionv2::{Companion, CompanionBuilder};
        use embassy_executor::Spawner;

        use esp_backtrace as _;
        use esp_hal::clock::CpuClock;
        use esp_hal::rng::Trng;
        use esp_hal::system::Stack;
        use esp_hal::timer::timg::TimerGroup;
        use esp_rtos::embassy::Executor;
        use litemap::LiteMap;
        // use chiyocore::handler::{BasicHandlerManager, ContactManager, HandlerStorage};
        use chiyocore::storage::SimpleFileDb;
        use chiyocore::{EspMutex, XIAO_S3};
        use chiyocore::builder::{ChiyocoreNode, BuildChiyocoreLayer, BuildChiyocoreSet};
        use chiyocore::simple_mesh::MeshLayerGet;
        use meshcore::identity::LocalIdentity;
        use smol_str::SmolStr;
        use static_cell::StaticCell;
    }
}

fn write_tokens(tokens: Tokens<Rust>, path: impl AsRef<Path>) {
    let file = File::create(path).unwrap();
    let mut w = genco::fmt::IoWriter::new(file);

    let fmt =
        genco::fmt::Config::from_lang::<Rust>().with_indentation(genco::fmt::Indentation::Space(2));
    let config = rust::Config::default();

    // Default format state for Rust.
    let format = rust::Format::default();
    tokens
        .format(&mut w.as_formatter(&fmt), &config, &format)
        .unwrap();
}

fn main() {
    println!("cargo:rerun-if-changed=firmware.toml");

    let proj_root = get_project_root().unwrap();

    let FirmwareConfiguration {
        app_stack_size,
        board,
        global_conf,
        nodes,
    } = toml::from_str(&std::fs::read_to_string(proj_root.join("firmware.toml")).unwrap()).unwrap();

    let board_file: BoardFile = toml::from_str(
        &std::fs::read_to_string(proj_root.join("board-defs").join(format!("{board}.toml")))
            .unwrap(),
    )
    .unwrap();
    let board_pins = board_file.pins;
    let board_ram = board_file.ram;

    // let add_node = load_and_add_node(nodes[0].clone());
    let node_fns = nodes.clone().into_iter().map(load_and_add_node);

    let gen_task = generate_task(nodes.clone());
    let global_conf = global_conf_fmt(global_conf);

    let imports = gen_imports();

    let tokens: rust::Tokens = quote! {
    $imports

    esp_bootloader_esp_idf::esp_app_desc!();

    $gen_task

    #[esp_rtos::main]
    async fn main(spawner: Spawner) -> ! {
        esp_println::logger::init_logger_from_env();


        let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
        let peripherals = esp_hal::init(config);

        $board_ram

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
            $board_pins,
        )
        .await;

        let slot_db = SimpleFileDb::new(Arc::clone(chiyocore.main_fs()), littlefs2::path!("/node_slots/")).await;

        let global_conf = $global_conf;

        let net_stack = chiyocore.add_network(
            &spawner,
            peripherals.WIFI,
            &global_conf[&"wifi.ssid".into()],
            &global_conf[&"wifi.pw".into()]
        )
        .await;

        net_stack.wait_config_up().await;
        log::info!("network connected - ip {}", net_stack.config_v4().unwrap().address);

        let chiyocore = chiyocore.add_node(&spawner, ($(for n in node_fns join (, ) => $n))).await;
        let chiyocore = chiyocore.build();

        static APP_CORE_STACK: StaticCell<Stack<$app_stack_size>> = StaticCell::new();
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
    };

    let out_dir = std::env::var_os("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("firmware_def.rs");

    write_tokens(tokens, dest_path);
    // std::fs::write(proj_root.join("out.rs"), tokens.to_file_string().unwrap()).unwrap();

    linker_be_nice();
    // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                what if what.starts_with("_defmt_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`"
                    );
                    eprintln!();
                }
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                what if what.starts_with("esp_rtos_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-radio` has no scheduler enabled. Make sure you have initialized `esp-rtos` or provided an external scheduler."
                    );
                    eprintln!();
                }
                "embedded_test_linker_file_not_added_to_rustflags" => {
                    eprintln!();
                    eprintln!(
                        "💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests"
                    );
                    eprintln!();
                }
                "free"
                | "malloc"
                | "calloc"
                | "get_free_internal_heap_size"
                | "malloc_internal"
                | "realloc_internal"
                | "calloc_internal"
                | "free_internal" => {
                    eprintln!();
                    eprintln!(
                        "💡 Did you forget the `esp-alloc` dependency or didn't enable the `compat` feature on it?"
                    );
                    eprintln!();
                }
                _ => (),
            },
            // we don't have anything helpful for "missing-lib" yet
            _ => {
                std::process::exit(1);
            }
        }

        std::process::exit(0);
    }

    println!(
        "cargo:rustc-link-arg=-Wl,--error-handling-script={}",
        std::env::current_exe().unwrap().display()
    );
}
