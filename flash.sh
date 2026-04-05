#!/bin/bash
./build.sh $1 $2
espflash flash --monitor --chip esp32s3 --partition-table firmware/partitions.csv firmware/target/xtensa-esp32s3-none-elf/release/chiyocore