//! Strony tla (PDF) na canvasie i ich kafelki. Czysta geometria, bez
//! Direct2D - renderer rysuje, aplikacja zamawia brakujace kafelki u watku
//! renderu PDF, a obie strony licza je tu tak samo.
//!
//! Kafelek to `TILE_PX` x `TILE_PX` pikseli na poziomie gestosci `2^level`
//! pikseli na jednostke canvasu. Poziom dla zoomu to najmniejsza potega
//! dwojki >= zoom: obraz jest zawsze pomniejszany (ostry), najwyzej 2x.
//! Poziom `OVERVIEW` to tani podglad calej strony (A4 w kolumnie = jeden
//! kafelek ~360x510 px), rysowany rozmyty, zanim dojda ostre.

use spectre_core::Bbox;

pub const TILE_PX: u32 = 1024;
pub const OVERVIEW: i8 = -3;
const MIN_LEVEL: i8 = -3;
const MAX_LEVEL: i8 = 2;

/// Strona w jednostkach canvasu (lewy gorny rog, rozmiar).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl PageRect {
    pub fn bbox(&self) -> Bbox {
        Bbox {
            min_x: self.x,
            min_y: self.y,
            max_x: self.x + self.w,
            max_y: self.y + self.h,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub page: u32,
    pub level: i8,
    pub tx: u32,
    pub ty: u32,
}

pub fn density(level: i8) -> f32 {
    2f32.powi(level as i32)
}

/// Bok kafelka w jednostkach canvasu.
pub fn span(level: i8) -> f32 {
    TILE_PX as f32 / density(level)
}

pub fn level_for(zoom: f32) -> i8 {
    let k = (zoom.max(1e-3).log2() - 1e-4).ceil() as i32;
    k.clamp(MIN_LEVEL as i32, MAX_LEVEL as i32) as i8
}

/// Prostokat kafelka na canvasie, przyciety do strony.
pub fn tile_rect(page: &PageRect, key: TileKey) -> Bbox {
    let s = span(key.level);
    let x0 = page.x + key.tx as f32 * s;
    let y0 = page.y + key.ty as f32 * s;
    Bbox {
        min_x: x0,
        min_y: y0,
        max_x: (x0 + s).min(page.x + page.w),
        max_y: (y0 + s).min(page.y + page.h),
    }
}

/// Kafelki poziomu `level` strony `page_no`, ktore przecinaja `rect`.
pub fn tiles_in(page: &PageRect, page_no: u32, level: i8, rect: Bbox) -> Vec<TileKey> {
    let pb = page.bbox();
    if !pb.intersects(&rect) {
        return Vec::new();
    }
    let s = span(level);
    let nx = (page.w / s).ceil().max(1.0) as u32;
    let ny = (page.h / s).ceil().max(1.0) as u32;
    let clamp = |v: f32, n: u32| (v.floor().max(0.0) as u32).min(n - 1);
    let tx0 = clamp((rect.min_x - page.x) / s, nx);
    let tx1 = clamp((rect.max_x - page.x) / s, nx);
    let ty0 = clamp((rect.min_y - page.y) / s, ny);
    let ty1 = clamp((rect.max_y - page.y) / s, ny);
    let mut out = Vec::with_capacity(((tx1 - tx0 + 1) * (ty1 - ty0 + 1)) as usize);
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            out.push(TileKey {
                page: page_no,
                level,
                tx,
                ty,
            });
        }
    }
    out
}

/// Kafelki potrzebne do pokazania `view` przy `zoom`: podglad kazdej
/// widocznej strony (najpierw - tani i od razu pokazuje strone), potem
/// ostre kafelki biezacego poziomu.
pub fn needed(pages: &[PageRect], view: Bbox, zoom: f32) -> Vec<TileKey> {
    let level = level_for(zoom);
    let mut out = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        if p.bbox().intersects(&view) {
            out.extend(tiles_in(p, i as u32, OVERVIEW, p.bbox()));
        }
    }
    if level != OVERVIEW {
        for (i, p) in pages.iter().enumerate() {
            out.extend(tiles_in(p, i as u32, level, view));
        }
    }
    out
}

