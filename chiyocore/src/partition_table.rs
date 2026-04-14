pub struct Partition {
    pub label: &'static str,
    pub kind: &'static str,
    pub subkind: &'static str,
    pub offset: u32,
    pub size: u32,
}

include!(concat!(env!("OUT_DIR"), "/partitions.rs"));
