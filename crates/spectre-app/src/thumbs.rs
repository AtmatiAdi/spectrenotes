//! Cache miniatur notatek na dysku: `<dane>/thumbs/<id>-<odcisk>-<w>x<h>.png`.
//!
//! Miniatura raz narysowana (kilkadziesiat-kilkaset ms na notatke: odczyt
//! operacji, dokument, geometria kazdej kreski) zostaje na dysku i przy
//! kolejnym starcie jest tylko dekodowana (PNG ~30 kB, ~1 ms). Odcisk to
//! nazwy, rozmiary i czasy plikow z operacjami notatki - zmienia sie przy
//! kazdym dopisaniu i po synchronizacji, wiec nieaktualna miniatura nie ma
//! jak zostac. Ten sam plik sluzy panelowi i oknu wyboru notatki.

use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use spectre_shell_win::{capture, image};

/// Odcisk katalogu operacji notatki. Brak katalogu = 0 (pusta notatka).
pub fn fingerprint(note_dir: &Path) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let ops = note_dir.join("ops");
    let Ok(authors) = fs::read_dir(&ops) else {
        return 0;
    };
    let mut entries: Vec<(String, u64, u64)> = Vec::new();
    for author in authors.flatten() {
        let Ok(chunks) = fs::read_dir(author.path()) else {
            continue;
        };
        for chunk in chunks.flatten() {
            let Ok(meta) = chunk.metadata() else {
                continue;
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64);
            entries.push((chunk.path().to_string_lossy().into_owned(), meta.len(), mtime));
        }
    }
    // Kolejnosc z `read_dir` nie jest gwarantowana - sortujemy, zeby odcisk
    // byl stabilny.
    entries.sort();
    entries.hash(&mut h);
    h.finish()
}

fn file_name(id: &str, fp: u64, w: u32, h: u32) -> String {
    format!("{id}-{fp:016x}-{w}x{h}.png")
}

/// Miniatura z dysku, jesli jest dla tego odcisku i rozmiaru: BGRA.
pub fn load(dir: &Path, id: &str, fp: u64, w: u32, h: u32) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = fs::read(dir.join(file_name(id, fp, w, h))).ok()?;
    let img = image::decode(&bytes, w.max(h)).ok()?;
    (img.w == w && img.h == h).then_some((img.w, img.h, img.bgra))
}

/// Zapis miniatury; starsze pliki tej notatki (inny odcisk / rozmiar) znikaja.
pub fn save(dir: &Path, id: &str, fp: u64, w: u32, h: u32, bgra: &[u8]) {
    let Ok(png) = capture::encode_png(w, h, bgra) else {
        return;
    };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let name = file_name(id, fp, w, h);
    if let Ok(rd) = fs::read_dir(dir) {
        let prefix = format!("{id}-");
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with(&prefix) && n != name {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    let path: PathBuf = dir.join(name);
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, png).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

/// Usuwa miniatury notatek spoza `keep` (skasowane, opuszczone space'y).
pub fn retain(dir: &Path, keep: &dyn Fn(&str) -> bool) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let id = n.split('-').next().unwrap_or("");
        if !keep(id) {
            let _ = fs::remove_file(e.path());
        }
    }
}

// ----- watek miniatur -----------------------------------------------------------

use std::sync::mpsc::{self, Receiver, Sender};

use spectre_core::Document;
use spectre_ink::InkConfig;
use spectre_proto::StrokeData;
use spectre_render::ThumbRenderer;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Watek miniatur ma cos dla okna (`Worker::poll`).
pub const WM_THUMBS: u32 = WM_APP + 7;

/// Skad wziac kreski notatki.
pub enum Source {
    /// Z dysku: katalog notatki (odczyt operacji w watku).
    Disk(PathBuf),
    /// Z pamieci: otwarta notatka, ktorej zapis moze jeszcze trwac.
    Strokes(Vec<StrokeData>),
}

pub struct Job {
    pub id: String,
    /// Katalog notatki (odcisk do cache'u liczy watek).
    pub note_dir: PathBuf,
    pub source: Source,
    /// Lamport dokumentu, z ktorego jest miniatura (otwarta notatka) - okno
    /// zapamietuje go, zeby wiedziec, czy miniatura jest aktualna.
    pub lamport: Option<u64>,
    pub w: u32,
    pub h: u32,
    pub ink: InkConfig,
}

pub struct Ready {
    pub id: String,
    pub lamport: Option<u64>,
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
    /// Z cache'u na dysku (true) czy narysowana teraz.
    pub cached: bool,
}

pub struct Worker {
    tx: Sender<Job>,
    rx: Receiver<Ready>,
}

impl Worker {
    /// Startuje watek z wlasnym urzadzeniem D3D/D2D. `author` - autor lokalny
    /// (potrzebny dokumentowi), `cache_dir` - katalog PNG.
    pub fn start(hwnd: HWND, author_id: spectre_core::AuthorId, cache_dir: PathBuf) -> Self {
        let (tx, jobs) = mpsc::channel::<Job>();
        let (ready, rx) = mpsc::channel::<Ready>();
        let hwnd_raw = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("thumbs".into())
            .spawn(move || run(jobs, ready, hwnd_raw, author_id, cache_dir))
            .expect("watek miniatur");
        Self { tx, rx }
    }

    pub fn send(&self, job: Job) {
        let _ = self.tx.send(job);
    }

    pub fn poll(&self) -> Vec<Ready> {
        let mut out = Vec::new();
        while let Ok(r) = self.rx.try_recv() {
            out.push(r);
        }
        out
    }
}

