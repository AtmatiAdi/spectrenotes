//! Eksport notatki do PDF - wektorowo, z tych samych obrysow kresek, ktore
//! rysuje ekran (`geometry::outlines`), wiec plik pokazuje dokladnie to, co
//! aplikacja, w dowolnym powiekszeniu. Bez zaleznosci: struktura PDF 1.4 to
//! kilka slownikow, tresc strony to operatory `m`/`l`/`h`/`f`, strumienie
//! kompresuje `deflate::zlib`.
//!
//! **Zwykla notatka**: strona ma szerokosc kolumny (to, co widac przy 100%),
//! poszerzona, gdy pismo wystaje poza kolumne (chyba ze `crop` - wtedy
//! dokladnie kolumna), i wysokosc ekranu przy 100% (`PLAIN_PAGE_H`);
//! kolejne strony w dol, dopoki jest pismo. Kreska na styku stron trafia na
//! obie (z przycieciem). Tlo: biale (kolory z odwrocona jasnoscia, zeby jasna
//! paleta AMOLED byla czytelna na papierze) albo czarne jak na ekranie.
//!
//! **Notatka na PDF** (`export_on`): oryginal z dopisana warstwa kresek
//! (`pdfedit`, aktualizacja przyrostowa). Pismo wystajace poza strone
//! powieksza ja; pismo pod ostatnia strona dostaje nowe strony jak w zwyklej
//! notatce.

use spectre_core::camera::COLUMN_W;
use spectre_core::{Bbox, Document};
use spectre_ink::{InkConfig, Segment};
use spectre_proto::Rgba;
use spectre_render::geometry;
use spectre_render::stroke_segments;
use spectre_render::tiles::PageRect;

use crate::deflate;
use crate::pdfedit::{self, dict_get, dict_set, Obj};

/// Wysokosc strony zwyklej notatki w jednostkach canvasu: ekran panelu przy
/// 100% (2880 x 1800, 16:10) - strona pokazuje to, co widac na raz.
pub const PLAIN_PAGE_H: f32 = COLUMN_W * 10.0 / 16.0;
/// Skala zwyklej notatki: kolumna 2880 jednostek = 720 pt (10 cali).
const PT_PER_UNIT: f32 = 0.25;
/// Gestosc lukow obrysu (jak `zoom` przy rysowaniu): punkt na pol jednostki.
const OUTLINE_ZOOM: f32 = 2.0;

#[derive(Debug, Clone)]
pub struct Options {
    /// Biale tlo z odwrocona jasnoscia kolorow; `false` = czarne jak ekran.
    pub paper: bool,
    /// Szerokosc dokladnie kolumny (100%) - pismo poza nia jest obcinane.
    pub crop: bool,
    pub title: String,
}

/// Wynik: bajty pliku i liczba stron.
pub struct Pdf {
    pub bytes: Vec<u8>,
    pub pages: usize,
}

pub struct Shape {
    color: Rgba,
    bbox: Bbox,
    polys: Vec<Vec<(f32, f32)>>,
}

/// Obrysy kresek w kolejnosci rysowania; `invert` - kolory z odwrocona
/// jasnoscia (pismo z czarnego ekranu na bialy papier).
pub fn shapes(doc: &Document, ink: &InkConfig, invert: bool) -> Vec<Shape> {
    let mut segs: Vec<Segment> = Vec::new();
    let mut out = Vec::new();
    for (_, data, bbox) in doc.visible() {
        segs.clear();
        stroke_segments(data, ink, &mut segs);
        let polys = geometry::outlines(&segs, OUTLINE_ZOOM);
        if polys.is_empty() {
            continue;
        }
        out.push(Shape {
            color: if invert {
                invert_lightness(data.color)
            } else {
                data.color
            },
            bbox: *bbox,
            polys,
        });
    }
    out
}

/// Kreski przecinajace `clip` (canvas), z przycieciem do niego. Uklad
/// wspolrzednych (`cm`) ustawia wolajacy.
fn ink_ops(c: &mut String, shapes: &[Shape], clip: Bbox) {
    c.push_str(&format!(
        "{} {} {} {} re W n\n",
        num(clip.min_x),
        num(clip.min_y),
        num(clip.max_x - clip.min_x),
        num(clip.max_y - clip.min_y)
    ));
    for s in shapes.iter().filter(|s| s.bbox.intersects(&clip)) {
        c.push_str(&format!(
            "{} {} {} rg\n",
            num(s.color.r as f32 / 255.0),
            num(s.color.g as f32 / 255.0),
            num(s.color.b as f32 / 255.0)
        ));
        for poly in &s.polys {
            for (i, (x, y)) in poly.iter().enumerate() {
                c.push_str(&num(*x));
                c.push(' ');
                c.push_str(&num(*y));
                c.push_str(if i == 0 { " m\n" } else { " l\n" });
            }
            c.push_str("h\n");
        }
        c.push_str("f\n");
    }
}

