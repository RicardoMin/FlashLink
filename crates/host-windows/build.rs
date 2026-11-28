fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let version = flashlink_update::WINDOWS_VERSION;
        let build = flashlink_update::WINDOWS_BUILD;
        let number = version
            .split('.')
            .map(|part| part.parse::<u64>().unwrap())
            .chain([u64::from(build)])
            .fold(0, |number, part| (number << 16) | part);
        winresource::WindowsResource::new()
            .set("ProductName", "FlashLink")
            .set("FileDescription", "FlashLink Windows Host")
            .set("OriginalFilename", "FlashLink.exe")
            .set("ProductVersion", version)
            .set("FileVersion", &format!("{version}.{build}"))
            .set_version_info(winresource::VersionInfo::PRODUCTVERSION, number)
            .set_version_info(winresource::VersionInfo::FILEVERSION, number)
            .compile()
            .expect("compile Windows version resource");
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
