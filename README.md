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
you'll need to set up a board definition file with the pin-out for the LoRa module, plus ram size for heap allocations. [you can see an example here](blossoms/boards/xiao-s3-kit.toml)

### configure your firmware build
the firmware is generated off a Rusty Object Notation (RON) file. [here's an example](blossoms/setups/sample.ron) for a board running two companions on different tcp ports, plus a ping bot.

```ron 4
#![enable(unwrap_variant_newtypes)]

ChiyocoreConfig(
  firmware: (
    stack_size: 32768,
    config: {
      "wifi.ssid": "nya",
      "wifi.pw": "nya"
    },
    default_channels: ["#test", "#emitestcorner", "#wardriving"]
  ),
  nodes: [
    Node(
      id: "chiyo0",
      layers: [
        Companion(
          id: "companion-0",
          tcp_port: 5000
        ),
        PingBot(
          name: "cafe / chiyobot 🌃☕",
          channels: [
            "#test",
            "#emitestcorner"
          ]
        )
      ]
    ),
    Node(
      id: "chiyo1",
      layers: [
        Companion(
          id: "companion-1",
          tcp_port: 3000
        )
      ]
    )
  ]
)
```

### run it
`./flash.sh blossoms/boards/<your-board-here.toml> blossoms/setups/<your-setup-here.ron>`


with meshcore-cli:
`meshcore-cli -t <board-ip-address> -p <companion-port>`

## why chiyocore?
[i think sakura chiyono o is neat](https://www.youtube.com/watch?v=e3YcYLE90po)