fn union(a: Bbox, b: Bbox) -> Bbox {
    Bbox {
        min_x: a.min_x.min(b.min_x),
        min_y: a.min_y.min(b.min_y),
        max_x: a.max_x.max(b.max_x),
        max_y: a.max_y.max(b.max_y),
    }
}

/// Strony "zwykle" dla pisma od `y_start` w dol: szerokosc kolumny (albo
/// szersza, gdy pismo wystaje i nie ma `crop`), wysokosc `PLAIN_PAGE_H`.
/// `plain` - zwykla notatka: pusta i tak dostaje strone, a pismo nad
/// `y_start` (wklejone ponad gore) przesuwa poczatek w gore. Pod stronami
/// PDF (`plain == false`) strony zaczynaja sie rowno od `y_start`.
fn plain_pages(shapes: &[Shape], y_start: f32, crop: bool, plain: bool) -> Vec<Bbox> {
    let force_one = plain;
    let below: Vec<Bbox> = shapes
        .iter()
        .map(|s| s.bbox)
        .filter(|b| b.max_y > y_start)
        .collect();
    let content = below.iter().copied().reduce(union);
    let Some(content) = content.or(force_one.then_some(Bbox {
        min_x: 0.0,
        min_y: y_start,
        max_x: COLUMN_W,
        max_y: y_start,
    })) else {
        return Vec::new();
    };
    let (x0, x1) = if crop {
        (0.0, COLUMN_W)
    } else {
        (content.min_x.min(0.0), content.max_x.max(COLUMN_W))
    };
    let top = if plain {
        content.min_y.min(y_start)
    } else {
        y_start
    };
    let n = (((content.max_y - top) / PLAIN_PAGE_H).ceil() as usize).max(1);
    (0..n)
        .map(|i| Bbox {
            min_x: x0,
            min_y: top + i as f32 * PLAIN_PAGE_H,
            max_x: x1,
            max_y: top + (i + 1) as f32 * PLAIN_PAGE_H,
        })
        .collect()
}

/// Tresc strony zwyklej: `k` punktow na jednostke, y w dol -> y w gore.
fn plain_content(shapes: &[Shape], page: Bbox, k: f32, black: bool) -> String {
    let h_pt = (page.max_y - page.min_y) * k;
    let mut c = String::with_capacity(64 * 1024);
    c.push_str(&format!(
        "q {} 0 0 {} {} {} cm\n",
        num(k),
        num(-k),
        num(-page.min_x * k),
        num(h_pt + page.min_y * k)
    ));
    if black {
        c.push_str(&format!(
            "0 0 0 rg {} {} {} {} re f\n",
            num(page.min_x),
            num(page.min_y),
            num(page.max_x - page.min_x),
            num(page.max_y - page.min_y)
        ));
    }
    ink_ops(&mut c, shapes, page);
    c.push_str("Q\n");
    c
}

pub fn export(doc: &Document, ink: &InkConfig, opts: &Options) -> Pdf {
    let shapes = shapes(doc, ink, opts.paper);
    let pages = plain_pages(&shapes, 0.0, opts.crop, true);
    let mut w = Writer::new();
    // 1 = katalog, 2 = strony, 3 = info; strony i tresci od 4.
    let catalog = w.reserve();
    let pages_obj = w.reserve();
    let info = w.reserve();
    let mut page_ids = Vec::with_capacity(pages.len());
    for page in &pages {
        let c = plain_content(&shapes, *page, PT_PER_UNIT, !opts.paper);
        let content_id = w.stream(c.as_bytes());
        let page_id = w.object(&format!(
            "<< /Type /Page /Parent {pages_obj} 0 R /MediaBox [0 0 {} {}] /Contents {content_id} 0 R /Resources << >> >>",
            num((page.max_x - page.min_x) * PT_PER_UNIT),
            num((page.max_y - page.min_y) * PT_PER_UNIT)
        ));
        page_ids.push(page_id);
    }
    let kids: Vec<String> = page_ids.iter().map(|id| format!("{id} 0 R")).collect();
    w.fill(
        pages_obj,
        &format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            page_ids.len()
        ),
    );
    w.fill(
        catalog,
        &format!("<< /Type /Catalog /Pages {pages_obj} 0 R >>"),
    );
    w.fill(
        info,
        &format!(
            "<< /Title {} /Producer (SpectreNotes {}) >>",
            pdf_text(&opts.title),
            spectre_update::CURRENT
        ),
    );
    Pdf {
        bytes: w.finish(catalog, info),
        pages: pages.len(),
    }
}

