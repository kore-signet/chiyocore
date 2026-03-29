use std::io::Write;
use std::{fs::File, path::Path};

use convert_case::ccase;
use csv::StringRecord;

fn main() {
    /* build.rs partitions */
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=partitions.csv");
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(File::open("../partitions.csv").unwrap());

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
