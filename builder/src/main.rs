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
                    rsa: peripherals.RSA
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
            defmt::info!("network connected - ip {}", net_stack.config_v4().unwrap().address);

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
                        spawner.spawn(run_handler(chiyocore).unwrap());
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

mod board_def;
