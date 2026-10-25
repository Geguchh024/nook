//! Embeds the app icon and version info in nook.exe.

fn main() {
    const ICON: &str = "../../assets/icon/nook.ico";
    println!("cargo:rerun-if-changed={ICON}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ICON)
        .set("ProductName", "Nook")
        .set("FileDescription", "Nook")
        .set("LegalCopyright", "MIT License");
    res.compile().expect("embedding the icon resource");
}
