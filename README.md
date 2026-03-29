![chiyocore](logo.png)

an implementation of [meshcore](https://meshcore.co.uk/) for ESP32s, written in rust!

## warning labels

### radio settings are currently hardcoded
they're set to max tx/rx power and the US/Canada meshcore preset. alter as you wish/need

### be respectful to your mesh
don't spam flood packets, be careful with how you run it!

### general disclaimer
this is extremely experimental and not guaranteed to work or not fry your radio/board. i have had success running it on my xiaos3 kit, but please tread with care and do not assume things will work! 

## how to get it running

### set up a board definition
you'll need to set up a board definition file with the pin-out for the LoRa module, plus ram size for heap allocations. [you can see an example here](board-defs/xiao-s3-kit.toml)

### configure your firmware build
the firmware is generated off of the firmware.toml file. currently, only companion nodes accessible over wifi are supported, plus a simple ping bot.

 here's how it works:
```toml
app_stack_size = 32768 # this is how much stack memory the node handler will have
board = "xiao-s3-kit" # your board definition (stored in board-defs/)

[global_conf.wifi] # wifi config
ssid = "your-ssid-here"
pw = "your-wifi-pw-here"

[[nodes]] # define the first node
slot = "companion-0" # slot for this node's identity keys
layers = ["CompanionBuilder<0, 5000>"] # this node will have one Companion handler, running on slot 0 and listening on port 5000

[[nodes]] # second node
slot = "companion-1"
layers = ["CompanionBuilder<1, 3000>", "PingBot"] # one Companion handler, running on slot 1 and listening on port 3000, plus a simple ping bot
```

### run it
flash the firmware using `cargo run --release`, then connect to your node using a meshcore companion client. the firmware will print the board's ip address to the terminal as it boots up!

with meshcore-cli:
`meshcore-cli -t <board-ip-address> -p <companion-port>`

## why chiyocore?
[i think sakura chiyono o is neat](https://www.youtube.com/watch?v=e3YcYLE90po)