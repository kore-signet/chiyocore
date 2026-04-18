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

generate your firmware using [chiyocore-builder](https://github.com/kore-signet/chiyocore-builder)!

if you setup a companion, connect to it with meshcore-cli:
`meshcore-cli -t <board-ip-address> -p <companion-port>`

## why chiyocore?
[i think sakura chiyono o is neat](https://www.youtube.com/watch?v=e3YcYLE90po)

## development notes!

### project architecture
the main meshcore logic is written in [chiyocore](chiyocore/). the goals are that this should provide a framework that handles:
- keeping track of contacts, channels and received packets
- most packet sending logic
- most packet decoding logic
so that layers built on top of it can remain as high-level as possible.

said layers are configured by the [builder](https://github.com/kore-signet/chiyocore-builder) tool, which takes a board configuration plus a firmware setup config and generates a temporary binary crate linking all the configured handler layers together with the specified board pinout. 

example implementations of handler layers are the [companion](companion/) implementation, as well as the example [TTC bus arrival time bot](https://codeberg.org/emisignet/chiyocore-ttc).

### todos & random thoughts
- more radio support!!
- partition tables need to be configurable
- is packet delaying logic correct?
- need a reorg/cleanup pass
- stack usage could likely be improved
