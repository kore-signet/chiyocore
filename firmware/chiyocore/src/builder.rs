//! The Chiyocore building system :tm: is a little weird. here's how it works:
//!
//! * **a node** -> is a standalone meshcore entity, with its own identity and independent set of handler layers
//! * **BuildChiyocoreLayer** -> takes some configuration input and builds a layer that is supposed to be attached to a certain node. returns a locked/arc'd version of said layer
//! * **BuildChiyocoreSet** -> builds a full node, complete with a set of handler layers.

use core::marker::PhantomData;

use alloc::sync::Arc;
use chiyo_hal::meshcore::{identity::LocalIdentity, payloads::AdvertisementExtraData};
use chiyo_hal::{
    EspMutex, embassy_net, embassy_sync, embassy_time, esp_hal, esp_storage, esp_sync,
    storage::{ChiyoFilesystem, FsPartition, PersistedObject},
};
use defmt::info;
use embassy_executor::Spawner;
use embassy_sync::rwlock::RwLock;
use esp_hal::{
    aes::dma::{AesDmaBackend, AesDmaWorkQueueDriver},
    peripherals::{ADC1, AES, DMA_CH0, FLASH, LPWR, RNG, RSA, SHA, WIFI},
    rng::TrngSource,
    rsa::{RsaBackend, RsaWorkQueueDriver},
    rtc_cntl::Rtc,
    sha::{ShaBackend, ShaWorkQueueDriver},
};
use esp_storage::FlashStorage;
use esp_sync::NonReentrantMutex;
use ouroboros::self_referencing;
use thingbuf::{mpsc::StaticReceiver, recycling::WithCapacity};

use crate::{
    DataWithSnr,
    lora::{LoraPins, LoraTaskChannel, TaskChannelHandler},
    partition_table,
    simple_mesh::{
        MeshLayerGet, SimpleMesh,
        storage::{MeshStorage, message_log::MessageLog},
    },
};

pub trait BuildChiyocoreLayer {
    type Input;
    type Output: MeshLayerGet + 'static;

    fn build<T: 'static>(
        spawner: &Spawner,
        chiyocore: &Chiyocore<T, ChiyocoreSetupData>,
        mesh: &Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>,
        config: &Self::Input,
    ) -> impl Future<Output = Self::Output>;
}

pub trait BuildChiyocoreSet {
    type Input;
    type Output: TaskChannelHandler + 'static;

    fn build_handler<T: 'static>(
        self,
        spawner: &Spawner,
        chiyocore: &Chiyocore<T, ChiyocoreSetupData>,
        config: &Self::Input,
    ) -> impl Future<Output = Self::Output>;
}

pub struct ChiyocoreNode<L: BuildChiyocoreLayer>(
    LocalIdentity,
    PersistedObject<AdvertisementExtraData<'static>>,
    PhantomData<L>,
);

impl<L: BuildChiyocoreLayer> ChiyocoreNode<L> {
    pub fn new(
        identity: LocalIdentity,
        advert: PersistedObject<AdvertisementExtraData<'static>>,
    ) -> Self {
        ChiyocoreNode(identity, advert, PhantomData)
    }
}

impl<L: BuildChiyocoreLayer> BuildChiyocoreSet for ChiyocoreNode<L> {
    type Input = L::Input;
    type Output = (Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>, L::Output);

    async fn build_handler<T: 'static>(
        self,
        spawner: &Spawner,
        chiyocore: &Chiyocore<T, ChiyocoreSetupData>,
        config: &Self::Input,
    ) -> Self::Output {
        let mesh = Arc::new(RwLock::new(SimpleMesh::new(
            self.0,
            chiyocore.mesh_storage().clone(),
            chiyocore.lora_channel.clone(),
            chiyocore.rtc(),
            self.1,
        )));
        // spawner.spawn(heap_log(Arc::clone(&mesh)).unwrap());
        let layers = L::build(spawner, chiyocore, &mesh, config).await;
        (mesh, layers)
    }
}

/// Setup data that will be discarded before the run() function
/// (useful for non-send items, such as embassy_net::Stack)
pub struct ChiyocoreSetupData {
    pub net_stack: Option<embassy_net::Stack<'static>>,
}

/// A constructed instance of a Chiyocore firmware.
pub struct Chiyocore<L: 'static, D> {
    pub base_peripherals: ChiyocorePeripheralSet,
    pub fs: ChiyocoreFs,
    pub lora_channel: LoraTaskChannel,
    pub lora_rx: StaticReceiver<DataWithSnr, WithCapacity>,
    pub setup_data: D,
    hosted_node: Option<L>,
}

