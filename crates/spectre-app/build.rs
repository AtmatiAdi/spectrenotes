//! Zasoby Win32 binarki: ikona (`icon.ico`, generowana przez
//! `icon\make-icon.ps1`) i blok wersji z Cargo.toml - to, co widzi
//! Eksplorator, pasek zadan i "Zainstalowane aplikacje".

fn main() {
    oauth_secret();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=icon.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("icon.ico");
    res.set("ProductName", "SpectreNotes");
    res.set("FileDescription", "SpectreNotes");
    res.compile()
        .expect("Win32 resources (rc.exe from the Windows SDK)");
}

/// Sekret OAuth App do `github::CLIENT_SECRET` (`option_env!`): ze zmiennej
/// `SPECTRENOTES_OAUTH_SECRET`, a gdy jej nie ma - z pliku
/// `%USERPROFILE%\.spectrenotes-oauth-secret` (jedna linia; plik poza repo,
/// wpisuje go wlasciciel aplikacji OAuth). Bez obu binarka ma pusty sekret
/// i logowanie idzie przez Device Flow (kod w schowku).
fn oauth_secret() {
    println!("cargo:rerun-if-env-changed=SPECTRENOTES_OAUTH_SECRET");
    if std::env::var_os("SPECTRENOTES_OAUTH_SECRET").is_some() {
        return;
    }
    let Some(home) = std::env::var_os("USERPROFILE") else {
        return;
    };
    let path = std::path::Path::new(&home).join(".spectrenotes-oauth-secret");
    println!("cargo:rerun-if-changed={}", path.display());
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim();
        if !s.is_empty() {
            println!("cargo:rustc-env=SPECTRENOTES_OAUTH_SECRET={s}");
        }
    }
}
