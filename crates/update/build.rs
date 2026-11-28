use std::{env, fs, path::Path};

fn main() {
    const VERSIONS: &str = "../../release/versions.json";
    println!("cargo:rerun-if-changed={VERSIONS}");
    let versions: serde_json::Value =
        serde_json::from_slice(&fs::read(VERSIONS).unwrap()).expect("invalid versions.json");
    let version = versions["windows"]["version"].as_str().unwrap();
    let parts: Vec<u16> = version.split('.').map(|v| v.parse().unwrap()).collect();
    assert_eq!(parts.len(), 3, "expected major.minor.patch");
    let build = u16::try_from(versions["windows"]["build"].as_u64().unwrap())
        .expect("Windows FILEVERSION build must fit in 16 bits");
    assert!(build > 0);
    fs::write(
        Path::new(&env::var_os("OUT_DIR").unwrap()).join("version.rs"),
        format!("pub const WINDOWS_VERSION: &str = {version:?};\npub const WINDOWS_BUILD: u32 = {build};\n"),
    ).unwrap();
}