#[embassy_executor::task]
async fn heap_log(mesh: Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>) {
    loop {
        info!(
            "simplemesh scratch use: {} bytes",
            mesh.read().await.scratch.allocated_bytes()
        );
        embassy_time::Timer::after_secs(120).await;
    }
}

impl<T: 'static> Chiyocore<T, ChiyocoreSetupData> {
    pub async fn setup<L: LoraPins>(
        spawner: &Spawner,
        peripherals: ChiyocorePeripherals,
        lora_pins: L,
    ) -> Chiyocore<T, ChiyocoreSetupData> {
        let peripherals = ChiyocorePeripheralSet::build(peripherals);
        let fs = ChiyocoreFs::new(&peripherals.flash).await;

        let lora = crate::lora::lora_init(lora_pins).await;
        let (lora_app_channel, lora_rx) = LoraTaskChannel::start_lora(lora, spawner);

        Chiyocore {
            base_peripherals: peripherals,
            fs,
            lora_channel: lora_app_channel,
            lora_rx,
            hosted_node: None,
            setup_data: ChiyocoreSetupData { net_stack: None },
        }
    }

    /// Starts wifi in client mode and constructs an embassy network stack.
    pub async fn add_network(
        &mut self,
        spawner: &embassy_executor::Spawner,
        wifi: WIFI<'static>,
        ssid: &str,
        password: &str,
    ) -> embassy_net::Stack<'static> {
        let net_stack = crate::wifi::wifi_init(wifi, spawner, ssid.into(), password.into()).await;

        spawner.spawn(
            crate::ntp::ntp_task(net_stack, Arc::clone(&self.base_peripherals.rtc)).unwrap(),
        );

        self.setup_data.net_stack = Some(net_stack);

        net_stack
    }

    pub fn rtc(&self) -> &Arc<Rtc<'static>> {
        &self.base_peripherals.rtc
    }

    pub fn main_fs(&self) -> &ChiyoFilesystem {
        &self.fs.main_fs
    }

    pub fn mesh_storage(&self) -> &MeshStorage {
        &self.fs.mesh_storage
    }

    pub fn log_fs(&self) -> &Arc<EspMutex<MessageLog>> {
        &self.fs.log_fs
    }

    /// Adds a set of nodes to this Chiyocore instance.
    pub async fn add_node<B: BuildChiyocoreSet>(
        self,
        spawner: &Spawner,
        builder: B,
        config: &B::Input,
    ) -> Chiyocore<B::Output, ChiyocoreSetupData> {
        let handler = builder.build_handler(spawner, &self, config).await;

        let Chiyocore {
            base_peripherals,
            fs,
            lora_channel,
            lora_rx,
            setup_data,
            hosted_node: _,
        } = self;

        Chiyocore {
            base_peripherals,
            fs,
            lora_channel,
            lora_rx,
            setup_data,
            hosted_node: Some(handler),
        }
    }

    /// Ejects setup data and makes this instance ready to be run.
    pub fn build(self) -> Chiyocore<T, ()> {
        let Chiyocore {
            base_peripherals,
            fs,
            lora_channel,
            lora_rx,
            setup_data: _,
            hosted_node,
        } = self;
        Chiyocore {
            base_peripherals,
            fs,
            lora_channel,
            lora_rx,
            setup_data: (),
            hosted_node,
        }
    }
}

impl<L: TaskChannelHandler + 'static> Chiyocore<L, ()> {
    pub async fn run(self) {
        let hosted_node = self.hosted_node.unwrap();
        self.lora_channel
            .run_handler(self.lora_rx, hosted_node)
            .await;
    }
}

/// Required filesystems, constructed and usable for handler layers.
pub struct ChiyocoreFs {
    pub main_fs: ChiyoFilesystem,
    pub log_fs: Arc<EspMutex<MessageLog>>,
    pub mesh_storage: MeshStorage,
}

impl ChiyocoreFs {
    pub async fn new(flash: &Arc<NonReentrantMutex<FlashStorage<'static>>>) -> ChiyocoreFs {
        let main_part = FsPartition {
            storage: Arc::clone(flash),
            partition_offset: partition_table::MESHCORE_DATA.offset as usize,
            partition_size: partition_table::MESHCORE_DATA.size as usize,
        };

        let log_part = FsPartition {
            storage: Arc::clone(flash),
            partition_offset: partition_table::LOGS.offset as usize,
            partition_size: partition_table::LOGS.size as usize,
        };

        let main_fs = ChiyoFilesystem::new(main_part).await.unwrap();

        ChiyocoreFs {
            mesh_storage: MeshStorage::new(&main_fs).await,
            main_fs,
            log_fs: Arc::new(EspMutex::new(MessageLog::new(log_part))),
        }
    }
}

