#!/bin/bash

PARSED_ARGS=$(getopt -n flash.sh -o hl:b:s: --long help,log-level:,board:,setup:,release -- "$@")
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
    echo "Usage: ./flash.sh 
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
        -l | --log-level) LOG_LEVEL="$2" ; shift 2 ;;
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


BUILD_OUT=$(./build.sh --log-level $LOG_LEVEL -b $BOARD -s $SETUP `if [ $RELEASE -eq 1 ]; then echo "--release"; fi` )

if [ $? -ne 0 ]; then echo "build failed"; exit 2; fi
echo $BUILD_OUT
export DEFMT_LOG=$LOG_LEVEL && espflash flash \
 --log-format defmt \
 --output-format "[{L:severity}] {c:bold}: {s} {({c}.{fff}:{l:1})%italic%dimmed}" \
 --monitor --chip esp32s3 \
 --partition-table firmware/partitions.csv \
  "$BUILD_OUT"