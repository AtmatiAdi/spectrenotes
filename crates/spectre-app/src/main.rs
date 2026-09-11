//! SpectreNotes - aplikacja.
//!
//! Spina: powloke Win32 (`spectre-shell-win`), renderer D2D (`spectre-render`),
//! dokument CRDT (`spectre-core`) i lokalny store `.ops` (`spectre-sync`).
//!
//! Stan po Etapach 1-3: jedna przestrzen (space) na dysku, wiele notatek,
//! kazda kreska laduje w pliku autora natychmiast po zakonczeniu, `fsync`
//! po 400 ms ciszy. Bez UI poza HUD-em - sterowanie klawiatura i przyciskami
//! rysika (patrz `hud_text`).

mod app;
mod bench;

use std::path::PathBuf;

use spectre_shell_win::window;

fn main() -> windows::core::Result<()> {
    if std::env::args().any(|a| a == "--bench") {
        return bench::run();
    }
    window::init_process();

    let space_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_space_dir);

    let hwnd = window::create_window("SpectreNotes", "SpectreNotes", app::wndproc, 1400, 900)?;
    app::install(hwnd, &space_dir)?;
    window::run_message_loop();
    Ok(())
}

/// `%APPDATA%\SpectreNotes\spaces\default` - jawna, zwykla sciezka, ktora
/// pozniej stanie sie repozytorium git (Etap 5).
fn default_space_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("SpectreNotes").join("spaces").join("default")
}
