use chiyo_hal::{
    EspMutex, embassy_embedded_hal, embassy_executor, embassy_futures, embassy_sync, embassy_time,
    esp_hal, esp_sync,
};
use defmt::{Debug2Format, error, info, trace};

use core::time::Duration;

use alloc::sync::Arc;
use embassy_executor::SendSpawner;
use embassy_futures::select::Either;
use embassy_sync::rwlock::RwLock;
use esp_hal::{
    gpio::{InputPin, OutputPin},
    time::Instant,
};
use lora_phy::{
    LoRa,
    mod_params::{ModulationParams, PacketParams, PacketStatus},
    mod_traits::IrqState,
};
use meshcore::{Packet, PacketHeader, PacketPayload, Path, RouteType, SerDeser};
use rand::Rng;
use static_cell::StaticCell;
use thingbuf::{mpsc::StaticChannel, recycling::DefaultRecycle};

use crate::{
    DataWithSnr, FirmwareError, FirmwareResult, MeshcoreHandler,
    simple_mesh::{MeshLayerGet, SimpleMesh, SimpleMeshLayer, WithLayer},
};

/// LoRa freq (currently hardcoded.)
pub const LORA_FREQUENCY_IN_HZ: u32 = 910_525_000; // WARNING: Set this appropriately for the region

static SPI_BUS: StaticCell<EspMutex<esp_hal::spi::master::Spi<'static, esp_hal::Async>>> =
    StaticCell::new();

/// Sx1262 radio (type alias for lora_phy)
pub type Sx1262Radio<'a> = lora_phy::sx126x::Sx126x<
    embassy_embedded_hal::shared_bus::asynch::spi::SpiDevice<
        'a,
        esp_sync::RawMutex,
        esp_hal::spi::master::Spi<'static, esp_hal::Async>,
        esp_hal::gpio::Output<'a>,
    >,
    lora_phy::iv::GenericSx126xInterfaceVariant<
        esp_hal::gpio::Output<'a>,
        esp_hal::gpio::Input<'a>,
    >,
    lora_phy::sx126x::Sx1262,
>;

/// Any object that can produce a static/owned bundle of the required pins to drive a LoRa radio (alongside an SPI driver)
pub trait LoraPins {
    type Sclk: OutputPin + 'static;
    type Mosi: OutputPin + 'static;
    type Miso: InputPin + 'static;
    type CS: OutputPin + 'static;
    type Reset: OutputPin + 'static;
    type Busy: InputPin + 'static;
    type Dio1: InputPin + 'static;
    type RxEn: OutputPin + 'static;
    type Spi: esp_hal::spi::master::Instance + 'static;

    fn into_bundle(
        self,
    ) -> LoraPinBundle<
        Self::Sclk,
        Self::Mosi,
        Self::Miso,
        Self::CS,
        Self::Reset,
        Self::Busy,
        Self::Dio1,
        Self::RxEn,
        Self::Spi,
    >;
}

/// Bundle of pins required to drive an SPI LoRa radio, alongside an SPI driver
pub struct LoraPinBundle<
    SCLK: OutputPin + 'static,
    MOSI: OutputPin + 'static,
    MISO: InputPin + 'static,
    CS: OutputPin + 'static,
    RESET: OutputPin + 'static,
    BUSY: InputPin + 'static,
    DIO1: InputPin + 'static,
    RXEN: OutputPin + 'static,
    SPI: esp_hal::spi::master::Instance + 'static,
> {
    pub sclk: SCLK,
    pub mosi: MOSI,
    pub miso: MISO,
    pub cs: CS,
    pub reset: RESET,
    pub busy: BUSY,
    pub dio1: DIO1,
    pub rx_en: Option<RXEN>,
    pub spi: SPI,
}

impl<
    SCLK: OutputPin + 'static,
    MOSI: OutputPin + 'static,
    MISO: InputPin + 'static,
    CS: OutputPin + 'static,
    RESET: OutputPin + 'static,
    BUSY: InputPin + 'static,
    DIO1: InputPin + 'static,
    RXEN: OutputPin + 'static,
    SPI: esp_hal::spi::master::Instance + 'static,
