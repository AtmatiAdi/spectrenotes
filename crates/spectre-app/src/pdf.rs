//! Eksport notatki do PDF - wektorowo, z tych samych obrysow kresek, ktore
//! rysuje ekran (`geometry::outlines`), wiec plik pokazuje dokladnie to, co
//! aplikacja, w dowolnym powiekszeniu. Bez zaleznosci: struktura PDF 1.4 to
//! kilka slownikow, tresc strony to operatory `m`/`l`/`h`/`f`, strumienie
//! kompresuje `deflate::zlib`.
//!
//! Strona = kolumna notatki (`COLUMN_W`, poszerzona, gdy tresc wystaje poza
//! nia) w proporcji A4, ciecie po wysokosci; kreska na styku stron trafia na
//! obie (z przycieciem do strony). Tlo: biale (kolory z odwrocona jasnoscia,
//! zeby jasna paleta AMOLED byla czytelna na papierze) albo czarne jak na
//! ekranie.

use spectre_core::camera::COLUMN_W;
use spectre_core::{Bbox, Document};
use spectre_ink::{InkConfig, Segment};
use spectre_proto::Rgba;
use spectre_render::geometry;
use spectre_render::stroke_segments;

use crate::deflate;

/// A4 w punktach.
const PAGE_W_PT: f32 = 595.0;
const PAGE_H_PT: f32 = 842.0;
/// Gestosc lukow obrysu (jak `zoom` przy rysowaniu): punkt na pol jednostki.
const OUTLINE_ZOOM: f32 = 2.0;

#[derive(Debug, Clone)]
pub struct Options {
    /// Biale tlo z odwrocona jasnoscia kolorow; `false` = czarne jak ekran.
    pub paper: bool,
    pub title: String,
}

/// Wynik: bajty pliku i liczba stron.
pub struct Pdf {
    pub bytes: Vec<u8>,
    pub pages: usize,
}

struct Shape {
    color: Rgba,
    bbox: Bbox,
    polys: Vec<Vec<(f32, f32)>>,
}

pub fn export(doc: &Document, ink: &InkConfig, opts: &Options) -> Pdf {
    let mut segs: Vec<Segment> = Vec::new();
    let mut shapes: Vec<Shape> = Vec::new();
    for (_, data, bbox) in doc.visible() {
        segs.clear();
        stroke_segments(data, ink, &mut segs);
        let polys = geometry::outlines(&segs, OUTLINE_ZOOM);
        if polys.is_empty() {
            continue;
        }
        shapes.push(Shape {
            color: if opts.paper {
                invert_lightness(data.color)
            } else {
                data.color
            },
            bbox: *bbox,
            polys,
        });
    }
    let content = doc.content_bbox();
    // Szerokosc strony: kolumna, chyba ze tresc wystaje poza nia.
    let x0 = content.map_or(0.0, |b| b.min_x.min(0.0));
    let x1 = content.map_or(COLUMN_W, |b| b.max_x.max(COLUMN_W));
    let width = (x1 - x0).max(1.0);
    let page_h = width * PAGE_H_PT / PAGE_W_PT;
    let y0 = content.map_or(0.0, |b| b.min_y.min(0.0));
    let y1 = content.map_or(0.0, |b| b.max_y);
    let pages = (((y1 - y0) / page_h).ceil() as usize).max(1);
    let scale = PAGE_W_PT / width;

    let mut w = Writer::new();
    // 1 = katalog, 2 = strony, 3 = info; strony i tresci od 4.
    let catalog = w.reserve();
    let pages_obj = w.reserve();
    let info = w.reserve();
    let mut page_ids = Vec::with_capacity(pages);
    for p in 0..pages {
        let top = y0 + p as f32 * page_h;
        let page_box = Bbox {
            min_x: x0,
            min_y: top,
            max_x: x1,
            max_y: top + page_h,
        };
        let mut c = String::with_capacity(64 * 1024);
        // Uklad: jednostki canvasu -> punkty, y w dol -> y w gore.
        c.push_str(&format!(
            "q {} 0 0 {} {} {} cm\n",
            num(scale),
            num(-scale),
            num(-x0 * scale),
            num(PAGE_H_PT + top * scale)
        ));
        if !opts.paper {
            c.push_str(&format!(
                "0 0 0 rg {} {} {} {} re f\n",
                num(x0),
                num(top),
                num(width),
                num(page_h)
            ));
        }
        c.push_str(&format!(
            "{} {} {} {} re W n\n",
            num(x0),
            num(top),
            num(width),
            num(page_h)
        ));
        for s in shapes.iter().filter(|s| s.bbox.intersects(&page_box)) {
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
        c.push_str("Q\n");
        let content_id = w.stream(c.as_bytes());
        let page_id = w.object(&format!(
            "<< /Type /Page /Parent {pages_obj} 0 R /MediaBox [0 0 {} {}] /Contents {content_id} 0 R /Resources << >> >>",
            num(PAGE_W_PT),
            num(PAGE_H_PT)
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
    w.fill(catalog, &format!("<< /Type /Catalog /Pages {pages_obj} 0 R >>"));
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
        pages,
    }
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
    let (r, g, b) = (
        c.r as f32 / 255.0,
        c.g as f32 / 255.0,
        c.b as f32 / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    let d = max - min;
    if d < 1e-6 {
        let v = ((1.0 - l) * 255.0).round() as u8;
        return Rgba { r: v, g: v, b: v, a: c.a };
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

    fn stroke(y: f32, color: Rgba) -> StrokeData {
        StrokeData {
            tool: 0,
            color,
            base_width: 6.0,
            samples: (0..20)
                .map(|i| Sample {
                    x: 200.0 + i as f32 * 50.0,
                    y: y + (i as f32 * 0.5).sin() * 40.0,
                    pressure: 0.6,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    t_us: i as u64 * 8000,
                })
                .collect(),
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
    fn strony_i_struktura() {
        let mut doc = Document::new(AuthorId(1));
        let _ = doc.add_stroke(stroke(300.0, Rgba::rgb(216, 216, 216)));
        // Druga kreska daleko w dol: trzecia strona (wysokosc strony ~4075).
        let _ = doc.add_stroke(stroke(9000.0, Rgba::rgb(229, 181, 103)));
        let ink = InkConfig::default();
        let pdf = export(
            &doc,
            &ink,
            &Options {
                paper: true,
                title: "test".into(),
            },
        );
        assert_eq!(pdf.pages, 3);
        let text = String::from_utf8_lossy(&pdf.bytes);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("/Count 3"));
        assert!(text.contains("/MediaBox [0 0 595 842]"));
        assert!(text.trim_end().ends_with("%%EOF"));
        assert_eq!(text.matches("/Type /Page ").count(), 3);
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
        // Strumienie sa binarne, wiec od `xref_at` czytamy bajty, nie `text`.
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
        let empty = export(
            &Document::new(AuthorId(1)),
            &ink,
            &Options {
                paper: false,
                title: String::new(),
            },
        );
        assert_eq!(empty.pages, 1);
    }
}
