#!/bin/bash

PARSED_ARGS=$(getopt -n build.sh -o hl:b:s: --long help,log-level:,board:,setup:,release -- "$@")
VALID_ARGS=$?

if [ "$VALID_ARGS" != "0" ]; then
    usage
    exit 2
fi

LOG_LEVEL=info
RELEASE=0
BOARD=unset
SETUP=unset

usage()
{
    echo "Usage: ./build.sh 
                    [ --release ] 
                    [ -l | --log-level LOG_LEVEL ]
                    [ -b | --board BOARD ]
                    [ -s | --setup SETUP ]"
    exit 2
}


eval set -- "$PARSED_ARGS"
while :
do
    case "$1" in
        -l | --log-level) LOG_LEVEL=$2 ; shift 2 ;;
        -b | --board) BOARD="$2" ; shift 2 ;;
        -s | --setup) SETUP="$2" ; shift 2 ;;
        --release) RELEASE=1 ; shift ;;
        --help) usage; shift ;;
        --) shift; break;;
    esac
done

if [ "$BOARD" = unset ]; then
    usage
fi

if [ "$SETUP" = unset ]; then
    usage
fi

export DEFMT_LOG="$LOG_LEVEL"

cd builder
cargo build
if [ $? -ne 0 ]; then echo "build failed"; exit 2; fi
cd ..
./builder/target/debug/chiyocore-builder --firmware "$SETUP" --board "$BOARD" --out firmware/firmware
if [ $? -ne 0 ]; then echo "firmware gen failed"; exit 2; fi
cd firmware
export DEFMT_LOG=$LOG_LEVEL && cargo build --bin chiyocore `if [ $RELEASE -eq 1 ]; then echo "--release"; fi`
if [ $? -ne 0 ]; then echo "build failed"; exit 2; fi
cd ..
if [ "$RELEASE" -eq 0  ]; then
    echo "firmware/target/xtensa-esp32s3-none-elf/debug/chiyocore"
else
    echo "firmware/target/xtensa-esp32s3-none-elf/release/chiyocore"
fi