/// Constructed set of Chiyocore peripherals as running drivers.
pub struct ChiyocorePeripheralSet {
    pub rtc: Arc<Rtc<'static>>,
    pub sha: ShaDriver<'static>,
    pub aes: AesDriver<'static>,
    pub trng: TrngSource<'static>,
    pub rsa: RsaDriver<'static>,
    pub flash: Arc<NonReentrantMutex<FlashStorage<'static>>>,
}

impl ChiyocorePeripheralSet {
    pub fn build(
        ChiyocorePeripherals {
            lpwr,
            sha,
            aes,
            rng,
            adc,
            flash,
            dma,
            rsa,
        }: ChiyocorePeripherals,
    ) -> Self {
        ChiyocorePeripheralSet {
            rtc: Arc::new(Rtc::new(lpwr)),
            sha: ShaDriverBuilder {
                sha: ShaBackend::new(sha),
                wq_builder: |sha| sha.start(),
            }
            .build(),
            aes: AesDriverBuilder {
                aes: AesDmaBackend::new(aes, dma),
                wq_builder: |aes| aes.start(),
            }
            .build(),
            trng: TrngSource::new(rng, adc),
            flash: Arc::new(NonReentrantMutex::new(
                FlashStorage::new(flash).multicore_auto_park(),
            )),
            rsa: RsaDriverBuilder {
                rsa: RsaBackend::new(rsa),
                wq_builder: |rsa| rsa.start(),
            }
            .build(),
        }
    }
}

/// Bundle of peripherals required to run Chiyocore.
pub struct ChiyocorePeripherals {
    pub lpwr: LPWR<'static>,
    pub sha: SHA<'static>,
    pub aes: AES<'static>,
    pub rng: RNG<'static>,
    pub adc: ADC1<'static>,
    pub flash: FLASH<'static>,
    pub dma: DMA_CH0<'static>,
    pub rsa: RSA<'static>,
}

#[self_referencing]
pub struct AesDriver<'a> {
    pub aes: AesDmaBackend<'a>,
    #[borrows(mut aes)]
    #[not_covariant]
    pub wq: AesDmaWorkQueueDriver<'this, 'a>,
}

#[self_referencing]
pub struct ShaDriver<'a> {
    pub sha: ShaBackend<'a>,
    #[borrows(mut sha)]
    #[not_covariant]
    pub wq: ShaWorkQueueDriver<'this, 'a>,
}

#[self_referencing]
pub struct RsaDriver<'a> {
    pub rsa: RsaBackend<'a>,
    #[borrows(mut rsa)]
    #[not_covariant]
    pub wq: RsaWorkQueueDriver<'this, 'a>,
}

macro_rules! impl_chiyocore_layer_tuple {
    ($($var:ident),*) => {
        #[allow(non_snake_case)]
        impl <$($var),*> BuildChiyocoreLayer for ($($var),*) where $($var: BuildChiyocoreLayer + 'static),*, ($(<$var as BuildChiyocoreLayer>::Output),*): MeshLayerGet {
            type Input = ($(<$var as BuildChiyocoreLayer>::Input),*);
            type Output = ($(<$var as BuildChiyocoreLayer>::Output),*);

            async fn build<T: 'static>(
                spawner: &Spawner,
                chiyocore: &Chiyocore<T, ChiyocoreSetupData>,
                mesh: &Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>,
                config: &Self::Input,
            ) -> Self::Output {
                let ($($var),*) = config;
                ($(
                    <$var as BuildChiyocoreLayer>::build(spawner, chiyocore, mesh, $var).await
                ),*)
            }
        }

        // pub struct ChiyocoreNode<L: BuildChiyocoreLayer>(LocalIdentity, PhantomData<L>);

        impl<$($var),*> BuildChiyocoreSet for ($(ChiyocoreNode<$var>),*) where $($var: BuildChiyocoreLayer),* {
            type Input = ($(<ChiyocoreNode<$var> as BuildChiyocoreSet>::Input),*);
            type Output = ($((Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>, $var::Output)),*);

            async fn build_handler<T: 'static>(
                self,
                spawner: &Spawner,
                chiyocore: &Chiyocore<T, ChiyocoreSetupData>,
                config: &Self::Input
            ) -> Self::Output {
                let ($($var),*) = self;
                pastey::paste! {
                    let ($([<$var cfg>]),*) = config;
                }

                (
                    $(
                        $var.build_handler(spawner, chiyocore, pastey::paste! { [<$var cfg>]}).await
                    ),*
                )
            }
        }

    }
}

impl_chiyocore_layer_tuple!(A, B);
impl_chiyocore_layer_tuple!(A, B, C);
impl_chiyocore_layer_tuple!(A, B, C, D);
impl_chiyocore_layer_tuple!(A, B, C, D, E);