/// Notatka na PDF: oryginal (`original`) z kreskami dopisanymi aktualizacja
/// przyrostowa. `layout` - strony na canvasie (`pdfview::layout_of`, ta sama
/// kolejnosc co w pliku), `gap` - odstep miedzy nimi. `invert` - kolory
/// kresek z odwrocona jasnoscia (strony byly na ekranie ciemne, a w pliku sa
/// biale). Strony bez pisma zostaja nietkniete.
pub fn export_on(
    original: &[u8],
    layout: &[PageRect],
    gap: f32,
    doc: &Document,
    ink: &InkConfig,
    invert: bool,
    crop: bool,
) -> Result<Pdf, pdfedit::Error> {
    let src = pdfedit::Doc::open(original)?;
    let pages = src.pages()?;
    if pages.len() != layout.len() {
        return Err(pdfedit::Error::Broken("page count differs from the note"));
    }
    let shapes = shapes(doc, ink, invert);
    let mut up = pdfedit::Update::new(&src);
    let n = pages.len();
    for (i, (page, rect)) in pages.iter().zip(layout).enumerate() {
        let Some(ext) = page_ext(&shapes, rect, i, gap, crop) else {
            continue;
        };
        let vis = page.visible_box();
        let m = pdfedit::canvas_to_page(
            (rect.x as f64, rect.y as f64, rect.w as f64, rect.h as f64),
            vis,
            page.rotate,
        );
        // Pole strony po powiekszeniu o pismo (w przestrzeni strony).
        let corners = [
            (ext.min_x, ext.min_y),
            (ext.max_x, ext.min_y),
            (ext.min_x, ext.max_y),
            (ext.max_x, ext.max_y),
        ]
        .map(|(x, y)| pdfedit::apply(&m, x as f64, y as f64));
        let grow = |b: [f64; 4]| {
            corners.iter().fold(b, |b, &(x, y)| {
                [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]
            })
        };
        let mut c = String::from("Q\nq\n");
        c.push_str(&format!(
            "{} {} {} {} {} {} cm\n",
            pdfedit::fmt_num(m[0]),
            pdfedit::fmt_num(m[1]),
            pdfedit::fmt_num(m[2]),
            pdfedit::fmt_num(m[3]),
            pdfedit::fmt_num(m[4]),
            pdfedit::fmt_num(m[5])
        ));
        ink_ops(&mut c, &shapes, ext);
        c.push_str("Q\n");
        let open_q = up.stream(b"q\n");
        let overlay = up.stream(c.as_bytes());
        let mut contents = vec![Obj::Ref(open_q, 0)];
        match dict_get(&page.dict, "Contents") {
            Some(Obj::Ref(cn, cg)) => match src.resolve(*cn) {
                // `/Contents` wskazuje na tablice strumieni (obiekt posredni).
                Some(Obj::Array(a)) => contents.extend(a),
                _ => contents.push(Obj::Ref(*cn, *cg)),
            },
            Some(Obj::Array(a)) => contents.extend(a.iter().cloned()),
            _ => {}
        }
        contents.push(Obj::Ref(overlay, 0));
        let mut d = page.dict.clone();
        dict_set(&mut d, "Contents", Obj::Array(contents));
        let media = grow(page.media);
        if media != page.media {
            dict_set(&mut d, "MediaBox", box_obj(media));
            if let Some(cb) = page.crop {
                dict_set(&mut d, "CropBox", box_obj(grow(cb)));
            }
        }
        // Dziedziczony `/Rotate` zostaje w rodzicu - nic nie zmieniamy.
        up.put(page.num, page.gen, &Obj::Dict(d));
    }

    // Pismo pod ostatnia strona: nowe strony jak w zwyklej notatce, w skali
    // ostatniej strony (ta sama wielkosc pisma co na niej).
    let (y_start, k) = match (layout.last(), pages.last()) {
        (Some(r), Some(p)) => {
            let vis = p.visible_box();
            let shown_w = if p.rotate % 180 == 0 {
                vis[2] - vis[0]
            } else {
                vis[3] - vis[1]
            };
            (r.y + r.h + gap * 0.5, (shown_w / r.w as f64) as f32)
        }
        _ => (0.0, PT_PER_UNIT),
    };
    let extra = plain_pages(&shapes, y_start, crop, false);
    if !extra.is_empty() {
        let (root_num, mut root) = src.root_pages()?;
        let mut kids = match dict_get(&root, "Kids") {
            Some(Obj::Array(a)) => a.clone(),
            _ => return Err(pdfedit::Error::Broken("/Kids")),
        };
        let count = dict_get(&root, "Count")
            .and_then(Obj::num)
            .unwrap_or(n as f64) as i64;
        for &page in &extra {
            let c = plain_content(&shapes, page, k, false);
            let content = up.stream(c.as_bytes());
            let num = up.alloc();
            let mut d = pdfedit::Dict::new();
            dict_set(&mut d, "Type", Obj::Name(b"Page".to_vec()));
            dict_set(&mut d, "Parent", Obj::Ref(root_num, 0));
            dict_set(
                &mut d,
                "MediaBox",
                box_obj([
                    0.0,
                    0.0,
                    ((page.max_x - page.min_x) * k) as f64,
                    ((page.max_y - page.min_y) * k) as f64,
                ]),
            );
            dict_set(&mut d, "Contents", Obj::Ref(content, 0));
            dict_set(&mut d, "Resources", Obj::Dict(Vec::new()));
            up.put(num, 0, &Obj::Dict(d));
            kids.push(Obj::Ref(num, 0));
        }
        dict_set(&mut root, "Kids", Obj::Array(kids));
        dict_set(&mut root, "Count", Obj::int(count + extra.len() as i64));
        up.put(root_num, 0, &Obj::Dict(root));
    }
    Ok(Pdf {
        bytes: up.finish(),
        pages: n + extra.len(),
    })
}

