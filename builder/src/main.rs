mod board_def;
mod config;
pub mod gen_cargo_conf;
mod gen_cargo_toml;
mod gen_main;

use std::{collections::HashMap, path::PathBuf};

use cargo_toml::DependencyDetail;
use clap::Parser;

use crate::{
    board_def::BoardFile,
    config::FullConfig,
    gen_cargo_conf::{GenConfigOptions, gen_config},
    gen_cargo_toml::gen_cargo,
    gen_main::gen_main,
};

#[derive(Parser, Debug)]
struct GenArgs {
    #[arg(short, long)]
    firmware: PathBuf,
    #[arg(short, long)]
    board: PathBuf,
    #[arg(short, long)]
    out: PathBuf,
    #[arg(long)]
    chiyo_path: Option<String>,
}

fn main() {
    let args = GenArgs::parse();
    let firmware: FullConfig =
        toml::from_str(&std::fs::read_to_string(&args.firmware).unwrap()).unwrap();

    let extra_deps: HashMap<String, DependencyDetail> = firmware
        .stackup
        .values()
        .flat_map(|v| v.layers.values().flat_map(|v| v.deps.clone()))
        .collect();

    let board: BoardFile = toml::from_str(&std::fs::read_to_string(&args.board).unwrap()).unwrap();
    // let psram_mode = board.ram.psram_mode.clone();

    let main_rs = gen_main(board, firmware);

    let chiyo_dep = if let Some(path) = args.chiyo_path {
        DependencyDetail {
            path: Some(path),
            ..Default::default()
        }
    } else {
        DependencyDetail {
            git: Some("https://github.com/kore-signet/chiyocore.git".into()),
            ..Default::default()
        }
    };

    let cargo_toml = gen_cargo(chiyo_dep, extra_deps);

    let src_folder = args.out.join("src/");
    std::fs::create_dir_all(&args.out).unwrap();
    std::fs::create_dir_all(&src_folder).unwrap();
    std::fs::create_dir_all(args.out.join(".cargo")).unwrap();

    let build_rs = include_str!("../res/build.rs");

    std::fs::write(args.out.join("build.rs"), build_rs).unwrap();
    std::fs::write(
        args.out.join("Cargo.toml"),
        toml::to_string_pretty(&cargo_toml).unwrap(),
    )
    .unwrap();
    std::fs::write(src_folder.join("main.rs"), &main_rs).unwrap();
    std::fs::write(
        args.out.join(".cargo/config.toml"),
        toml::to_string_pretty(&gen_config(GenConfigOptions { })).unwrap(),
    )
    .unwrap();
    std::fs::write(
        args.out.join("rust-toolchain.toml"),
        include_str!("../res/rust-toolchain.toml"),
    )
    .unwrap();
    std::fs::write(
        args.out.join("partitions.csv"),
        include_str!("../res/partitions.csv"),
    )
    .unwrap();
}