> LoraPins for LoraPinBundle<SCLK, MOSI, MISO, CS, RESET, BUSY, DIO1, RXEN, SPI>
{
    type Sclk = SCLK;
    type Mosi = MOSI;
    type Miso = MISO;
    type CS = CS;
    type Reset = RESET;
    type Busy = BUSY;
    type Dio1 = DIO1;
    type RxEn = RXEN;
    type Spi = SPI;

    fn into_bundle(
        self,
    ) -> LoraPinBundle<
        Self::Sclk,
        Self::Mosi,
        Self::Miso,
        Self::CS,
        Self::Reset,
        Self::Busy,
        Self::Dio1,
        Self::RxEn,
        Self::Spi,
    > {
        self
    }
}

/// Create and initialize a LoRa radio from a set of pins.
pub async fn lora_init<T: LoraPins>(
    pins: T,
) -> lora_phy::LoRa<Sx1262Radio<'static>, embassy_time::Delay> {
    let pins = pins.into_bundle();

    let lora_cs = esp_hal::gpio::Output::new(
        pins.cs,
        esp_hal::gpio::Level::High,
        esp_hal::gpio::OutputConfig::default(),
    );

    let reset = esp_hal::gpio::Output::new(
        pins.reset,
        esp_hal::gpio::Level::Low,
        esp_hal::gpio::OutputConfig::default(),
    );
    let busy = esp_hal::gpio::Input::new(pins.busy, esp_hal::gpio::InputConfig::default());
    let dio1 = esp_hal::gpio::Input::new(pins.dio1, esp_hal::gpio::InputConfig::default());
    let rx_en = pins.rx_en.map(|pin| esp_hal::gpio::Output::new(
        pin,
        esp_hal::gpio::Level::Low,
        esp_hal::gpio::OutputConfig::default(),
    ));

    let spi = esp_hal::spi::master::Spi::new(
        pins.spi,
        esp_hal::spi::master::Config::default()
            .with_frequency(esp_hal::time::Rate::from_khz(100))
            .with_mode(esp_hal::spi::Mode::_0),
    )
    .unwrap()
    .with_sck(pins.sclk)
    .with_mosi(pins.mosi)
    .with_miso(pins.miso)
    .into_async();

    // let out = esp_hal::gpio::Output::new(pin, initial_level, config)

    // Initialize the static SPI bus
    let spi_bus = SPI_BUS.init(embassy_sync::mutex::Mutex::new(spi));
    let spi_device =
        embassy_embedded_hal::shared_bus::asynch::spi::SpiDevice::new(spi_bus, lora_cs);

    // Create the SX126x configuration
    let sx126x_config = lora_phy::sx126x::Config {
        chip: lora_phy::sx126x::Sx1262,
        tcxo_ctrl: Some(lora_phy::sx126x::TcxoCtrlVoltage::Ctrl1V8),
        use_dcdc: true,
        rx_boost: true,
    };

    // Create the radio instance
    let iv = lora_phy::iv::GenericSx126xInterfaceVariant::new(reset, dio1, busy, rx_en, None)
        .unwrap();

    // log::info!("{}",lora.get_rssi().await.unwrap());

    lora_phy::LoRa::new(
        lora_phy::sx126x::Sx126x::new(spi_device, iv, sx126x_config),
        false,
        embassy_time::Delay,
    )
    .await
    .unwrap()
}

/// An active radio with set modulation parameters
pub struct Radio {
    lora: LoRa<Sx1262Radio<'static>, embassy_time::Delay>,
    modulation_params: ModulationParams,

    tx_params: PacketParams,
    rx_params: PacketParams,
    buf: [u8; 256],
}

impl Radio {
    pub fn new(mut lora: LoRa<Sx1262Radio<'static>, embassy_time::Delay>) -> FirmwareResult<Radio> {
        let modulation_params = lora.create_modulation_params(
            lora_phy::mod_params::SpreadingFactor::_7,
            lora_phy::mod_params::Bandwidth::_62KHz,
            lora_phy::mod_params::CodingRate::_4_5,
            LORA_FREQUENCY_IN_HZ,
        )?;

        let rx_params =
            lora.create_rx_packet_params(8, false, 255, true, false, &modulation_params)?;

        let tx_params = lora.create_tx_packet_params(8, false, true, false, &modulation_params)?;

        Ok(Radio {
            lora,
            modulation_params,
            tx_params,
            rx_params,
            buf: [0u8; 256],
        })
    }

