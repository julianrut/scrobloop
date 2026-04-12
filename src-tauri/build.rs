fn main() {
    println!("cargo:rerun-if-changed=../.env");
    if let Ok(path) = dotenvy::from_path_iter("../.env") {
        for item in path {
            if let Ok((key, value)) = item {
                println!("cargo:rustc-env={key}={value}");
            }
        }
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/system_audio.m");
        cc::Build::new()
            .file("src/system_audio.m")
            .flag("-fobjc-arc")
            .flag("-fmodules")
            .flag("-Wno-deprecated-declarations")
            .compile("system_audio");
        println!("cargo:rustc-link-lib=framework=ScreenCaptureKit");
        println!("cargo:rustc-link-lib=framework=CoreMedia");
        println!("cargo:rustc-link-lib=framework=AudioToolbox");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }

    tauri_build::build()
}
