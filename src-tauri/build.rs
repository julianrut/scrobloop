fn main() {
    println!("cargo:rerun-if-changed=../.env");
    if let Ok(path) = dotenvy::from_path_iter("../.env") {
        for item in path {
            if let Ok((key, value)) = item {
                println!("cargo:rustc-env={key}={value}");
            }
        }
    }
    tauri_build::build()
}