    async fn cad_check(&mut self) -> FirmwareResult<()> {
        let mut tries: u64 = 0;
        //https://github.com/rightup/pyMC_core/blob/ba3ff4c26b9fadda1995c1ef6e21c7f143768c8d/src/pymc_core/hardware/sx1262_wrapper.py#L828
        while tries < 5 {
            self.lora.prepare_for_cad(&self.modulation_params).await?;
            if self.lora.cad(&self.modulation_params).await? {
                info!("\tcad attempt {} | failed, waiting", tries);
                tries += 1;

                let jitter = esp_hal::rng::Rng::new().random_range(50..200);
                let backoff_ms = jitter * (tries - 1).pow(2);

                embassy_time::Timer::after_millis(backoff_ms.min(5000)).await;
            } else {
                info!("\tcad: sucess, doing send");
                return Ok(());
            }
        }

        Ok(())
    }

    async fn prepare_for_rx(&mut self) -> FirmwareResult<()> {
        self.lora
            .prepare_for_rx(
                lora_phy::RxMode::Continuous,
                &self.modulation_params,
                &self.rx_params,
            )
            .await
            .map_err(FirmwareError::LoRa)
    }

    async fn tx(&mut self, data: &[u8]) -> FirmwareResult<()> {
        self.cad_check().await?;

        self.lora
            .prepare_for_tx(&self.modulation_params, &mut self.tx_params, 22, data)
            .await?;
        self.lora.tx().await?;
        Ok(())
    }

    /// Loop receiving messages, as well as sending when needed (to_send)
    async fn run(
        &mut self,
        received_tx: &mut thingbuf::mpsc::StaticSender<DataWithSnr, DefaultRecycle>,
        to_send: &mut thingbuf::mpsc::StaticReceiver<heapless::Vec<u8, 256>, DefaultRecycle>,
    ) -> FirmwareResult<()> {
        self.prepare_for_rx().await?;
        self.lora.start_rx().await?;
        let mut needs_rx_restart = false;
        loop {
            if let Ok(to_send) = to_send.try_recv_ref() {
                info!("lora tx");
                self.tx(&to_send).await?;
                needs_rx_restart = true;
                continue;
            };

            if needs_rx_restart {
                self.buf.fill(0);
                self.prepare_for_rx().await?;
                self.lora.start_rx().await?;
            }

            // todo: i think it can get stuck here
            match self.lora.process_irq_event().await {
                Ok(Some(IrqState::PreambleReceived)) => {
                    needs_rx_restart = false;
                }
                Ok(Some(IrqState::Done)) => {
                    let (received_len, received_status) = self
                        .lora
                        .get_rx_result(&self.rx_params, &mut self.buf)
                        .await?;
                    let received = &self.buf[..received_len as usize];
                    let mut tx_slot = received_tx.send_ref().await.unwrap(); // todo no unwrappin
                    let _ = tx_slot.0.extend_from_slice(received);
                    tx_slot.1 = received_status;
                    drop(tx_slot);

                    self.lora.clear_irq_status().await?;

                    needs_rx_restart = true;
                }
                Ok(None) => {
                    needs_rx_restart = false;
                }
                Err(err) => {
                    error!("radio error: {:?}", Debug2Format(&err));
                    return Err(err.into());
                }
            }

            match embassy_futures::select::select(
                embassy_time::with_timeout(
                    embassy_time::Duration::from_millis(300),
                    self.lora.wait_for_irq(),
                ),
                to_send.recv_ref(),
            )
            .await
            {
                Either::First(_res) => {
                    // res?;
                }
                Either::Second(Some(to_send)) => {
                    // self.lora.
                    info!("lora tx");
                    self.tx(&to_send).await?;
                    needs_rx_restart = true;
                }
                Either::Second(None) => {}
            }
        }
    }
}
#[embassy_executor::task]
pub async fn lora_task(
    mut radio: Radio,
    mut received: thingbuf::mpsc::StaticSender<DataWithSnr, DefaultRecycle>,
    mut to_send: thingbuf::mpsc::StaticReceiver<heapless::Vec<u8, 256>, DefaultRecycle>,
) {
    loop {
        if let Err(e) = radio.run(&mut received, &mut to_send).await {
            error!("lora task crashed: {:?}. restarting", e);
        }
    }
}

