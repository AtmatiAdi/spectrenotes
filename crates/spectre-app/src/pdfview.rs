//! Notatka na PDF: plik lezy w `assets/` space'a (nazwa = SHA-256 tresci,
//! jedzie gitem razem z notatka), a notatka wskazuje go metadana `pdf`
//! (LWW jak tytul, wiec dochodzi tez przez LAN). Strony leza w kolumnie jedna
//! pod druga, kazda na cala jej szerokosc (`tiles::stack`), pod kreskami.
//!
//! Renderuje je osobny watek (`Windows.Data.Pdf`: strona A4 w szerokosci
//! panelu to 80-120 ms - na watku okna zabraloby to piora). Aplikacja po
//! kazdej klatce podaje liste kafelkow potrzebnych **teraz** (`want`), watek
//! bierze je od poczatku, a to, co przestalo byc potrzebne (przewinieto
//! dalej), po prostu nie jest renderowane.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use spectre_core::camera::COLUMN_W;
use spectre_render::tiles::{self, PageRect, TileKey};
use spectre_shell_win::pdfdoc::{PageSize, PdfFile, Pixels};
use windows::Foundation::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Watek renderu ma cos dla okna (`PdfView::poll`).
pub const WM_PDF: u32 = WM_APP + 9;
/// Metadana notatki: sciezka pliku wzgledem katalogu space'u.
pub const META_KEY: &str = "pdf";
/// Odstep miedzy stronami w jednostkach canvasu.
pub const PAGE_GAP: f32 = 48.0;
/// GitHub odrzuca pliki powyzej 100 MB - taki PDF nie dojechalby na inne maszyny.
pub const MAX_BYTES: u64 = 95 << 20;

pub enum Event {
    Pages(Vec<PageRect>),
    Tile(TileKey, Pixels),
    Failed(String),
}

#[derive(Default)]
struct Queue {
    want: VecDeque<TileKey>,
    in_flight: HashSet<TileKey>,
    quit: bool,
}

type Shared = Arc<(Mutex<Queue>, Condvar)>;

pub struct PdfView {
    /// Wartosc metadanej, z ktorej powstal widok.
    pub asset: String,
    pub root: PathBuf,
    pub dark: bool,
    /// Uklad stron na canvasie; pusty, dopoki watek nie otworzy pliku.
    pub layout: Vec<PageRect>,
    shared: Shared,
    rx: Receiver<Event>,
}

impl PdfView {
    pub fn start(hwnd: HWND, root: &Path, asset: &str, dark: bool) -> Self {
        let shared: Shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let (tx, rx) = channel();
        let path = asset_path(root, asset);
        let worker = shared.clone();
        let raw = hwnd.0 as isize;
        std::thread::spawn(move || run(&path, dark, worker, tx, raw));
        Self {
            asset: asset.to_string(),
            root: root.to_path_buf(),
            dark,
            layout: Vec::new(),
            shared,
            rx,
        }
    }

    /// Zdarzenia od watku (po `WM_PDF`).
    pub fn poll(&mut self) -> Vec<Event> {
        let evs: Vec<Event> = self.rx.try_iter().collect();
        for e in &evs {
            if let Event::Pages(p) = e {
                self.layout = p.clone();
            }
        }
        evs
    }

    /// Dol ostatniej strony (do ograniczenia przewijania).
    pub fn bottom(&self) -> f32 {
        self.layout.last().map_or(0.0, |p| p.y + p.h)
    }

    /// Kafelki potrzebne teraz, w kolejnosci waznosci; `have` - czy renderer
    /// juz go ma. Zastepuje poprzednia liste.
    pub fn want(&self, keys: impl Iterator<Item = TileKey>, have: impl Fn(&TileKey) -> bool) {
        let (m, cv) = &*self.shared;
        let mut q = m.lock().unwrap();
        let mut seen = HashSet::new();
        let list: VecDeque<TileKey> = keys
            .filter(|k| !have(k) && !q.in_flight.contains(k) && seen.insert(*k))
            .collect();
        let wake = !list.is_empty();
        q.want = list;
        drop(q);
        if wake {
            cv.notify_one();
        }
    }
}

impl Drop for PdfView {
    fn drop(&mut self) {
        let (m, cv) = &*self.shared;
        m.lock().unwrap().quit = true;
        cv.notify_one();
    }
}

fn run(path: &Path, dark: bool, shared: Shared, tx: Sender<Event>, hwnd: isize) {
    let post = || unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as _)), WM_PDF, WPARAM(0), LPARAM(0));
    };
    let opened = PdfFile::open(path).and_then(|f| {
        let p = f.pages()?;
        Ok((f, p))
    });
    let (file, sizes) = match opened {
        Ok(v) => v,
        Err(e) => {
            let msg = if path.exists() {
                format!("PDF: cannot open ({})", e.message())
            } else {
                "PDF: file not on this machine yet (waiting for sync)".to_string()
            };
            let _ = tx.send(Event::Failed(msg));
            post();
            return;
        }
    };
    let layout = layout_of(&sizes);
    if tx.send(Event::Pages(layout.clone())).is_err() {
        return;
    }
    post();
    let (m, cv) = &*shared;
    loop {
        let key = {
            let mut q = m.lock().unwrap();
            loop {
                if q.quit {
                    return;
                }
                if let Some(k) = q.want.pop_front() {
                    q.in_flight.insert(k);
                    break k;
                }
                q = cv.wait(q).unwrap();
            }
        };
        let px = render_tile(&file, &sizes, &layout, key, dark);
        m.lock().unwrap().in_flight.remove(&key);
        match px {
            Some(px) => {
                if tx.send(Event::Tile(key, px)).is_err() {
                    return;
                }
                post();
            }
            None => continue,
        }
    }
}

