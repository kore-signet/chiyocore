![chiyocore](logo.png)

an implementation of [meshcore](https://meshcore.co.uk/) for ESP32s, written in rust!

## warning labels

### radio settings are currently hardcoded
they're set to max tx/rx power and the US/Canada meshcore preset. alter as you wish/need

### be respectful to your mesh
don't spam flood packets, be careful with how you run it!

### general disclaimer
this is extremely experimental and not guaranteed to work or not fry your radio/board. i have had success running it on my xiaos3 kit, but please tread with care and do not assume things will work! 

## things you need to do to get this running
- check if the pinouts in [src/boards.rs](src/boards.rs) are correct for your board
- set the `lora_pins` variable in [src/main.rs](src/main.rs) to your board of choice
- set the wifi-ssid and wifi-password in [src/main.rs](src/main.rs)
- set all the esp32 cargo features in [Cargo.toml](Cargo.toml) to your actual board (i really need to make that easier)
- `cargo run`
- connect with the meshcore cli tool using tcp (`meshcore-cli -t <logged-ip-address-here>`)

## why chiyocore?
[i think sakura chiyono o is neat](https://www.youtube.com/watch?v=e3YcYLE90po)