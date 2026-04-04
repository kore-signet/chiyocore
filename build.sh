#!/bin/bash
cd builder
cargo build 
cd ..
./builder/target/debug/chiyocore-builder --firmware $2 --board $1 --out firmware/firmware
cd firmware
ESP_LOG=chiyocore=trace,chiyocore-ttc=trace,chiyocore-companion=trace,chiyocore-firmware=trace,lora-phy=trace cargo build --bin chiyocore 
cd ..