use genco::{Tokens, lang::Rust, quote_in, tokens::FormatInto};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct BoardFile {
    pub ram: BoardRam,
    pub pins: BoardPins,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BoardRam {
    pub reclaimed: String,
    pub main: String,
    pub psram: bool,
}

impl FormatInto<Rust> for BoardRam {
    fn format_into(self, tokens: &mut Tokens<Rust>) {
        let BoardRam {
            reclaimed,
            main,
            psram,
        } = self;

        quote_in! {*tokens =>
            esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: $reclaimed);
            esp_alloc::heap_allocator!(size: $main);
            $(if psram {
                esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);
            })
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BoardPins {
    pub sclk: String,
    pub mosi: String,
    pub miso: String,
    pub cs: String,
    pub reset: String,
    pub busy: String,
    pub dio1: String,
    pub rx_en: String,
    pub spi: String,
}

impl FormatInto<Rust> for BoardPins {
    fn format_into(self, tokens: &mut genco::Tokens<Rust>) {
        let BoardPins {
            sclk,
            mosi,
            miso,
            cs,
            reset,
            busy,
            dio1,
            rx_en,
            spi,
        } = self;
        quote_in! { *tokens =>
            chiyocore::lora::LoraPinBundle {
                sclk: peripherals.$sclk,
                mosi: peripherals.$mosi,
                miso: peripherals.$miso,
                cs: peripherals.$cs,
                reset: peripherals.$reset,
                busy: peripherals.$busy,
                dio1: peripherals.$dio1,
                rx_en: peripherals.$rx_en,
                spi: peripherals.$spi
            }
        }
    }
}
