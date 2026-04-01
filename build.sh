#!/bin/bash
cd builder
cargo build 
cd ..
./builder/target/debug/chiyocore-builder --firmware $2 --board $1 --out firmware/firmware