/// Strony PDF w kolumnie: kazda na cala szerokosc `column_w`, jedna pod
/// druga z odstepem `gap`. `sizes` - rozmiary stron w dowolnych jednostkach
/// (liczy sie proporcja).
pub fn stack(sizes: &[(f32, f32)], column_w: f32, gap: f32) -> Vec<PageRect> {
    let mut y = 0.0;
    sizes
        .iter()
        .map(|&(w, h)| {
            let ph = column_w * h / w.max(1e-3);
            let r = PageRect {
                x: 0.0,
                y,
                w: column_w,
                h: ph,
            };
            y += ph + gap;
            r
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poziom_to_potega_dwojki_nie_mniejsza_niz_zoom() {
        assert_eq!(level_for(1.0), 0);
        assert_eq!(level_for(0.8), 0);
        assert_eq!(level_for(0.5), -1);
        assert_eq!(level_for(0.45), -1);
        assert_eq!(level_for(1.3), 1);
        assert_eq!(level_for(2.0), 1);
        assert_eq!(level_for(40.0), MAX_LEVEL);
        assert_eq!(level_for(0.001), MIN_LEVEL);
    }

    #[test]
    fn strony_jedna_pod_druga_na_szerokosc_kolumny() {
        let p = stack(&[(595.0, 842.0), (842.0, 595.0)], 2880.0, 48.0);
        assert_eq!(p[0].w, 2880.0);
        assert!((p[0].h - 4075.6).abs() < 0.5, "{}", p[0].h);
        assert!((p[1].y - (p[0].h + 48.0)).abs() < 1e-3);
        // Pozioma: nizsza, ta sama szerokosc.
        assert!(p[1].h < 2880.0);
    }

    #[test]
    fn kafelki_pokrywaja_widok_i_sa_przyciete_do_strony() {
        let page = stack(&[(595.0, 842.0)], 2880.0, 0.0)[0];
        // Poziom 0: kafelek 1024 jednostek, strona 2880x4076 -> 3x4 kafelki.
        let all = tiles_in(&page, 0, 0, page.bbox());
        assert_eq!(all.len(), 12);
        let last = tile_rect(&page, all[11]);
        assert_eq!(last.max_x, 2880.0);
        assert!((last.max_y - page.h).abs() < 1e-3);
        // Widok w srodku strony.
        let view = Bbox {
            min_x: 100.0,
            min_y: 1500.0,
            max_x: 1100.0,
            max_y: 1600.0,
        };
        let v = tiles_in(&page, 0, 0, view);
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(v.iter().all(|k| k.ty == 1));
        // Podglad: cala strona A4 w jednym kafelku.
        assert_eq!(tiles_in(&page, 0, OVERVIEW, page.bbox()).len(), 1);
        // Poza strona - nic.
        let off = Bbox {
            min_x: 0.0,
            min_y: 9000.0,
            max_x: 10.0,
            max_y: 9010.0,
        };
        assert!(tiles_in(&page, 0, 0, off).is_empty());
    }

    #[test]
    fn potrzebne_najpierw_podglad() {
        let pages = stack(&[(595.0, 842.0); 3], 2880.0, 48.0);
        let view = Bbox {
            min_x: 0.0,
            min_y: 3000.0,
            max_x: 2880.0,
            max_y: 4800.0,
        };
        let n = needed(&pages, view, 1.0);
        // Widok zahacza o strony 0 i 1: dwa podglady na poczatku.
        assert_eq!(n[0].level, OVERVIEW);
        assert_eq!(n[1].level, OVERVIEW);
        assert_eq!((n[0].page, n[1].page), (0, 1));
        assert!(n[2..].iter().all(|k| k.level == 0));
    }
}
