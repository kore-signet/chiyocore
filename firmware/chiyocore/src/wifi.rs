///! ESP32 wifi boilerplate
use alloc::string::String;
use embassy_executor::Spawner;
use embassy_time::Duration;

use embassy_net::Runner;
use embassy_time::Timer;
use esp_hal::peripherals::WIFI;
use esp_println::println;
use esp_radio::wifi::{ControllerConfig, Interface, WifiController, sta::StationConfig};

use crate::mk_static;

#[embassy_executor::task]
pub async fn connection(mut controller: WifiController<'static>) {
    println!("start connection task");

    loop {
        println!("About to connect...");

        match controller.connect_async().await {
            Ok(info) => {
                println!("Wifi connected to {:?}", info);

                // wait until we're no longer connected
                let info = controller.wait_for_disconnect_async().await.ok();
                println!("Disconnected: {:?}", info);
            }
            Err(e) => {
                println!("Failed to connect to wifi: {e:?}");
            }
        }

        Timer::after(Duration::from_millis(5000)).await
    }
}

#[embassy_executor::task]
pub async fn net_task(mut runner: Runner<'static, Interface<'static>>) {
    runner.run().await
}

pub async fn wifi_init(
    wifi: WIFI<'static>,
    spawner: &Spawner,
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
        ControllerConfig::default()
            .with_initial_config(station_config)
            .with_static_rx_buf_num(5),
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
            embassy_net::StackResources<8>,
            embassy_net::StackResources::<8>::new()
        ),
        seed,
    );

    spawner.spawn(connection(controller)).ok();
    spawner.spawn(net_task(runner)).ok();

    stack.wait_config_up().await;

    stack
}