static LORA_TRANSMIT_CHANNEL: StaticChannel<heapless::Vec<u8, 256>, 16> = StaticChannel::new();
static LORA_RECEIVE_CHANNEL: StaticChannel<DataWithSnr, 16> = StaticChannel::new();

#[embassy_executor::task(pool_size = 16)]
async fn send_delayed(
    tx: thingbuf::mpsc::StaticSender<heapless::Vec<u8, 256>>,
    packet: heapless::Vec<u8, 256>,
    delay: Duration,
) {
    embassy_time::Timer::after(embassy_time::Duration::from_micros(delay.as_micros() as u64)).await;
    let Ok(mut slot) = tx.send_ref().await else {
        return;
    };
    *slot = packet;
    drop(slot);
}

/// Handle to transmit raw byte packets to be sent by the running LoRa radio.
#[derive(Clone)]
pub struct LoraTaskChannel {
    // scratch: [u8; 256],
    pub tx: thingbuf::mpsc::StaticSender<heapless::Vec<u8, 256>, DefaultRecycle>,
}

impl LoraTaskChannel {
    /// Start LoRa receive/transmit task.
    pub fn start_lora(
        lora: LoRa<Sx1262Radio<'static>, embassy_time::Delay>,
        spawner: &embassy_executor::Spawner,
    ) -> (
        Self,
        thingbuf::mpsc::StaticReceiver<DataWithSnr, DefaultRecycle>,
    ) {
        let (receive_tx, receive_rx) = LORA_RECEIVE_CHANNEL.split();
        let (transmit_tx, transmit_rx) = LORA_TRANSMIT_CHANNEL.split();
        let radio = Radio::new(lora).unwrap();

        spawner.spawn(lora_task(radio, receive_tx, transmit_rx).unwrap());
        (
            LoraTaskChannel {
                // scratch: [0u8; 256],
                tx: transmit_tx,
                // rx: receive_rx,
            },
            receive_rx,
        )
    }

    /// Run the specified set of handlers using the passed receive channel handle.
    pub async fn run_handler<L: TaskChannelHandler>(
        self,
        rx: thingbuf::mpsc::StaticReceiver<DataWithSnr, DefaultRecycle>,
        handler: L,
    ) {
        while let Some(rx_slot) = rx.recv_ref().await {
            let start = Instant::now();
            let Ok(packet) = meshcore::Packet::decode(&rx_slot.0) else {
                continue;
            };

            // let ctx = PacketContext {
            //     packet: &packet,
            //     packet_status: rx_slot.1,
            //     bytes: &rx_slot.0[..],
            //     handler: &handler,
            // };

            // layer.with_layer(ctx).await;
            handler.run(&packet, rx_slot.1, &rx_slot.0[..]).await;

            info!("packet took: {}", start.elapsed());
        }
    }

    // TODO: support delaying
    pub async fn send_packet(&self, packet: &Packet<'_>) -> Duration {
        info!(
            "tx | {} | {} (path: {})",
            packet.header.payload_type(),
            packet.header.route_type(),
            packet.path
        );

        let mut slot = self.tx.send_ref().await.unwrap();

        let timeout = packet.timeout_est(&packet.path, packet.header.route_type());

        Packet::encode_into_vec(packet, &mut *slot).unwrap();
        timeout
    }

    // returns est airtime
    pub async fn send_direct<P: PacketPayload>(
        &self,
        payload: &P::Representation<'_>,
        path: Path<'_>,
    ) -> Duration {
        info!(
            "tx | {} | direct (path: {})",
            P::PAYLOAD_TYPE,
            path
        );

        let mut scratch = [0u8; 256];

        let payload_buf = P::encode(payload, &mut scratch).unwrap();
        let packet = Packet {
            header: PacketHeader::new()
                .with_payload_type(P::PAYLOAD_TYPE)
                .with_route_type(RouteType::Direct),
            transport_codes: None,
            path,
            payload: payload_buf.into(),
            // rssi: None,
            // snr: None,
        };

        let timeout = packet.timeout_est(&packet.path, RouteType::Direct);
        let mut slot = self.tx.send_ref().await.unwrap();
        Packet::encode_into_vec(&packet, &mut *slot).unwrap();
        timeout
    }

