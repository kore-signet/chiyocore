![chiyocore](logo.png)

an implementation of [meshcore](https://meshcore.co.uk/) for ESP32s, written in rust!

# chiyocore-builder
this is *not* the main firmware code - that lives [over here!](https://github.com/kore-signet/chiyocore).

this is the chiyocore-builder, which automagically puts together a chiyocore firmware build with your selection of modules, for your preferred board!

### general disclaimer
this is extremely experimental and not guaranteed to work or not fry your radio/board. i have had success running it on my xiaos3 kit, but please tread with care and do not assume things will work! 

# how the builder works

a chiyocore build is made up of a stackup of a number of nodes (meshcore entities with their own key-pair), each of which has a set of modules or layers attached to it.

these layers are what actually implement behavior - e.g, as a companion node, or as a repeater, etc.

## so, you'll need:

### a board definition
this lays out the pin-out of the LoRa module, as well as how much RAM your board has.

for example, the setup for a heltecv4 board might look like this:

```toml
[ram]
reclaimed = "73744" # this is ram reclaimed from the esp-idf bootloader!
main = "1024 * 32"
psram_mode = "quad"

[pins]
sclk = "GPIO7"
mosi = "GPIO9"
miso = "GPIO8"
cs = "GPIO41"
reset = "GPIO42"
busy = "GPIO40"
dio1 = "GPIO39"
rx_en = "GPIO38"
spi = "SPI2"
```

### a firmware build config

this sets up how many nodes you'll have running on your board, and what handlers/layers each should have. a couple kinds of handler are built-in, but you can also add other crates with their own modules!

```toml
[firmware]
stack_size = 32768 # how large the stack for the node-running task should be

[chiyocore]
config = { "wifi.pw" = "nya", "wifi.ssid" = "nya" } # default config parameters, if not already set
default_channels = ["#test", "#emitestcorner"] # default meshcore channels to setup keys for, alongside the public channel

# set up one node with id chiyo0
[stackup.chiyo0]
id = "chiyo0" # node id

# add a companion to that node
[stackup.chiyo0.layers.companion-0]
type = "chiyocore_companion::companionv2::Companion" # the rust type of the handler
id = "chiyocompanion0"
tcp_port = 5000 # tcp port for the companion to listen on

# add a ping bot
[stackup.chiyo0.layers.ping_bot]
type = "chiyocore::ping_bot::PingBot" 
name = "cafe / chiyobot 🌃☕" # what name should the bot use when answering pings?
channels = ["#test", "#emitestcorner"] # channels for the bot to be active in

# add a bot from another crate
[stackup.chiyo0.layers.ttc_bot]
deps = { chiyocore-ttc = { git = "https://codeberg.org/emisignet/chiyocore-ttc.git" } } # this is just a set of regular cargo dependencies! you can import them from git, local path, crates.io, etc.
type = "chiyocore_ttc::TTCBot" # type of handler
```

### generate your firmware
`cargo run -- --firmware blossoms/setups/<yr-firmware>.toml --board blossoms/boards/<yr-board>.toml --out generated-firmware`

### run it!
`cd generated-firmware`
`DEFMT_LOG=info cargo run`

### if you're running a companion, connect to it:

with meshcore-cli:
`meshcore-cli -t <board-ip-address> -p <companion-port>`

## why chiyocore?
[i think sakura chiyono o is neat](https://www.youtube.com/watch?v=e3YcYLE90po)
