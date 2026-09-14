//! SpectreNotes - aplikacja.
//!
//! Spina: powloke Win32 (`spectre-shell-win`), renderer D2D (`spectre-render`),
//! dokument CRDT (`spectre-core`) i lokalny store `.ops` (`spectre-sync`).
//!
//! Stan po Etapach 1-3: jedna przestrzen (space) na dysku, wiele notatek,
//! kazda kreska laduje w pliku autora natychmiast po zakonczeniu, `fsync`
//! po 400 ms ciszy. Bez UI poza HUD-em - sterowanie klawiatura i przyciskami
//! rysika (patrz `hud_text`).
//!
//! Binarka jest aplikacja okienkowa (`windows_subsystem`): bez okna konsoli.
//! Tryby wierszowe (`--bench`, `--fill-white`) i `--console` dolaczaja sie do
//! konsoli rodzica, zeby ich wyjscie bylo widoczne w terminalu.

#![windows_subsystem = "windows"]

mod amoled;
mod app;
mod bench;
mod config;
mod github;
mod lan;
mod live;
mod menu;
mod sync;
mod ui;

use std::path::PathBuf;

use spectre_shell_win::window;

fn main() -> windows::core::Result<()> {
    if std::env::args().any(|a| a == "--bench" || a == "--fill-white" || a == "--console") {
        window::attach_parent_console();
    }
    if std::env::args().any(|a| a == "--bench") {
        return bench::run();
    }
    if std::env::args().any(|a| a == "--fill-white") {
        let dir = std::env::args()
            .skip(1)
            .find(|a| !a.starts_with("--"))
            .map(PathBuf::from)
            .unwrap_or_else(default_space_dir);
        return fill_white(&dir).map_err(|e| {
            eprintln!("blad: {e}");
            windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL)
        });
    }
    window::init_process();

    let space_dir = std::env::args()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .unwrap_or_else(default_space_dir);

    // `--tray` (autostart z systemem): start schowany, okno na Win+Shift+N.
    let start_hidden = std::env::args().any(|a| a == spectre_shell_win::autostart::TRAY_ARG);

    let hwnd = window::create_window("SpectreNotes", "SpectreNotes", app::wndproc, 1400, 900)?;
    app::install(hwnd, &space_dir, start_hidden)?;
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

/// `spectrenotes --fill-white [space]`: notatka testowa dla ochrony AMOLED -
/// cala kolumna zamalowana czysta biela (poziome pasy, dwie wysokosci ekranu).
/// Celowo #FFFFFF, nie paleta: to test najgorszego przypadku, nie do pisania.
fn fill_white(space_dir: &std::path::Path) -> std::io::Result<()> {
    use spectre_core::Document;
    use spectre_proto::{Rgba, Sample, StrokeData};
    use spectre_sync::{AuthorName, NoteStore, Space};

    let space = Space::open_or_create(space_dir)?;
    let author = AuthorName::from_env();
    let id = space.create_note()?;
    let (mut store, _) = NoteStore::open(&space, &id, &author)?;
    let mut doc = Document::new(author.id());
    store.append(&doc.set_meta("title", "Test AMOLED - biel"))?;
    let width = spectre_core::camera::COLUMN_W;
    let (step, w) = (10.0f32, 12.0f32);
    let mut y = w * 0.5;
    let mut n = 0;
    while y < 3600.0 {
        let s = |x: f32, t: u64| Sample {
            x,
            y,
            pressure: 1.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_us: t,
        };
        let op = doc.add_stroke(StrokeData {
            tool: 0,
            color: Rgba::rgb(255, 255, 255),
            base_width: w,
            samples: vec![s(0.0, 0), s(width * 0.5, 4000), s(width, 8000)],
        });
        store.append(&op)?;
        y += step;
        n += 1;
    }
    store.sync()?;
    space.write_note_meta(
        &id,
        &spectre_sync::NoteMeta {
            title: "Test AMOLED - biel".into(),
            folder: String::new(),
        },
    )?;
    println!("notatka {id}: {n} bialych pasow, {}", space_dir.display());
    Ok(())
}