/// Pismo strony `i` na canvasie: pas od polowy odstepu nad nia do polowy pod
/// nia (pierwsza - od samej gory). Wynik to strona powiekszona o pismo
/// z pasa (z `crop` - tylko w pionie); `None` = na tej stronie nic nie ma.
fn page_ext(shapes: &[Shape], rect: &PageRect, i: usize, gap: f32, crop: bool) -> Option<Bbox> {
    let band = Bbox {
        min_x: f32::NEG_INFINITY,
        min_y: if i == 0 {
            f32::NEG_INFINITY
        } else {
            rect.y - gap * 0.5
        },
        max_x: f32::INFINITY,
        max_y: rect.y + rect.h + gap * 0.5,
    };
    let mut ext = rect_bbox(rect);
    let mut any = false;
    for s in shapes.iter().filter(|s| s.bbox.intersects(&band)) {
        any = true;
        ext = union(
            ext,
            Bbox {
                min_x: s.bbox.min_x,
                min_y: s.bbox.min_y.max(band.min_y),
                max_x: s.bbox.max_x,
                max_y: s.bbox.max_y.min(band.max_y),
            },
        );
    }
    if crop {
        ext.min_x = ext.min_x.max(rect.x);
        ext.max_x = ext.max_x.min(rect.x + rect.w);
    }
    any.then_some(ext)
}

/// Strona oryginalu jako obraz (PDF zaszyfrowany - dopisac sie do niego nie
/// da): JPEG w rozmiarze strony, tak jak ja widac (po obrocie i przycieciu).
pub struct RasterPage {
    pub jpeg: Vec<u8>,
    pub w_px: u32,
    pub h_px: u32,
    /// Rozmiar widocznej strony w punktach.
    pub w_pt: f32,
    pub h_pt: f32,
}

