///! ESP32 wifi boilerplate
use alloc::string::String;
use chiyo_hal::{
    embassy_futures::{self, select::Either},
    embassy_net, embassy_time, esp_hal,
    esp_radio::{self, wifi::CountryInfo},
};
use embassy_executor::Spawner;
use embassy_time::Duration;

use chiyo_hal::esp_println::println;
use embassy_net::Runner;
use embassy_time::Timer;
use esp_hal::peripherals::WIFI;
use esp_radio::wifi::{ControllerConfig, Interface, WifiController, sta::StationConfig};

use crate::mk_static;

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    println!("start connection task");

    loop {
        match controller.connect_async().await {
            Ok(_) => {
                // wait until we're no longer connected
                loop {
                    let info = embassy_futures::select::select(
                        controller.wait_for_disconnect_async(),
                        controller.wait_for_access_point_connected_event_async(),
                    )
                    .await;

                    match info {
                        Either::First(station_disconnected) => {
                            if let Ok(station_disconnected) = station_disconnected {
                                println!("Station disconnected: {:?}", station_disconnected);
                                break;
                            }
                        }
                        Either::Second(event) => {
                            if let Ok(event) = event {
                                match event {
                                    esp_radio::wifi::AccessPointStationEventInfo::Connected(
                                        access_point_station_connected_info,
                                    ) => {
                                        println!(
                                            "Station connected: {:?}",
                                            access_point_station_connected_info
                                        );
                                    }
                                    esp_radio::wifi::AccessPointStationEventInfo::Disconnected(
                                        access_point_station_disconnected_info,
                                    ) => {
                                        println!(
                                            "Station disconnected: {:?}",
                                            access_point_station_disconnected_info
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                println!("Failed to connect to wifi: {e:?}");
                Timer::after(Duration::from_millis(5000)).await
            }
        }
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
    let (mut controller, interfaces) = esp_radio::wifi::new(
        wifi,
        ControllerConfig::default()
            .with_initial_config(station_config)
            .with_ampdu_rx_enable(true)
            .with_ampdu_tx_enable(true)
            .with_country_info([b'C', b'A']), // .with_static_rx_buf_num(5),
    )
    .unwrap();
    println!("Wifi configured and started!");
    controller.set_max_tx_power(84);
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

    spawner.spawn(connection(controller).unwrap());
    spawner.spawn(net_task(runner).unwrap());

    stack.wait_config_up().await;

    stack
}