/// Metadana trzyma sciezke z `/` (ta sama na kazdej maszynie), a
/// `StorageFile::GetFileFromPathAsync` odrzuca `/` w sciezce Windows
/// ("nie znaleziono pliku", choc `std::fs` go widzi) - skladamy z czlonow.
pub fn asset_path(root: &Path, asset: &str) -> PathBuf {
    asset
        .split('/')
        // `join("C:")` albo czlon z `\` zastapilby cala sciezke.
        .filter(|c| !c.is_empty() && *c != ".." && !c.contains([':', '\\']))
        .fold(root.to_path_buf(), |p, c| p.join(c))
}

pub fn layout_of(sizes: &[PageSize]) -> Vec<PageRect> {
    let wh: Vec<(f32, f32)> = sizes.iter().map(|s| (s.w, s.h)).collect();
    tiles::stack(&wh, COLUMN_W, PAGE_GAP)
}

fn render_tile(
    file: &PdfFile,
    sizes: &[PageSize],
    layout: &[PageRect],
    key: TileKey,
    dark: bool,
) -> Option<Pixels> {
    let page = layout.get(key.page as usize)?;
    let size = sizes.get(key.page as usize)?;
    let r = tiles::tile_rect(page, key);
    // Canvas -> DIP strony: strona ma `page.w` jednostek i `size.w` DIP-ow.
    let k = size.w / page.w;
    let src = Rect {
        X: (r.min_x - page.x) * k,
        Y: (r.min_y - page.y) * k,
        Width: (r.max_x - r.min_x) * k,
        Height: (r.max_y - r.min_y) * k,
    };
    let dest_w = ((r.max_x - r.min_x) * tiles::density(key.level)).round() as u32;
    let mut px = file.render(key.page, Some(src), dest_w.max(1)).ok()?;
    if dark {
        invert_lightness(&mut px.bgra);
    }
    Some(px)
}

/// Jasnosc odwrocona, odcien i nasycenie bez zmian (HSL): biala strona staje
/// sie czarna, czarny tekst jasny, zolte tlo ramki - ciemnozolte. W HSL to
/// przesuniecie wszystkich kanalow o `255 - max - min` - tanie, bez
/// przeliczania na HSL i z powrotem. Tak samo odwraca eksport kolory kresek.
pub fn invert_lightness(bgra: &mut [u8]) {
    for px in bgra.chunks_exact_mut(4) {
        let (b, g, r) = (px[0] as i32, px[1] as i32, px[2] as i32);
        let d = 255 - b.max(g).max(r) - b.min(g).min(r);
        px[0] = (b + d) as u8;
        px[1] = (g + d) as u8;
        px[2] = (r + d) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odwrocenie_jasnosci_zachowuje_odcien() {
        let mut px = vec![
            255, 255, 255, 255, // biel
            0, 0, 0, 255, // czern
            214, 238, 255, 255, // jasnozolty (BGR: niebieski najmniejszy)
        ];
        invert_lightness(&mut px);
        assert_eq!(&px[0..3], &[0, 0, 0]);
        assert_eq!(&px[4..7], &[255, 255, 255]);
        let (b, g, r) = (px[8], px[9], px[10]);
        assert!(r > g && g > b, "zolty zostaje zolty: {r} {g} {b}");
        assert!((r as u32 + g as u32 + b as u32) < 300, "i jest ciemny");
        // Zgodnie z eksportem (`pdf::invert_lightness` na kolorach kresek).
        let c = crate::pdf::invert_lightness(spectre_proto::Rgba::rgb(255, 238, 214));
        assert!((c.r as i32 - r as i32).abs() <= 1, "{c:?} vs {r}");
        assert!((c.g as i32 - g as i32).abs() <= 1);
        assert!((c.b as i32 - b as i32).abs() <= 1);
    }

    #[test]
    fn sciezka_assetu_bez_ukosnikow_i_wyjscia_z_katalogu() {
        let p = asset_path(Path::new(r"C:\space"), "assets/abc.pdf");
        assert_eq!(p, PathBuf::from(r"C:\space\assets\abc.pdf"));
        assert!(!p.to_string_lossy().contains('/'));
        // Metadana przychodzi od innych (git, LAN): nie wolno nia wyjsc poza space.
        let p = asset_path(Path::new(r"C:\space"), "../../Windows/x.pdf");
        assert_eq!(p, PathBuf::from(r"C:\space\Windows\x.pdf"));
        let p = asset_path(Path::new(r"C:\space"), r"C:/x.pdf");
        assert!(p.starts_with(r"C:\space"), "{}", p.display());
        let p = asset_path(Path::new(r"C:\space"), r"a\..\..\x.pdf");
        assert_eq!(p, PathBuf::from(r"C:\space"));
    }

    #[test]
    fn uklad_stron_na_szerokosc_kolumny() {
        let l = layout_of(&[
            PageSize {
                w: 793.28,
                h: 1122.56,
            },
            PageSize {
                w: 1122.56,
                h: 793.28,
            },
        ]);
        assert_eq!(l[0].w, COLUMN_W);
        assert_eq!(l[1].w, COLUMN_W);
        assert!((l[1].y - l[0].h - PAGE_GAP).abs() < 1e-3);
    }
}
