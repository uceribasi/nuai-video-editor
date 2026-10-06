fn main() {
    // whisper.cpp's Metal backend guards newer APIs with `@available`, which calls
    // `__isPlatformVersionAtLeast` from clang's runtime library when the deployment
    // target (bundle.macOS.minimumSystemVersion) is older than those APIs. rustc doesn't
    // link that library, so add it.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let out = std::process::Command::new("cc")
            .arg("--print-resource-dir")
            .output()
            .expect("cc --print-resource-dir");
        let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
        println!("cargo:rustc-link-search=native={dir}/lib/darwin");
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    }
    tauri_build::build()
}
