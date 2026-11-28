fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        println!("cargo:rerun-if-changed=src/desktop_icons.cpp");
        println!("cargo:rerun-if-changed=src/audio_endpoint.cpp");
        println!("cargo:rerun-if-changed=src/media_transport.cpp");
        cc::Build::new()
            .cpp(true)
            .flag_if_supported(
                if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
                    "/EHsc"
                } else {
                    "-fexceptions"
                },
            )
            .file("src/desktop_icons.cpp")
            .file("src/audio_endpoint.cpp")
            .file("src/media_transport.cpp")
            .compile("flashlink_windows");
        for lib in [
            "shell32", "ole32", "oleaut32", "uuid", "user32", "iphlpapi", "ws2_32",
        ] {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
}
