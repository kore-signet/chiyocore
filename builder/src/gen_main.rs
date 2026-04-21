use std::collections::HashMap;

// use chiyocore_config::{ChiyocoreConfig, };
use genco::{Tokens, lang::Rust, quote, quote_fn, quote_in, tokens::FormatInto};
use litemap::LiteMap;
use meshcore::payloads::{AdvertType, AdvertisementExtraData, AppdataFlags};
use rust_format::Formatter;

use crate::{
    board_def::BoardFile,
    config::{ChiyocoreBaseConf, FirmwareConfig, FullConfig, LayerConfig, NodeConfig},
};

fn gen_layer_types(nodes: impl Iterator<Item = LayerConfig>) -> impl FormatInto<Rust> {
    let nodes = nodes.into_iter().map(|k| k.kind).collect::<Vec<String>>();
    quote_fn! {
        ($(for n in nodes join (, ) => $n))
    }
}

fn generate_task(nodes: impl Iterator<Item = NodeConfig>) -> impl FormatInto<Rust> {
    let node_type = nodes.into_iter().map(|NodeConfig { layers, .. }| {
        let layer_types = gen_layer_types(layers.clone().into_values());
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

fn node_cfg(NodeConfig { layers, .. }: NodeConfig) -> impl FormatInto<Rust> {
    // let layers = layers.into_values();

    let layers = layers.into_iter().map(|(k, v)| {
        let v = serde_json::to_string(&v.values).unwrap();
        quote! {
            ($[str]($[const](k)), serde_json::from_str($[str]($[const](v))).unwrap())
        }
    });

    quote_fn! {
        ($(for layer in layers join (, ) => $layer))
    }
}

fn role_to_tokens(role: AdvertType) -> Tokens<Rust> {
    match role {
        AdvertType::None => quote! { AdvertType::None },
        AdvertType::ChatNode => quote! { AdvertType::ChatNode },
        AdvertType::Repeater => quote! { AdvertType::Repeater },
        AdvertType::RoomServer => quote! { AdvertType::RoomServer },
        AdvertType::Sensor => quote! { AdvertType::Sensor },
    }
}

fn load_and_add_nodes(nodes: HashMap<String, NodeConfig>) -> impl FormatInto<Rust> {
    let cfgs = nodes.clone().into_values().map(node_cfg);
    let loads = nodes.into_values().map(|NodeConfig { id, layers, name, role }| {
        let layer_tys = gen_layer_types(layers.clone().into_values());
        let role = role_to_tokens(role);
        quote_fn! {
            load_node_slot::<$layer_tys>($[str]($[const](name)), $role, $[str]($[const](id)), &slot_db).await
        }
    });

    quote_fn! {
        chiyocore.add_node(&spawner, ($(for n in loads join (, ) => $n)), &($(for c in cfgs join (, ) => $c))).await
    }
}

fn node_load_fn() -> impl FormatInto<Rust> {
    quote_fn! {
        use chiyocore::meshcore::payloads::{AppdataFlags, AdvertType, AdvertisementExtraData};

        const NODE_SLOT_DIR: DirKey = DirKey::const_new(b"chiyonodeslots");
        const NODE_ADVERT_DIR: DirKey = DirKey::const_new(b"chiyoadvertslots");

        async fn load_node_slot<T: chiyocore::builder::BuildChiyocoreLayer>(name: &'static str, advert_role: AdvertType, slot: &'static str, slot_db: &ChiyoFilesystem) -> ChiyocoreNode<T> {
                let advert_flags = match advert_role {
                    AdvertType::None => AppdataFlags::HAS_NAME,
                    AdvertType::ChatNode => AppdataFlags::HAS_NAME | AppdataFlags::IS_CHAT_NODE,
                    AdvertType::Repeater => AppdataFlags::HAS_NAME | AppdataFlags::IS_REPEATER,
                    AdvertType::RoomServer => AppdataFlags::HAS_NAME | AppdataFlags::IS_ROOM_SERVER,
                    AdvertType::Sensor => AppdataFlags::HAS_NAME | AppdataFlags::IS_SENSOR
                };

                let def_advert = AdvertisementExtraData {
                    flags: advert_flags,
                    latitude: None,
                    longitude: None,
                    feature_1: None,
                    feature_2: None,
                    name: Some(name.as_bytes().into()),
                };

                
                let advert = slot_db.get_persistable(NODE_ADVERT_DIR, NODE_ADVERT_DIR.file(slot.as_bytes()), || def_advert).await.unwrap();


                let identity = if let Some(id) = slot_db
                    .get_deser::<512, LocalIdentity>(NODE_SLOT_DIR.file(slot.as_bytes()))
                    .await
                    .unwrap()
                {
                    id
                } else {
                    let bytes = rand::Rng::random(&mut Trng::try_new().unwrap());
                    let seed = ed25519_compact::Seed::new(bytes);
                    let sk = ed25519_compact::KeyPair::from_seed(seed);
                    let id = LocalIdentity::from_sk(*sk.sk);
                    let id_ser = chiyo_hal::postcard::to_allocvec(&id).unwrap();
                    slot_db.set(NODE_SLOT_DIR, NODE_SLOT_DIR.file(slot.as_bytes()), &id_ser).await.unwrap();
                    id
                };
                let node: ChiyocoreNode<T> = ChiyocoreNode::new(identity, advert);
                node
        }
    }
    // use meshcore::payloads::ad
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

pub fn gen_main(BoardFile { ram, pins }: BoardFile, conf: FullConfig) -> String {
    let FullConfig {
        firmware,
        chiyocore,
        stackup,
    } = conf;

    let FirmwareConfig { stack_size, .. } = firmware;
    let ChiyocoreBaseConf {
        config,
        default_channels,
    } = chiyocore;

    let global_config_inserts = config.into_iter().map(|(k,v)| {
        let k2 = k.clone();
        quote! {
            chiyocore.main_fs().get_or_insert::<512, (SmolStr, SmolStr)>(
                chiyocore::GLOBAL_VARS_DIR,
                chiyocore::GLOBAL_VARS_DIR.file($[str]($[const](k)).as_bytes()),
                ($[str]($[const](k2)).into(), $[str]($[const](v)).into())
            ).await.unwrap();
        }
    });

    let gen_task = generate_task(stackup.clone().into_values());
    let nodes = load_and_add_nodes(stackup.clone());
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

            $(for c in global_config_inserts => $c)

            let net_stack = chiyocore.add_network(
                &spawner,
                peripherals.WIFI,
                &chiyocore.main_fs().get_deser::<128, (SmolStr, SmolStr)>(chiyocore::GLOBAL_VARS_DIR.file(b"wifi.ssid")).await.unwrap().unwrap().1,
                &chiyocore.main_fs().get_deser::<128, (SmolStr, SmolStr)>(chiyocore::GLOBAL_VARS_DIR.file(b"wifi.pw")).await.unwrap().unwrap().1,
            )
            .await;

            net_stack.wait_config_up().await;
            defmt::info!("network connected - ip {}", net_stack.config_v4().unwrap().address);

            let slot_db = chiyocore.main_fs().clone();

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

    rust_format::RustFmt::new().format_str(&complete).unwrap()
}
