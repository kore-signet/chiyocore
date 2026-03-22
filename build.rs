use std::io::Write;
use std::{fs::File, path::Path};

use convert_case::ccase;
use csv::StringRecord;

fn main() {
    linker_be_nice();
    // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
    println!("cargo:rustc-link-arg=-Tlinkall.x");

    /* build.rs partitions */
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=partitions.csv");
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(File::open("./partitions.csv").unwrap());

    let out_dir = std::env::var_os("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("partitions.rs");
    let mut out = std::fs::File::create(dest_path).unwrap();

    for partition_record in rdr.records() {
        let partition_record = partition_record.unwrap();
        write_partition(partition_record, &mut out).unwrap();
    }
}

fn write_partition(partition_record: StringRecord, out: &mut impl Write) -> std::io::Result<()> {
    let name = partition_record.get(0).unwrap().trim();
    let kind = partition_record.get(1).unwrap().trim();
    let subkind = partition_record.get(2).unwrap().trim();
    let offset = partition_record.get(3).unwrap().trim();
    let size = partition_record.get(4).unwrap().trim();

    let const_name = ccase!(constant, name);

    writeln!(out, "pub const {const_name}: Partition = Partition {{")?;
    writeln!(out, "\tlabel: \"{name}\",")?;
    writeln!(out, "\tkind: \"{kind}\",")?;
    writeln!(out, "\tsubkind: \"{subkind}\",")?;
    writeln!(out, "\toffset: {offset},")?;
    writeln!(out, "\tsize: {size},")?;
    writeln!(out, "}};\n")?;

    Ok(())
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                what if what.starts_with("_defmt_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`"
                    );
                    eprintln!();
                }
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                what if what.starts_with("esp_rtos_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-radio` has no scheduler enabled. Make sure you have initialized `esp-rtos` or provided an external scheduler."
                    );
                    eprintln!();
                }
                "embedded_test_linker_file_not_added_to_rustflags" => {
                    eprintln!();
                    eprintln!(
                        "💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests"
                    );
                    eprintln!();
                }
                "free"
                | "malloc"
                | "calloc"
                | "get_free_internal_heap_size"
                | "malloc_internal"
                | "realloc_internal"
                | "calloc_internal"
                | "free_internal" => {
                    eprintln!();
                    eprintln!(
                        "💡 Did you forget the `esp-alloc` dependency or didn't enable the `compat` feature on it?"
                    );
                    eprintln!();
                }
                _ => (),
            },
            // we don't have anything helpful for "missing-lib" yet
            _ => {
                std::process::exit(1);
            }
        }

        std::process::exit(0);
    }

    println!(
        "cargo:rustc-link-arg=-Wl,--error-handling-script={}",
        std::env::current_exe().unwrap().display()
    );
}
