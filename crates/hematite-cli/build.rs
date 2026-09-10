fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("icon.ico");
        res.compile().unwrap();
    }

    println!("cargo::rustc-check-cfg=cfg(bmth)");
    println!("cargo::rerun-if-changed=src/bmth.rs");
    if std::path::Path::new("src/bmth.rs").exists() {
        println!("cargo::rustc-cfg=bmth");
    }
}
