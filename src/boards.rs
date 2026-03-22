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
        $crate::lora::LoraPins {
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
