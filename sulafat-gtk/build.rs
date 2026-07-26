use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../data/icons.gresource.xml");
    println!("cargo:rerun-if-changed=../data/icons/hicolor/scalable/apps/org.lyraos.Sulafat.svg");
    println!("cargo:rerun-if-changed=../data/icons/hicolor/symbolic/apps/org.lyraos.Sulafat-symbolic.svg");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let target = out_dir.join("icons.gresource");
    let status = Command::new("glib-compile-resources")
        .arg("--target")
        .arg(&target)
        .arg("--sourcedir")
        .arg("../data/icons")
        .arg("../data/icons.gresource.xml")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("failed to execute glib-compile-resources");

    if !status.success() {
        panic!("glib-compile-resources failed with status {status}");
    }
}
