// [lora_pins]
// sclk = "GPIO7"
// mosi = "GPIO9"
// miso = "GPIO8"
// cs = "GPIO41"
// reset = "GPIO42"
// busy = "GPIO40"
// dio1 = "GPIO39"
// rx_en = "GPIO38"
// spi = "SPI2"
// //

// pub struct LoraPins {

// }

#[macro_export]
macro_rules! board_def {
    ($peripherals:ident, {
        sclk = $sclk:ident;
        mosi = $mosi:ident;
        miso = $miso:ident;
        cs = $cs:ident;
        reset = $reset:ident;
        busy = $busy:ident;
        dio1 = $dio1:ident;
        rx_en = $rx_en:ident;
        spi = $spi:ident;
    }) => {
        $crate::lora::LoraPinBundle {
            sclk: $peripherals.$sclk,
            mosi: $peripherals.$mosi,
            miso: $peripherals.$miso,
            cs: $peripherals.$cs,
            reset: $peripherals.$reset,
            busy: $peripherals.$busy,
            dio1: $peripherals.$dio1,
            rx_en: $peripherals.$rx_en,
            spi: $peripherals.$spi,
        }
    };
}

#[macro_export]
macro_rules! XIAO_S3 {
    ($peripherals:ident) => {
        $crate::board_def!($peripherals, {
            sclk = GPIO7;
            mosi = GPIO9;
            miso = GPIO8;
            cs = GPIO41;
            reset = GPIO42;
            busy = GPIO40;
            dio1 = GPIO39;
            rx_en = GPIO38;
            spi = SPI2;
        })
    };
}

use esp_hal::peripherals::{GPIO7, GPIO8, GPIO9, GPIO38, GPIO39, GPIO40, GPIO41, GPIO42, SPI2};

use crate::lora::LoraPinBundle;

pub type XiaoS3 = LoraPinBundle<
    GPIO7<'static>,
    GPIO9<'static>,
    GPIO8<'static>,
    GPIO41<'static>,
    GPIO42<'static>,
    GPIO40<'static>,
    GPIO39<'static>,
    GPIO38<'static>,
    SPI2<'static>,
>;
