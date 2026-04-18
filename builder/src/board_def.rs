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
    pub psram: bool
    // pub psram_mode: Option<String>,
}

impl FormatInto<Rust> for BoardRam {
    fn format_into(self, tokens: &mut Tokens<Rust>) {
        let BoardRam {
            reclaimed,
            main,
            psram,
        } = self;

        // let psram_enabled = psram_mode.is_some();

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
    pub rx_en: Option<String>,
    pub board_en: Option<String>,
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
            board_en
        } = self;

        let rx_en = match rx_en {
            Some(rx_en) => {
                genco::quote! {
                    Some(peripherals.$rx_en)
                }
            }
            None => {
                genco::quote! {
                    Option::<esp_hal::gpio::AnyPin>::None
                }
            }
        };

        let board_en =  match board_en {
            Some(board_en) => {
                genco::quote! {
                    Some(peripherals.$board_en)
                }
            }
            None => {
                genco::quote! {
                    Option::<esp_hal::gpio::AnyPin>::None
                }
            }
        };

        quote_in! { *tokens =>
            chiyocore::lora::LoraPinBundle {
                sclk: peripherals.$sclk,
                mosi: peripherals.$mosi,
                miso: peripherals.$miso,
                cs: peripherals.$cs,
                reset: peripherals.$reset,
                busy: peripherals.$busy,
                dio1: peripherals.$dio1,
                rx_en: $rx_en,
                board_en: $board_en,
                spi: peripherals.$spi
            }
        }
    }
}
