// [target.xtensa-esp32s3-none-elf]
// runner = "espflash flash --monitor --chip esp32s3 --partition-table partitions.csv --log-format defmt"

// [env]
// CC = "xtensa-esp32s3-elf-cc"
// AR = "xtensa-esp32s3-elf-ar"
// CFLAGS = "-mlongcalls"
// ESP_HAL_CONFIG_PSRAM_MODE = "octal"
// # ESP_HAL_CONFIG_PLACE_SPI_MASTER_DRIVER_IN_RAM = "true"
// # ESP_ALLOC_CONFIG_HEAP_ALGORITHM = "TLSF"

// [build]
// rustflags = [
//   "-C", "link-arg=-nostartfiles",
// ]

// target = "xtensa-esp32s3-none-elf"

// [unstable]
// build-std = ["alloc", "core"]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct GenConfigOptions {
    // pub psram_mode: Option<String>,
}

pub fn gen_config(GenConfigOptions { }: GenConfigOptions) -> CargoConfig {
    let target_opts = HashMap::from_iter([("xtensa-esp32s3-none-elf".into(), TargetOpts {
        runner: "espflash flash --monitor --chip esp32s3 --partition-table partitions.csv --log-format defmt".into()
    })]);

    let env = CargoEnv {
        cc: "xtensa-esp32s3-elf-cc".into(),
        ar: "xtensa-esp32s3-elf-ar".into(),
        cflags: "-mlongcalls".into(),
        // psram_mode,
    };

    let unstable = CargoUnstable {
        build_std: vec!["alloc".into(), "core".into()],
    };

    let build = CargoBuild {
        rustflags: vec!["-C".into(), "link-arg=-nostartfiles".into()],
        target: "xtensa-esp32s3-none-elf".into(),
    };

    CargoConfig {
        target: target_opts,
        unstable,
        build,
        env,
    }
}

#[derive(Serialize, Deserialize)]
pub struct CargoConfig {
    target: HashMap<String, TargetOpts>,
    unstable: CargoUnstable,
    build: CargoBuild,
    env: CargoEnv,
}

#[derive(Serialize, Deserialize)]
pub struct CargoEnv {
    #[serde(rename = "CC")]
    cc: String,
    #[serde(rename = "AR")]
    ar: String,
    #[serde(rename = "CFLAGS")]
    cflags: String,
    // #[serde(
    //     rename = "ESP_HAL_CONFIG_PSRAM_MODE",
    //     skip_serializing_if = "Option::is_none"
    // )]
    // psram_mode: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct CargoBuild {
    rustflags: Vec<String>,
    target: String,
}

#[derive(Serialize, Deserialize)]
pub struct CargoUnstable {
    #[serde(rename = "build-std")]
    build_std: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct TargetOpts {
    runner: String,
}