/// PDF zastepczy: strony oryginalu jako obrazy, kreski wektorowo na nich -
/// te same zasady stron co `export_on` (powiekszenie, strony pod spodem).
/// Tekst oryginalu przestaje byc zaznaczalny; stad tylko awaryjnie.
#[allow(clippy::too_many_arguments)]
pub fn export_raster(
    pages: &[RasterPage],
    layout: &[PageRect],
    gap: f32,
    doc: &Document,
    ink: &InkConfig,
    invert: bool,
    crop: bool,
    title: &str,
) -> Pdf {
    let shapes = shapes(doc, ink, invert);
    let mut w = Writer::new();
    let catalog = w.reserve();
    let pages_obj = w.reserve();
    let info = w.reserve();
    let mut ids = Vec::new();
    for (i, (p, rect)) in pages.iter().zip(layout).enumerate() {
        let vis = [0.0, 0.0, p.w_pt as f64, p.h_pt as f64];
        let m = pdfedit::canvas_to_page(
            (rect.x as f64, rect.y as f64, rect.w as f64, rect.h as f64),
            vis,
            0,
        );
        let ext = page_ext(&shapes, rect, i, gap, crop);
        let mut media = vis;
        if let Some(e) = ext {
            for (x, y) in [
                (e.min_x, e.min_y),
                (e.max_x, e.min_y),
                (e.min_x, e.max_y),
                (e.max_x, e.max_y),
            ] {
                let (x, y) = pdfedit::apply(&m, x as f64, y as f64);
                media = [
                    media[0].min(x),
                    media[1].min(y),
                    media[2].max(x),
                    media[3].max(y),
                ];
            }
        }
        let img = w.stream_raw(
            &format!(
                "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode",
                p.w_px, p.h_px
            ),
            &p.jpeg,
        );
        let mut c = format!("q {} 0 0 {} 0 0 cm /Im0 Do Q\n", num(p.w_pt), num(p.h_pt));
        if let Some(e) = ext {
            c.push_str(&format!(
                "q {} {} {} {} {} {} cm\n",
                pdfedit::fmt_num(m[0]),
                pdfedit::fmt_num(m[1]),
                pdfedit::fmt_num(m[2]),
                pdfedit::fmt_num(m[3]),
                pdfedit::fmt_num(m[4]),
                pdfedit::fmt_num(m[5])
            ));
            ink_ops(&mut c, &shapes, e);
            c.push_str("Q\n");
        }
        let content = w.stream(c.as_bytes());
        ids.push(w.object(&format!(
            "<< /Type /Page /Parent {pages_obj} 0 R /MediaBox [{} {} {} {}] /Contents {content} 0 R /Resources << /XObject << /Im0 {img} 0 R >> >> >>",
            pdfedit::fmt_num(media[0]),
            pdfedit::fmt_num(media[1]),
            pdfedit::fmt_num(media[2]),
            pdfedit::fmt_num(media[3])
        )));
    }
    let (y_start, k) = match (layout.last(), pages.last()) {
        (Some(r), Some(p)) => (r.y + r.h + gap * 0.5, p.w_pt / r.w),
        _ => (0.0, PT_PER_UNIT),
    };
    for page in plain_pages(&shapes, y_start, crop, false) {
        let c = plain_content(&shapes, page, k, false);
        let content = w.stream(c.as_bytes());
        ids.push(w.object(&format!(
            "<< /Type /Page /Parent {pages_obj} 0 R /MediaBox [0 0 {} {}] /Contents {content} 0 R /Resources << >> >>",
            num((page.max_x - page.min_x) * k),
            num((page.max_y - page.min_y) * k)
        )));
    }
    let kids: Vec<String> = ids.iter().map(|id| format!("{id} 0 R")).collect();
    w.fill(
        pages_obj,
        &format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            ids.len()
        ),
    );
    w.fill(
        catalog,
        &format!("<< /Type /Catalog /Pages {pages_obj} 0 R >>"),
    );
    w.fill(
        info,
        &format!(
            "<< /Title {} /Producer (SpectreNotes {}) >>",
            pdf_text(title),
            spectre_update::CURRENT
        ),
    );
    Pdf {
        bytes: w.finish(catalog, info),
        pages: ids.len(),
    }
}

fn rect_bbox(r: &PageRect) -> Bbox {
    Bbox {
        min_x: r.x,
        min_y: r.y,
        max_x: r.x + r.w,
        max_y: r.y + r.h,
    }
}

fn box_obj(b: [f64; 4]) -> Obj {
    Obj::Array(b.iter().map(|&v| Obj::real(v)).collect())
}

