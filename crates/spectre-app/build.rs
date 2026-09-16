//! Zasoby Win32 binarki: ikona (`icon.ico`, generowana przez
//! `icon\make-icon.ps1`) i blok wersji z Cargo.toml - to, co widzi
//! Eksplorator, pasek zadan i "Zainstalowane aplikacje".

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=icon.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("icon.ico");
    res.set("ProductName", "SpectreNotes");
    res.set("FileDescription", "SpectreNotes");
    res.compile().expect("Win32 resources (rc.exe from the Windows SDK)");
}