fn run(
    jobs: Receiver<Job>,
    ready: Sender<Ready>,
    hwnd_raw: isize,
    author_id: spectre_core::AuthorId,
    cache_dir: PathBuf,
) {
    // Urzadzenie powstaje leniwie: przy cieplym cache'u moze nie byc potrzebne.
    let mut renderer: Option<ThumbRenderer> = None;
    // Statystyka partii (od pierwszego zlecenia do oproznienia kolejki) - do stderr.
    let (mut n_fp, mut n_load, mut n_draw, mut n_jobs) = (0f32, 0f32, 0f32, 0u32);
    let mut batch_t0: Option<std::time::Instant> = None;
    loop {
        let job = match jobs.try_recv() {
            Ok(j) => j,
            Err(mpsc::TryRecvError::Empty) => {
                if let Some(t0) = batch_t0.take() {
                    eprintln!(
                        "thumbs worker: {n_jobs} jobs in {:.0} ms (fp {n_fp:.0}, cache {n_load:.0}, draw+save {n_draw:.0})",
                        t0.elapsed().as_secs_f32() * 1000.0
                    );
                    (n_fp, n_load, n_draw, n_jobs) = (0.0, 0.0, 0.0, 0);
                }
                match jobs.recv() {
                    Ok(j) => j,
                    Err(_) => break,
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => break,
        };
        batch_t0.get_or_insert_with(std::time::Instant::now);
        n_jobs += 1;
        let t = std::time::Instant::now();
        let fp = fingerprint(&job.note_dir);
        n_fp += t.elapsed().as_secs_f32() * 1000.0;
        let t = std::time::Instant::now();
        let cached = load(&cache_dir, &job.id, fp, job.w, job.h);
        n_load += t.elapsed().as_secs_f32() * 1000.0;
        let t = std::time::Instant::now();
        let result = match cached {
            Some((w, h, bgra)) => Some((w, h, bgra, true)),
            None => {
                let strokes: Vec<StrokeData> = match job.source {
                    Source::Strokes(s) => s,
                    Source::Disk(dir) => match spectre_sync::store::read_note_ops(&dir) {
                        Ok(ops) => {
                            let mut doc = Document::new(author_id);
                            for op in &ops {
                                doc.apply(op);
                            }
                            doc.visible().map(|(_, d, _)| d.clone()).collect()
                        }
                        Err(_) => continue,
                    },
                };
                if renderer.is_none() {
                    renderer = ThumbRenderer::new().ok();
                }
                let Some(r) = renderer.as_mut() else {
                    continue;
                };
                match r.render(&strokes, &job.ink, job.w, job.h) {
                    Ok(bgra) => {
                        save(&cache_dir, &job.id, fp, job.w, job.h, &bgra);
                        Some((job.w, job.h, bgra, false))
                    }
                    Err(e) => {
                        eprintln!("thumb {}: {e}", job.id);
                        None
                    }
                }
            }
        };
        if !matches!(result, Some((_, _, _, true))) {
            n_draw += t.elapsed().as_secs_f32() * 1000.0;
        }
        let Some((w, h, bgra, cached)) = result else {
            continue;
        };
        if ready
            .send(Ready {
                id: job.id,
                lamport: job.lamport,
                w,
                h,
                bgra,
                cached,
            })
            .is_err()
        {
            break;
        }
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd_raw as *mut _)), WM_THUMBS, WPARAM(0), LPARAM(0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odcisk_zmienia_sie_po_dopisaniu() {
        let dir = std::env::temp_dir().join(format!("spectre-thumbs-{}", std::process::id()));
        let note = dir.join("note");
        fs::create_dir_all(note.join("ops").join("a")).unwrap();
        let empty = fingerprint(&note);
        fs::write(note.join("ops").join("a").join("0001.ops"), b"abc").unwrap();
        let one = fingerprint(&note);
        fs::write(note.join("ops").join("a").join("0001.ops"), b"abcdef").unwrap();
        let two = fingerprint(&note);
        assert_ne!(empty, one);
        assert_ne!(one, two);
        assert_eq!(two, fingerprint(&note));
        assert_eq!(fingerprint(&dir.join("brak")), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn zapis_i_odczyt_png() {
        let dir = std::env::temp_dir().join(format!("spectre-thumbs-io-{}", std::process::id()));
        let (w, h) = (4u32, 3u32);
        let mut px = Vec::new();
        for i in 0..(w * h) {
            px.extend_from_slice(&[(i * 20) as u8, 100, 200, 255]);
        }
        save(&dir, "N1", 7, w, h, &px);
        let (lw, lh, got) = load(&dir, "N1", 7, w, h).expect("miniatura z dysku");
        assert_eq!((lw, lh), (w, h));
        assert_eq!(got, px);
        assert!(load(&dir, "N1", 8, w, h).is_none());
        // Nowy odcisk zastepuje stary plik.
        save(&dir, "N1", 8, w, h, &px);
        assert!(load(&dir, "N1", 7, w, h).is_none());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        retain(&dir, &|id| id != "N1");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// `SPECTRE_THUMB_PNG=<plik> cargo test --release thumbs::bench -- --nocapture --ignored`
    #[test]
    #[ignore]
    fn czas_dekodowania() {
        let Some(path) = std::env::var_os("SPECTRE_THUMB_PNG") else {
            return;
        };
        let bytes = fs::read(&path).unwrap();
        let t = std::time::Instant::now();
        for _ in 0..100 {
            let img = image::decode(&bytes, 4096).unwrap();
            assert!(img.w > 0);
        }
        eprintln!("decode: {:.2} ms / plik", t.elapsed().as_secs_f32() * 10.0);
        let t = std::time::Instant::now();
        for _ in 0..100 {
            let _ = fs::read(&path).unwrap();
        }
        eprintln!("read: {:.2} ms / plik", t.elapsed().as_secs_f32() * 10.0);
    }
}
