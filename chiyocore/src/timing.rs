use core::time::Duration;

use chiyo_hal::esp_hal;
use meshcore::{Packet, SerDeser, timing::AirtimeEstConfig};

const DIRECT_TX_DELAY: Duration = Duration::from_millis(500);
const FLOOD_TX_DELAY_FACTOR: f32 = 1.0;

// based on https://github.com/rightup/pyMC_Repeater/blob/3e47122daee85734d323009d67f606257828698f/repeater/engine.py#L546
pub fn rx_retransmit_delay(rxd_packet: &Packet<'_>) -> Duration {
    let base_airtime = meshcore::timing::estimate_airtime(
        Packet::encode_size(rxd_packet) as i32,
        &AirtimeEstConfig {
            spreading_factor: 7,
            bandwidth: 62500,
            coding_rate: 5,
            preamble_length: 8,
        },
    );

    let airtime_ms: f32 = base_airtime.as_millis() as f32;
    if rxd_packet.header.route_type().is_flood() {
        let base_delay_ms = (airtime_ms * 1.04) / 2.0;
        let random_mult = rand::Rng::random_range(&mut esp_hal::rng::Rng::new(), 1.0..5.0);
        let delay_ms = base_delay_ms * random_mult * FLOOD_TX_DELAY_FACTOR;
        Duration::from_millis(u64::min(5000, delay_ms as u64))
    } else {
        DIRECT_TX_DELAY
    }
}
