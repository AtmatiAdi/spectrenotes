//! Ikona i blok wersji instalatora - ta sama ikona co aplikacja.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let icon = "../../crates/spectre-app/icon.ico";
    println!("cargo:rerun-if-changed={icon}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon);
    res.set("ProductName", "SpectreNotes");
    res.set("FileDescription", "SpectreNotes Setup");
    res.compile().expect("Win32 resources (rc.exe from the Windows SDK)");
}
