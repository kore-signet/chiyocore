use std::collections::HashMap;

use cargo_toml::{Dependency, DependencyDetail, Manifest};

// fn gen_config(extra_deps: Vec<>) {

// }

pub fn gen_cargo(
    chiyo_dep: DependencyDetail,
    extra_deps: HashMap<String, DependencyDetail>,
) -> Manifest {
    let base_manifest = toml::from_str(include_str!("../res/base_crate.toml")).unwrap();

    gen_config_from_base(chiyo_dep, extra_deps, base_manifest)
}

fn gen_chiyo_dep(chiyo_dep: &DependencyDetail, sub_dep: &str) -> Dependency {
    let mut dep_detail = chiyo_dep.clone();
    if let Some(base_path) = chiyo_dep.path.as_ref() {
        let base_path = format!("{base_path}/{sub_dep}");
        dep_detail.path = Some(base_path);
    }

    Dependency::Detailed(Box::new(dep_detail))
}

fn gen_config_from_base(
    chiyo_dep: DependencyDetail,
    extra_deps: HashMap<String, DependencyDetail>,
    mut base: Manifest,
) -> Manifest {
    let chiyo_dep = Box::new(chiyo_dep);
    base.dependencies
        .insert("chiyocore".into(), gen_chiyo_dep(&chiyo_dep, "chiyocore"));
    base.dependencies.insert(
        "chiyocore-companion".into(),
        gen_chiyo_dep(&chiyo_dep, "companion"),
    );
    base.dependencies
        .insert("chiyo-hal".into(), gen_chiyo_dep(&chiyo_dep, "chiyo-hal"));

    for (dep_name, dep) in extra_deps {
        base.dependencies
            .insert(dep_name, Dependency::Detailed(Box::new(dep)));
    }

    base
}
