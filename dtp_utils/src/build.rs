use cc;
fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("manifest_dir: {}", manifest_dir);
    let include_dir = format!("{}/include", manifest_dir);
    let config_c = format!("{}/src/dtp_config.c", manifest_dir);
    cc::Build::new()
        .file(config_c)
        .include(include_dir)
        .compile("dtp_config")
}