    // returns est airtime
    pub async fn send_flood<P: PacketPayload>(
        &self,
        payload: &P::Representation<'_>,
        path: Path<'_>,
    ) -> Duration {
        info!(
            "tx | {} | flood (path: {})",
            P::PAYLOAD_TYPE,
            path
        );
        let mut scratch = [0u8; 256]; // todo: is this better than alloc'ing :?
        let payload_buf = P::encode(payload, &mut scratch).unwrap();
        let packet = Packet {
            header: PacketHeader::new()
                .with_payload_type(P::PAYLOAD_TYPE)
                .with_route_type(RouteType::Flood),
            transport_codes: None,
            path,
            payload: payload_buf.into(),
            // rssi: None,
            // snr: None,
        };
        let timeout = packet.timeout_est(&packet.path, RouteType::Flood);
        let mut slot = self.tx.send_ref().await.unwrap();
        Packet::encode_into_vec(&packet, &mut *slot).unwrap();
        timeout
    }

    pub async fn send_delayed(&self, packet: &Packet<'_>, delay: Duration) -> Duration {
        let airtime = packet.timeout_est(&packet.path, packet.header.route_type());
        let mut out = heapless::Vec::new();
        let _ = Packet::encode_into_vec(packet, &mut out);

        trace!("sending packet with delay: {}ms", delay.as_millis() as u64);

        SendSpawner::for_current_executor()
            .await
            .spawn(send_delayed(self.tx.clone(), out, delay).unwrap());

        airtime
    }
}

struct PacketContext<'a, 'b, 'h> {
    packet: &'a Packet<'b>,
    packet_status: PacketStatus,
    bytes: &'b [u8],
    handler: &'h Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>,
}

impl<'a, 'b, 'h> WithLayer for PacketContext<'a, 'b, 'h> {
    async fn call_me(&self, mut layer: impl SimpleMeshLayer) {
        let mut handler = self.handler.write().await;
        if let Err(e) = handler
            .packet(self.packet, self.packet_status, self.bytes, &mut layer)
            .await
        {
            error!("handler error: {:?}", e);
        }
    }
}

/// A handler (or set of handlers) that can handle packets as they're recieved.
pub trait TaskChannelHandler {
    fn run<'b>(
        &self,
        packet: &Packet<'b>,
        packet_status: PacketStatus,
        bytes: &'b [u8],
    ) -> impl Future<Output = ()>;
}

impl<L: MeshLayerGet> TaskChannelHandler for (Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>, L) {
    #[allow(clippy::needless_lifetimes)]
    async fn run<'a, 'b>(
        &self,
        packet: &'a Packet<'b>,
        packet_status: PacketStatus,
        bytes: &'b [u8],
    ) {
        let ctx = PacketContext {
            packet,
            packet_status,
            bytes,
            handler: &self.0,
        };

        self.1.with_layer(ctx).await;
    }
}

macro_rules! impl_mesh_layer_tuple {
    ($join:path; $($var:ident),*) => {
        #[allow(non_snake_case, clippy::needless_lifetimes)]
        impl<$($var),*> TaskChannelHandler for ($((Arc<RwLock<esp_sync::RawMutex, SimpleMesh>>, $var)),*) where $($var: MeshLayerGet),* {
            async fn run<'a, 'b>(&self, packet: &'a Packet<'b>, packet_status: PacketStatus, bytes: &'b [u8]) {
                let ($($var),*) = self;

                $join(
                    $(
                        $var.run(packet, packet_status,bytes)
                    ),*
                ).await;
            }
        }
    }
}

impl_mesh_layer_tuple!(
    embassy_futures::join::join;
    A,B
);
impl_mesh_layer_tuple!(
    embassy_futures::join::join3;
    A,B,C
);
impl_mesh_layer_tuple!(
    embassy_futures::join::join4;
    A,B,C,D
);
impl_mesh_layer_tuple!(
    embassy_futures::join::join5;
    A,B,C,D,E
);
