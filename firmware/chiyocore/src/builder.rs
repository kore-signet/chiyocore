use core::marker::PhantomData;

use alloc::sync::Arc;
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
use meshcore::identity::LocalIdentity;
use ouroboros::self_referencing;
use thingbuf::mpsc::StaticReceiver;

use crate::{
    DataWithSnr, EspMutex,
    lora::{LoraPins, LoraTaskChannel, TaskChannelHandler},
    message_log::MessageLog,
    partition_table,
    simple_mesh::{MeshLayerGet, SimpleMesh, storage::MeshStorage},
    storage::{ActiveFilesystem, FsPartition, SimpleFileDb},
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

pub struct ChiyocoreNode<L: BuildChiyocoreLayer>(LocalIdentity, PhantomData<L>);

impl<L: BuildChiyocoreLayer> ChiyocoreNode<L> {
    pub fn new(identity: LocalIdentity) -> Self {
        ChiyocoreNode(identity, PhantomData)
    }
}

impl<L: BuildChiyocoreLayer> BuildChiyocoreSet for ChiyocoreNode<L> {
    type Input = L::Input;
    type Output = (Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>, L::Output);

    async fn build_handler<T: 'static>(
        self,
        spawner: &Spawner,
        chiyocore: &Chiyocore<T, ChiyocoreSetupData>,
        config: &L::Input,
    ) -> Self::Output {
        let mesh = Arc::new(RwLock::new(SimpleMesh::new(
            self.0,
            chiyocore.mesh_storage().clone(),
            chiyocore.lora_channel.clone(),
            chiyocore.rtc(),
        )));
        let layers = L::build(spawner, chiyocore, &mesh, config).await;
        (mesh, layers)
    }
}

pub struct ChiyocoreSetupData {
    pub net_stack: Option<embassy_net::Stack<'static>>,
}

pub struct Chiyocore<L: 'static, D> {
    pub base_peripherals: ChiyocorePeripheralSet,
    pub fs: ChiyocoreFs,
    pub lora_channel: LoraTaskChannel,
    pub lora_rx: StaticReceiver<DataWithSnr>,
    pub setup_data: D,
    hosted_node: Option<L>, // hosted_nodes: Vec<Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>>
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

    pub async fn add_network(
        &mut self,
        spawner: &embassy_executor::Spawner,
        wifi: WIFI<'static>,
        ssid: &str,
        password: &str,
    ) -> embassy_net::Stack<'static> {
        let net_stack = crate::wifi::wifi_init(wifi, spawner, ssid.into(), password.into()).await;

        spawner
            .spawn(crate::ntp::ntp_task(
                net_stack,
                Arc::clone(&self.base_peripherals.rtc),
            ))
            .unwrap();

        self.setup_data.net_stack = Some(net_stack);

        net_stack
    }

    pub fn rtc(&self) -> &Arc<Rtc<'static>> {
        &self.base_peripherals.rtc
    }

    pub fn main_fs(
        &self,
    ) -> &Arc<EspMutex<ActiveFilesystem<{ partition_table::MESHCORE_DATA.size as usize }>>> {
        &self.fs.main_fs
    }

    pub fn config_db(&self) -> &SimpleFileDb<{ partition_table::MESHCORE_DATA.size as usize }> {
        &self.fs.config_db
    }

    pub fn mesh_storage(&self) -> &MeshStorage {
        &self.fs.mesh_storage
    }

    pub fn log_fs(&self) -> &Arc<EspMutex<MessageLog>> {
        &self.fs.log_fs
    }

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

pub struct ChiyocoreFs {
    pub main_fs: Arc<EspMutex<ActiveFilesystem<{ partition_table::MESHCORE_DATA.size as usize }>>>,
    pub log_fs: Arc<EspMutex<MessageLog>>,
    pub mesh_storage: MeshStorage,
    pub config_db: SimpleFileDb<{ partition_table::MESHCORE_DATA.size as usize }>,
}

impl ChiyocoreFs {
    pub async fn new(flash: &Arc<NonReentrantMutex<FlashStorage<'static>>>) -> ChiyocoreFs {
        let main_part = FsPartition {
            storage: Arc::clone(flash),
            partition_offset: partition_table::MESHCORE_DATA.offset as usize,
        };

        let log_part = FsPartition {
            storage: Arc::clone(flash),
            partition_offset: partition_table::LOGS.offset as usize,
        };

        let main_fs = Arc::new(EspMutex::new(ActiveFilesystem::build(main_part)));

        ChiyocoreFs {
            config_db: SimpleFileDb::new(Arc::clone(&main_fs), littlefs2::path!("/globalconf/"))
                .await,
            mesh_storage: MeshStorage::new(&main_fs).await,
            main_fs,
            log_fs: Arc::new(EspMutex::new(MessageLog::new(log_part))),
        }
    }
}

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
            type Input = ($(<$var as BuildChiyocoreLayer>::Input),*);
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