/// Liczba bez zbednych zer: `12.5`, `3`, `-0.4`.
fn num(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// Napis PDF: UTF-16BE z BOM, zeby tytul z polskimi znakami przezyl.
fn pdf_text(s: &str) -> String {
    let mut out = String::from("<FEFF");
    for u in s.encode_utf16() {
        out.push_str(&format!("{u:04X}"));
    }
    out.push('>');
    out
}

/// Ten sam odcien i nasycenie, jasnosc odwrocona (HSL): jasnoszara kreska
/// z ekranu AMOLED staje sie ciemnoszara na papierze, zolta - ciemnozolta.
pub fn invert_lightness(c: Rgba) -> Rgba {
    let (r, g, b) = (c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    let d = max - min;
    if d < 1e-6 {
        let v = ((1.0 - l) * 255.0).round() as u8;
        return Rgba {
            r: v,
            g: v,
            b: v,
            a: c.a,
        };
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    let l2 = 1.0 - l;
    let cc = (1.0 - (2.0 * l2 - 1.0).abs()) * s;
    let x = cc * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l2 - cc * 0.5;
    let (r2, g2, b2) = match (h / 60.0) as u32 {
        0 => (cc, x, 0.0),
        1 => (x, cc, 0.0),
        2 => (0.0, cc, x),
        3 => (0.0, x, cc),
        4 => (x, 0.0, cc),
        _ => (cc, 0.0, x),
    };
    let to = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgba {
        r: to(r2),
        g: to(g2),
        b: to(b2),
        a: c.a,
    }
}

/// Plik PDF: obiekty numerowane od 1, tabela xref na koncu.
struct Writer {
    out: Vec<u8>,
    /// Offset kazdego obiektu (indeks = numer - 1); `None` = zarezerwowany.
    offsets: Vec<Option<usize>>,
}

impl Writer {
    fn new() -> Self {
        Self {
            out: b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec(),
            offsets: Vec::new(),
        }
    }

    fn reserve(&mut self) -> usize {
        self.offsets.push(None);
        self.offsets.len()
    }

    fn fill(&mut self, id: usize, body: &str) {
        self.offsets[id - 1] = Some(self.out.len());
        self.out
            .extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
    }

    fn object(&mut self, body: &str) -> usize {
        let id = self.reserve();
        self.fill(id, body);
        id
    }

    fn stream(&mut self, data: &[u8]) -> usize {
        let z = deflate::zlib(data);
        let id = self.reserve();
        self.offsets[id - 1] = Some(self.out.len());
        self.out.extend_from_slice(
            format!(
                "{id} 0 obj\n<< /Length {} /Filter /FlateDecode >>\nstream\n",
                z.len()
            )
            .as_bytes(),
        );
        self.out.extend_from_slice(&z);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
        id
    }

    /// Strumien bez naszej kompresji (JPEG ma juz swoja): `dict` - wpisy
    /// slownika bez `<< >>` i `/Length`.
    fn stream_raw(&mut self, dict: &str, data: &[u8]) -> usize {
        let id = self.reserve();
        self.offsets[id - 1] = Some(self.out.len());
        self.out.extend_from_slice(
            format!("{id} 0 obj\n<< {dict} /Length {} >>\nstream\n", data.len()).as_bytes(),
        );
        self.out.extend_from_slice(data);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
        id
    }

    fn finish(mut self, root: usize, info: usize) -> Vec<u8> {
        let xref = self.out.len();
        let n = self.offsets.len() + 1;
        self.out
            .extend_from_slice(format!("xref\n0 {n}\n0000000000 65535 f \n").as_bytes());
        for o in &self.offsets {
            self.out
                .extend_from_slice(format!("{:010} 00000 n \n", o.unwrap_or(0)).as_bytes());
        }
        self.out.extend_from_slice(
            format!(
                "trailer\n<< /Size {n} /Root {root} 0 R /Info {info} 0 R >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spectre_core::{AuthorId, Sample, StrokeData};

    fn stroke_at(x: f32, y: f32, color: Rgba) -> StrokeData {
        StrokeData {
            tool: 0,
            color,
            base_width: 6.0,
            samples: (0..20)
                .map(|i| Sample {
                    x: x + i as f32 * 50.0,
                    y: y + (i as f32 * 0.5).sin() * 40.0,
                    pressure: 0.6,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_us: i as u64 * 8000,
                })
                .collect(),
        }
    }

    fn stroke(y: f32, color: Rgba) -> StrokeData {
        stroke_at(200.0, y, color)
    }

    fn opts(paper: bool, crop: bool) -> Options {
        Options {
            paper,
            crop,
            title: "test".into(),
        }
    }

    #[test]
    fn liczby_i_napisy() {
        assert_eq!(num(12.5), "12.5");
        assert_eq!(num(3.0), "3");
        assert_eq!(num(-0.004), "0");
        assert_eq!(num(-0.4), "-0.4");
        assert_eq!(pdf_text("Aż"), "<FEFF0041017C>");
    }

    #[test]
    fn odwrocona_jasnosc() {
        let grey = invert_lightness(Rgba::rgb(216, 216, 216));
        assert_eq!((grey.r, grey.g, grey.b), (39, 39, 39));
        let y = invert_lightness(Rgba::rgb(229, 181, 103));
        // Zolty zostaje zolty (r > g > b), tylko ciemny.
        assert!(y.r > y.g && y.g > y.b, "{y:?}");
        assert!((y.r as u32 + y.g as u32 + y.b as u32) < 300, "{y:?}");
        let back = invert_lightness(y);
        assert!((back.r as i32 - 229).abs() <= 2, "{back:?}");
    }

    #[test]
    fn strony_zwyklej_notatki_i_struktura() {
        let mut doc = Document::new(AuthorId(1));
        let _ = doc.add_stroke(stroke(300.0, Rgba::rgb(216, 216, 216)));
        // Druga kreska daleko w dol: 9000 / 1800 -> szosta strona.
        let _ = doc.add_stroke(stroke(9000.0, Rgba::rgb(229, 181, 103)));
        let ink = InkConfig::default();
        let pdf = export(&doc, &ink, &opts(true, false));
        assert_eq!(pdf.pages, 6);
        let text = String::from_utf8_lossy(&pdf.bytes);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("/Count 6"));
        // Kolumna 2880 x 1800 w skali 0,25 pt.
        assert!(text.contains("/MediaBox [0 0 720 450]"));
        assert!(text.trim_end().ends_with("%%EOF"));
        assert_eq!(text.matches("/Type /Page ").count(), 6);
        // Kazdy offset w xref wskazuje na "N 0 obj".
        let xref_at: usize = text
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(pdf.bytes[xref_at..].starts_with(b"xref"));
        let tail = String::from_utf8_lossy(&pdf.bytes[xref_at..]);
        for (i, line) in tail.lines().skip(3).take(pdf.pages * 2 + 3).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            let head = format!("{} 0 obj", i + 1);
            assert!(
                pdf.bytes[off..].starts_with(head.as_bytes()),
                "obiekt {} pod {off}",
                i + 1
            );
        }
        // Pusta notatka: jedna strona, bez bledu.
        let empty = export(&Document::new(AuthorId(1)), &ink, &opts(false, false));
        assert_eq!(empty.pages, 1);
    }

    #[test]
    fn pismo_poza_kolumna_poszerza_strone_chyba_ze_przyciecie() {
        let mut doc = Document::new(AuthorId(1));
        // Kreska od x=2500 do ~3450: wystaje ~600 jednostek za kolumne.
        let _ = doc.add_stroke(stroke_at(2500.0, 300.0, Rgba::rgb(216, 216, 216)));
        let ink = InkConfig::default();
        let wide =
            String::from_utf8_lossy(&export(&doc, &ink, &opts(true, false)).bytes).to_string();
        let w: f32 = wide
            .split("/MediaBox [0 0 ")
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(w > 720.0 + 100.0, "strona poszerzona: {w}");
        let narrow =
            String::from_utf8_lossy(&export(&doc, &ink, &opts(true, true)).bytes).to_string();
        assert!(
            narrow.contains("/MediaBox [0 0 720 450]"),
            "przyciecie do 100%"
        );
    }

    /// PDF 1.5: tablica xref w strumieniu (z predyktorem PNG Up, jak robi
    /// wiekszosc generatorow) i katalog, drzewo stron oraz strona
    /// w strumieniu obiektow. Tak zapisuja Word, LibreOffice, pdfTeX.
    fn pdf15() -> Vec<u8> {
        let mut out = b"%PDF-1.5\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let content = b"0 0 1 rg 100 700 200 50 re f\n";
        let off4 = out.len();
        out.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        out.extend_from_slice(content);
        out.extend_from_slice(b"\nendstream\nendobj\n");
        // Strumien obiektow 5: obiekty 1 (katalog), 2 (strony), 3 (strona).
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>",
        ];
        let mut body = String::new();
        let mut heads = String::new();
        for (i, o) in objs.iter().enumerate() {
            heads.push_str(&format!("{} {} ", i + 1, body.len()));
            body.push_str(o);
            body.push('\n');
        }
        let data = format!("{heads}\n{body}");
        let first = heads.len() + 1;
        let z = deflate::zlib(data.as_bytes());
        let off5 = out.len();
        out.extend_from_slice(
            format!("5 0 obj\n<< /Type /ObjStm /N 3 /First {first} /Length {} /Filter /FlateDecode >>\nstream\n", z.len()).as_bytes(),
        );
        out.extend_from_slice(&z);
        out.extend_from_slice(b"\nendstream\nendobj\n");
        // Xref (obiekt 6): W [1 2 1], wiersze z predyktorem Up.
        let off6 = out.len();
        let rows: Vec<[u8; 4]> = vec![
            [0, 0, 0, 255],
            [2, 0, 5, 0],
            [2, 0, 5, 1],
            [2, 0, 5, 2],
            [1, (off4 >> 8) as u8, off4 as u8, 0],
            [1, (off5 >> 8) as u8, off5 as u8, 0],
            [1, (off6 >> 8) as u8, off6 as u8, 0],
        ];
        let mut pred = Vec::new();
        let mut prev = [0u8; 4];
        for r in &rows {
            pred.push(2);
            for i in 0..4 {
                pred.push(r[i].wrapping_sub(prev[i]));
            }
            prev = *r;
        }
        let z = deflate::zlib(&pred);
        out.extend_from_slice(
            format!(
                "6 0 obj\n<< /Type /XRef /Size 7 /W [1 2 1] /Root 1 0 R /Filter /FlateDecode /DecodeParms << /Columns 4 /Predictor 12 >> /Length {} >>\nstream\n",
                z.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(&z);
        out.extend_from_slice(
            format!("\nendstream\nendobj\nstartxref\n{off6}\n%%EOF\n").as_bytes(),
        );
        out
    }

    #[test]
    fn pdf_15_xref_w_strumieniu_i_strumien_obiektow() {
        let original = pdf15();
        let src = pdfedit::Doc::open(&original).unwrap();
        assert!(src.xref_is_stream);
        let pages = src.pages().unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].num, 3);
        assert_eq!(
            pages[0].media,
            [0.0, 0.0, 612.0, 792.0],
            "odziedziczony MediaBox"
        );
        let layout = spectre_render::tiles::stack(&[(612.0, 792.0)], COLUMN_W, 48.0);
        let mut doc = Document::new(AuthorId(2));
        // Kreska wystajaca w prawo poza strone: strona ma urosnac.
        let _ = doc.add_stroke(stroke_at(2600.0, 500.0, Rgba::rgb(229, 181, 103)));
        let ink = InkConfig::default();
        let out = export_on(&original, &layout, 48.0, &doc, &ink, true, false).unwrap();
        assert!(out.bytes.starts_with(&original));
        let again = pdfedit::Doc::open(&out.bytes).unwrap();
        assert!(again.xref_is_stream, "aktualizacja tez strumieniem xref");
        let p = again.pages().unwrap();
        assert_eq!(p.len(), 1);
        assert!(
            p[0].media[2] > 612.0 + 30.0,
            "powiekszona w prawo: {:?}",
            p[0].media
        );
        assert_eq!(p[0].media[1], 0.0, "reszta pola bez zmian");
        // Z przycieciem do 100% strona zostaje w rozmiarze oryginalu.
        let cropped = export_on(&original, &layout, 48.0, &doc, &ink, true, true).unwrap();
        let p = pdfedit::Doc::open(&cropped.bytes).unwrap().pages().unwrap();
        assert_eq!(p[0].media, [0.0, 0.0, 612.0, 792.0]);
        if let Ok(dir) = std::env::var("SPECTRENOTES_PDF15_OUT") {
            std::fs::write(format!("{dir}\\pdf15.pdf"), &original).unwrap();
            std::fs::write(format!("{dir}\\pdf15-out.pdf"), &out.bytes).unwrap();
        }
    }

    /// Notatka na PDF: oryginal (nasz wlasny, 2 strony) + kreska na 1. stronie
    /// + kreska pod ostatnia. Wynik czyta z powrotem `pdfedit` - stary plik
    /// jest przedrostkiem nowego, stron jest 3, pierwsza ma nowa tresc.
    #[test]
    fn dopisanie_do_oryginalu() {
        let mut base = Document::new(AuthorId(1));
        let _ = base.add_stroke(stroke(300.0, Rgba::rgb(216, 216, 216)));
        let _ = base.add_stroke(stroke(2000.0, Rgba::rgb(216, 216, 216)));
        let ink = InkConfig::default();
        let original = export(&base, &ink, &opts(true, false)).bytes;
        let src = pdfedit::Doc::open(&original).unwrap();
        let src_pages = src.pages().unwrap();
        assert_eq!(src_pages.len(), 2);
        // Uklad jak w aplikacji: strony 720x450 pt w kolumnie.
        let layout =
            spectre_render::tiles::stack(&[(720.0, 450.0), (720.0, 450.0)], COLUMN_W, 48.0);
        let mut doc = Document::new(AuthorId(2));
        let _ = doc.add_stroke(stroke(500.0, Rgba::rgb(229, 181, 103)));
        let below = layout[1].y + layout[1].h + 400.0;
        let _ = doc.add_stroke(stroke(below, Rgba::rgb(229, 181, 103)));
        let out = export_on(&original, &layout, 48.0, &doc, &ink, true, false).unwrap();
        assert_eq!(out.pages, 3);
        assert!(out.bytes.starts_with(&original), "oryginal nietkniety");
        let again = pdfedit::Doc::open(&out.bytes).unwrap();
        let pages = again.pages().unwrap();
        assert_eq!(pages.len(), 3);
        // Pierwsza strona: ten sam obiekt, tresc to tablica [q, oryginal, nasza].
        assert_eq!(pages[0].num, src_pages[0].num);
        let Some(Obj::Array(c)) = dict_get(&pages[0].dict, "Contents") else {
            panic!("{:?}", pages[0].dict);
        };
        assert_eq!(c.len(), 3);
        // Druga strona bez pisma - nietknieta (stara wersja obiektu).
        assert_eq!(
            dict_get(&pages[1].dict, "Contents"),
            dict_get(&src_pages[1].dict, "Contents")
        );
        // Nowa strona ma rozmiar strony PDF (720 pt szerokosci).
        assert_eq!(pages[2].media[2], 720.0);
    }
}
