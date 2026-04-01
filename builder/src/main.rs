// use std::{collections::HashMap, fs::File, path::Path};

// use genco::{
//     Tokens,
//     lang::{Rust, rust},
//     quote, quote_fn, quote_in,
//     tokens::FormatInto,
// };
// use project_root::get_project_root;
// use serde::{Deserialize, Serialize};

// #[derive(Serialize, Deserialize, Debug)]
// struct FirmwareConfiguration {
//     app_stack_size: usize,
//     board: String,
//     global_conf: GlobalConf,
//     nodes: Vec<Node>,
// }

use std::path::PathBuf;

use chiyocore_config::{ChiyocoreConfig, FirmwareConfig, LayerConfig, Node};
use clap::Parser;
use genco::{Tokens, lang::Rust, quote, quote_fn, quote_in, tokens::FormatInto};
use rust_format::Formatter;

use crate::board_def::BoardFile;

fn gen_layer_types(nodes: Vec<LayerConfig>) -> impl FormatInto<Rust> {
    let nodes = nodes
        .into_iter()
        .map(|k| k.kind())
        .collect::<Vec<&'static str>>();
    quote_fn! {
        ($(for n in nodes join (, ) => $n))
    }
}

fn generate_task(nodes: Vec<Node>) -> impl FormatInto<Rust> {
    let node_type = nodes.into_iter().map(|Node { layers, .. }| {
        let layer_types = gen_layer_types(layers.into_owned());
        quote_fn! {
            ChiyocoreNode<$layer_types>
        }
    });

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

fn node_cfg(Node { layers, .. }: Node) -> impl FormatInto<Rust> {
    let layers = layers.into_owned().into_iter();
    quote_fn! {
        ($(for layer in layers join (, ) => $layer))
    }
}

fn load_and_add_nodes(nodes: Vec<Node>) -> impl FormatInto<Rust> {
    let cfgs = nodes.clone().into_iter().map(node_cfg);
    let loads = nodes.into_iter().map(|Node { id, layers }| {
        let layer_tys = gen_layer_types(layers.into_owned());
        quote_fn! {
            load_node_slot::<_, $layer_tys>(c$[str]($[const](id)), &slot_db).await
        }
    });

    quote_fn! {
        chiyocore.add_node(&spawner, ($(for n in loads join (, ) => $n)), &($(for c in cfgs join (, ) => $c))).await
    }
}

fn node_load_fn() -> impl FormatInto<Rust> {
    quote_fn! {
        async fn load_node_slot<const FS_SIZE: usize, T: chiyocore::builder::BuildChiyocoreLayer>(slot: &CStr, slot_db: &SimpleFileDb<FS_SIZE>) -> ChiyocoreNode<T> {
                let identity = if let Some(id) = slot_db
                    .get::<LocalIdentity>(slot)
                    .await
                    .unwrap()
                {
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
    }
}

fn add_channels(channels: Vec<String>) -> impl FormatInto<Rust> {
    //    chiyocore.mesh_storage().channels.write().await.insert(Channel::from_keys(0, "public", ChannelKeys::public())).await;

    let channels = channels.into_iter().enumerate().map(|(idx, channel)| {
        let idx = idx + 1;
                    let c2 = channel.clone();

        genco::tokens::from_fn::<_, Rust>(move |tokens| quote_in! { *tokens =>
            channels.insert(Channel::from_keys($idx, $[str]($[const](channel)), ChannelKeys::from_hashtag($[str]($[const](c2))))).await;
        })
    });

    quote_fn! {
        {
            let mesh_storage = chiyocore.mesh_storage();
            let mut channels = mesh_storage.channels.write().await;

            channels.insert(Channel::from_keys(0, "public", ChannelKeys::public())).await;
            $(for c in channels => $c)
        }
    }
}

#[derive(Parser, Debug)]
struct GenArgs {
    #[arg(short, long)]
    firmware: PathBuf,
    #[arg(short, long)]
    board: PathBuf,
    #[arg(short, long)]
    out: PathBuf,
}

fn main() {
    let args = GenArgs::parse();

    let BoardFile { ram, pins } =
        toml::from_str(&std::fs::read_to_string(args.board).unwrap()).unwrap();
    let ChiyocoreConfig { firmware, nodes } =
        ron::from_str(&std::fs::read_to_string(args.firmware).unwrap()).unwrap();

    let FirmwareConfig {
        stack_size,
        config,
        default_channels,
    } = firmware;
    let global_conf = chiyocore_config::codegen::fmt_litemap(config);
    let gen_task = generate_task(nodes.clone().into_owned());
    let nodes = load_and_add_nodes(nodes.into_owned());
    let node_load_f = node_load_fn();
    let channels = add_channels(default_channels);

    let t: Tokens<Rust> = quote! {
        esp_bootloader_esp_idf::esp_app_desc!();

        $node_load_f

        $gen_task

        #[allow(clippy::large_stack_frames)]
        #[esp_rtos::main]
        async fn main(spawner: Spawner) -> ! {
            esp_println::logger::init_logger_from_env();


            let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
            let peripherals = esp_hal::init(config);

            $ram

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
                $pins,
            )
            .await;

            $channels

            let slot_db = SimpleFileDb::new(Arc::clone(chiyocore.main_fs()), littlefs2::path!("/node_slots/")).await;

            let global_conf = chiyocore
                .config_db()
                .get_persistable::<LiteMap<SmolStr, SmolStr>>(c"general", || {
                    $global_conf
                }).await.unwrap();

            let net_stack = chiyocore.add_network(
                &spawner,
                peripherals.WIFI,
                &global_conf[&"wifi.ssid".into()],
                &global_conf[&"wifi.pw".into()]
            )
            .await;

            net_stack.wait_config_up().await;
            log::info!("network connected - ip {}", net_stack.config_v4().unwrap().address);

            let chiyocore = $nodes;
            // let global_conf = $global_conf;

            let chiyocore = chiyocore.build();

            static APP_CORE_STACK: StaticCell<Stack<$stack_size>> = StaticCell::new();
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

    let generated = t.to_file_string().unwrap();

    let template_rs = include_str!("../res/template.rs");

    let complete = format!("{template_rs}{generated}");
    let complete = rust_format::RustFmt::new().format_str(&complete).unwrap();

    let out_folder = args.out;
    let src_folder = out_folder.join("src/");
    std::fs::create_dir_all(&out_folder).unwrap();
    std::fs::create_dir_all(&src_folder).unwrap();

    let build_rs = include_str!("../res/build.rs");
    let cargo_toml = include_str!("../res/Cargo.toml");

    std::fs::write(out_folder.join("build.rs"), build_rs).unwrap();
    std::fs::write(out_folder.join("Cargo.toml"), cargo_toml).unwrap();
    std::fs::write(src_folder.join("main.rs"), &complete).unwrap();
}

// type GlobalConf = HashMap<String, HashMap<String, String>>;

// #[derive(Serialize, Deserialize, Debug, Clone)]
// struct Node {
//     slot: String,
//     layers: Vec<String>,
// }

// fn load_and_add_node(Node { slot, layers }: Node) -> impl FormatInto<Rust> {
//     quote_fn! {
//             {
//                 let slot_name = c$[str]($[const](slot));
//                 let identity = if let Some(id) = slot_db
//                     .get::<LocalIdentity>(slot_name)
//                     .await
//                     .unwrap()
//                 {
//                     id
//                 } else {
//                     let bytes = rand::Rng::random(&mut Trng::try_new().unwrap());
//                     let seed = ed25519_compact::Seed::new(bytes);
//                     let sk = ed25519_compact::KeyPair::from_seed(seed);
//                     let id = LocalIdentity::from_sk(*sk.sk);
//                     slot_db.insert(slot_name, &id).await.unwrap();
//                     id
//                 };
//                 let node: ChiyocoreNode<($(for n in layers join (, ) => $n))> = ChiyocoreNode::new(identity);
//                 node
//             }
//     }
// }

// fn global_conf_fmt(global_conf: HashMap<String, HashMap<String, String>>) -> impl FormatInto<Rust> {
//     let confs = global_conf.into_iter().flat_map(|(k,v)| {
//         v.into_iter().map(move |(sub_k, sub_v)| -> Tokens<Rust> {
//             let k = k.clone();
//             quote! { (($[str]($[const](k).$[const](sub_k))).into(), ($[str]($[const](sub_v))).into()) }
//         })
//     });

// quote_fn! {
//     chiyocore
//         .config_db()
//         .get_persistable::<LiteMap<SmolStr, SmolStr>>(c"general", || {
//             LiteMap::from_iter([
//                 $(for n in confs join (, ) => $n)
//             ])
//         }).await.unwrap()
// }
// }

// fn generate_task(nodes: Vec<Node>) -> impl FormatInto<Rust> {
//     let node_type = nodes
//         .into_iter()
//         .map(|Node { layers, .. }| {
//             quote! {
//                 ChiyocoreNode<($(for n in layers join (, ) => $n))>
//             }
//         })
//         .collect::<Vec<Tokens<Rust>>>();

//     quote_fn! {
//         #[embassy_executor::task]
//         async fn run_handler(chiyocore: Chiyocore<
//                 <($(for n in node_type join (, ) => $n)) as BuildChiyocoreSet>::Output,
//                 // ($(for n in layers join (, ) => ChiyocoreNode<$n>)) as BuildChiyocoreSet>::Output,
//                 ()
//         >
//         ) {
//             chiyocore.run().await;
//         }
//     }
// }

// fn gen_imports() -> impl FormatInto<Rust> {
//     quote_fn! {
//         extern crate alloc;

//         use alloc::sync::Arc;
//         use chiyocore::builder::{Chiyocore, ChiyocorePeripherals, ChiyocoreSetupData};
//         use chiyocore::ping_bot::PingBot;
//         use chiyocore_companion::companionv2::{Companion, CompanionBuilder};
//         use embassy_executor::Spawner;

//         use esp_backtrace as _;
//         use esp_hal::clock::CpuClock;
//         use esp_hal::rng::Trng;
//         use esp_hal::system::Stack;
//         use esp_hal::timer::timg::TimerGroup;
//         use esp_rtos::embassy::Executor;
//         use litemap::LiteMap;
//         // use chiyocore::handler::{BasicHandlerManager, ContactManager, HandlerStorage};
//         use chiyocore::storage::SimpleFileDb;
//         use chiyocore::{EspMutex, XIAO_S3};
//         use chiyocore::builder::{ChiyocoreNode, BuildChiyocoreLayer, BuildChiyocoreSet};
//         use chiyocore::simple_mesh::MeshLayerGet;
//         use meshcore::identity::LocalIdentity;
//         use smol_str::SmolStr;
//         use static_cell::StaticCell;
//     }
// }

// fn write_tokens(tokens: Tokens<Rust>, path: impl AsRef<Path>) {
//     let file = File::create(path).unwrap();
//     let mut w = genco::fmt::IoWriter::new(file);

//     let fmt =
//         genco::fmt::Config::from_lang::<Rust>().with_indentation(genco::fmt::Indentation::Space(2));
//     let config = rust::Config::default();

//     // Default format state for Rust.
//     let format = rust::Format::default();
//     tokens
//         .format(&mut w.as_formatter(&fmt), &config, &format)
//         .unwrap();
// }

// fn main() {
//     println!("cargo:rerun-if-changed=firmware.toml");

//     let proj_root = get_project_root().unwrap();

//     let FirmwareConfiguration {
//         app_stack_size,
//         board,
//         global_conf,
//         nodes,
//     } = toml::from_str(&std::fs::read_to_string(proj_root.join("firmware.toml")).unwrap()).unwrap();

//     let board_file: BoardFile = toml::from_str(
//         &std::fs::read_to_string(proj_root.join("board-defs").join(format!("{board}.toml")))
//             .unwrap(),
//     )
//     .unwrap();
//     let board_pins = board_file.pins;
//     let board_ram = board_file.ram;

//     // let add_node = load_and_add_node(nodes[0].clone());
//     let node_fns = nodes.clone().into_iter().map(load_and_add_node);

//     let gen_task = generate_task(nodes.clone());
//     let global_conf = global_conf_fmt(global_conf);

//     let imports = gen_imports();

//     let tokens: rust::Tokens = quote! {
//     $imports

// esp_bootloader_esp_idf::esp_app_desc!();

//     $gen_task

// #[allow(clippy::large_stack_frames)]
// #[esp_rtos::main]
// async fn main(spawner: Spawner) -> ! {
//     esp_println::logger::init_logger_from_env();

//     let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
//     let peripherals = esp_hal::init(config);

//     $board_ram

//         let timg0 = TimerGroup::new(peripherals.TIMG0);
//         let sw_int =
//             esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
//         esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

//         let mut chiyocore: Chiyocore<(), ChiyocoreSetupData> = Chiyocore::setup(
//             &spawner,
//             ChiyocorePeripherals {
//                 lpwr: peripherals.LPWR,
//                 sha: peripherals.SHA,
//                 aes: peripherals.AES,
//                 rng: peripherals.RNG,
//                 adc: peripherals.ADC1,
//                 dma: peripherals.DMA_CH0,
//                 flash: peripherals.FLASH,
//             },
//             $board_pins,
//         )
//         .await;

//         let slot_db = SimpleFileDb::new(Arc::clone(chiyocore.main_fs()), littlefs2::path!("/node_slots/")).await;

//         let global_conf = $global_conf;

// let net_stack = chiyocore.add_network(
//     &spawner,
//     peripherals.WIFI,
//     &global_conf[&"wifi.ssid".into()],
//     &global_conf[&"wifi.pw".into()]
// )
// .await;

// net_stack.wait_config_up().await;
// log::info!("network connected - ip {}", net_stack.config_v4().unwrap().address);

// let chiyocore = chiyocore.add_node(&spawner, ($(for n in node_fns join (, ) => $n))).await;
//     let chiyocore = chiyocore.build();

//     static APP_CORE_STACK: StaticCell<Stack<$app_stack_size>> = StaticCell::new();
//     let app_core_stack = APP_CORE_STACK.init(Stack::new());

//     esp_rtos::start_second_core(
//         peripherals.CPU_CTRL,
//         sw_int.software_interrupt1,
//         app_core_stack,
//         move || {
//             static EXECUTOR: StaticCell<Executor> = StaticCell::new();
//             let executor = EXECUTOR.init(Executor::new());
//             executor.run(|spawner| {
//                 spawner.spawn(run_handler(chiyocore)).unwrap();
//             });
//         },
//     );
//     loop {
//         embassy_time::Timer::after_secs(10).await;
//     }
// }
//     };

//     let out_dir = std::env::var_os("OUT_DIR").unwrap();
//     let dest_path = Path::new(&out_dir).join("firmware_def.rs");

//     write_tokens(tokens, dest_path);
//     // std::fs::write(proj_root.join("out.rs"), tokens.to_file_string().unwrap()).unwrap();

//     linker_be_nice();
//     // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
//     println!("cargo:rustc-link-arg=-Tlinkall.x");
// }

mod board_def;
