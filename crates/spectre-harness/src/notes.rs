//! Notatki wzorcowe testu T1: plaszczyzna zerowa (tryb 2D) ma wygladac
//! i zapisywac sie dokladnie tak, jak w v0.4.5.
//!
//! Dziesiec notatek obejmuje drogi rysowania 2D: tla (linie, kratka, kropki,
//! heksy, w kolumnie i bez konca), zoom, przewijanie przyrostowe, strony PDF
//! (kafelki - tu syntetyczne, deterministyczne), cienkie kreski przy
//! oddaleniu, wymazanie i cofanie, mokra kreska w trakcie rysowania.
//! Wszystko z ziarna - te same bajty i te same piksele przy kazdym uruchomieniu.

use spectre_core::background::{Background, Pattern};
use spectre_core::camera::COLUMN_W;
use spectre_core::{Document, Op, Rgba, Sample, StrokeData};
use spectre_proto::{AuthorId, OpsFileHeader, FORMAT_VERSION};
use spectre_render::tiles::{self, PageRect, TileKey};
use windows::core::Result;

use crate::pen::{Recording, Rng, SAMPLE_US};
use crate::scene::{Dirty, Scene};

/// Autor notatek wzorcowych (staly - bajty pliku od niego zaleza).
pub const AUTHOR: AuthorId = AuthorId(0x5EC7_2E00_0000_0001);

pub struct RefNote {
    pub name: &'static str,
    pub size: (u32, u32),
    /// Operacje notatki w kolejnosci zapisu (do pliku `.ops`).
    pub ops: Vec<Op>,
}

/// Kafelek syntetycznej strony PDF: jasna kartka z "wierszami tekstu"
/// zaleznymi tylko od klucza - jak strona odwrocona dla AMOLED (ciemna).
pub fn page_tile(key: TileKey, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    let d = tiles::density(key.level);
    for y in 0..h {
        for x in 0..w {
            // Wspolrzedne strony w jednostkach canvasu: wzor ciagly miedzy kafelkami.
            let cx = (key.tx as f32 * tiles::span(key.level)) + x as f32 / d;
            let cy = (key.ty as f32 * tiles::span(key.level)) + y as f32 / d;
            let line = (cy / 48.0) as i32;
            let in_text = (cy % 48.0) > 30.0
                && cx > 160.0
                && cx < 2600.0 - ((line * 37) % 700) as f32
                && (cx / 23.0) as i32 % 7 != 0;
            let v = if in_text {
                190
            } else {
                28 + (key.page as u8) * 6
            };
            out.extend_from_slice(&[v, v, v, 255]);
        }
    }
    out
}

fn stroke_from(samples: Vec<Sample>, color: Rgba, base_width: f32) -> StrokeData {
    StrokeData {
        tool: 0,
        color,
        base_width,
        samples,
    }
}

fn write_rec(doc: &mut Document, ops: &mut Vec<Op>, rec: &Recording, color: Rgba, w: f32) {
    for s in &rec.strokes {
        ops.push(doc.add_stroke(stroke_from(s.clone(), color, w)));
    }
}

fn bg(pattern: Pattern, spacing: f32, width: f32, color: Rgba, infinite: bool) -> String {
    Background {
        pattern,
        spacing,
        width,
        color,
        infinite,
    }
    .to_meta()
}

/// Losowe bazgroly po calej kolumnie (gesta notatka).
fn scribbles(doc: &mut Document, ops: &mut Vec<Op>, n: usize, height: f32, seed: u32) {
    let mut r = Rng(seed);
    for i in 0..n {
        let (mut x, mut y) = (r.unit() * COLUMN_W, i as f32 / n as f32 * height);
        let mut s = Vec::new();
        for k in 0..40 {
            x += (r.unit() - 0.5) * 9.0;
            y += (r.unit() - 0.4) * 6.0;
            s.push(Sample {
                x,
                y,
                pressure: 0.2 + 0.7 * r.unit(),
                t_us: k * SAMPLE_US,
                ..Default::default()
            });
        }
        ops.push(doc.add_stroke(stroke_from(s, Rgba::rgb(216, 216, 216), 3.2)));
    }
}

/// Dziesiec notatek wzorcowych (tylko dokument; kamera i reszta w `render`).
pub fn reference_notes() -> Vec<RefNote> {
    let mut out = Vec::new();
    let mut note =
        |name: &'static str, size: (u32, u32), f: &dyn Fn(&mut Document, &mut Vec<Op>)| {
            let mut doc = Document::new(AUTHOR);
            let mut ops = Vec::new();
            f(&mut doc, &mut ops);
            out.push(RefNote { name, size, ops });
        };
    let ink = Rgba::rgb(216, 216, 216);
    note("01-pusta-kratka", (1440, 900), &|d, o| {
        o.push(d.set_meta("title", "pusta"));
        o.push(d.set_meta(
            "bg",
            &bg(Pattern::Grid, 40.0, 1.0, Rgba::rgb(48, 48, 48), false),
        ));
    });
    note("02-pismo", (1440, 900), &|d, o| {
        write_rec(
            d,
            o,
            &Recording::handwriting(11, (80.0, 120.0), 8, 6, 34.0),
            ink,
            3.2,
        );
    });
    note("03-linie-zoom", (1440, 900), &|d, o| {
        o.push(d.set_meta(
            "bg",
            &bg(Pattern::Lines, 48.0, 1.0, Rgba::rgb(32, 52, 82), false),
        ));
        write_rec(
            d,
            o,
            &Recording::handwriting(12, (60.0, 100.0), 14, 7, 30.0),
            ink,
            3.2,
        );
    });
    note("04-kropki-oddalenie", (1440, 900), &|d, o| {
        o.push(d.set_meta(
            "bg",
            &bg(Pattern::Dots, 32.0, 2.0, Rgba::rgb(84, 84, 84), true),
        ));
        write_rec(
            d,
            o,
            &Recording::handwriting(13, (-400.0, 200.0), 10, 12, 40.0),
            ink,
            3.2,
        );
    });
    note("05-heksy-kolory", (1440, 900), &|d, o| {
        o.push(d.set_meta(
            "bg",
            &bg(Pattern::Hex, 36.0, 1.0, Rgba::rgb(30, 64, 44), false),
        ));
        let colors = [
            Rgba::rgb(230, 80, 80),
            Rgba::rgb(80, 200, 120),
            Rgba::rgb(90, 140, 240),
            Rgba::rgb(240, 200, 60),
        ];
        for (i, c) in colors.iter().enumerate() {
            let rec = Recording::handwriting(
                20 + i as u32,
                (100.0, 150.0 + i as f32 * 180.0),
                2,
                5,
                36.0,
            );
            write_rec(d, o, &rec, *c, 2.0 + i as f32 * 1.5);
        }
    });
    note("06-pdf", (1440, 900), &|d, o| {
        o.push(d.set_meta("pdf", "wzorzec"));
        write_rec(
            d,
            o,
            &Recording::handwriting(31, (300.0, 140.0), 6, 5, 30.0),
            Rgba::rgb(240, 120, 60),
            2.6,
        );
    });
    note("07-pdf-zoom-przewiniety", (1440, 900), &|d, o| {
        o.push(d.set_meta("pdf", "wzorzec"));
        write_rec(
            d,
            o,
            &Recording::handwriting(32, (200.0, 1500.0), 10, 6, 30.0),
            ink,
            3.2,
        );
    });
    note("08-gesta-cienka", (1440, 900), &|d, o| {
        scribbles(d, o, 2500, 9000.0, 0xBADC_0FFE);
    });
    note("09-pelny-ekran-przewijanie", (2880, 1800), &|d, o| {
        o.push(d.set_meta(
            "bg",
            &bg(Pattern::Grid, 40.0, 1.0, Rgba::rgb(48, 48, 48), true),
        ));
        write_rec(
            d,
            o,
            &Recording::handwriting(41, (100.0, 120.0), 30, 9, 40.0),
            ink,
            3.2,
        );
    });
    note("10-gumka-cofanie", (1440, 900), &|d, o| {
        write_rec(
            d,
            o,
            &Recording::handwriting(51, (80.0, 120.0), 6, 6, 34.0),
            ink,
            3.2,
        );
        let ids: Vec<_> = d.visible().map(|(id, _, _)| id).step_by(3).collect();
        o.extend(d.erase_strokes(&ids));
        o.extend(d.undo());
        let more: Vec<_> = d
            .visible()
            .map(|(id, _, _)| id)
            .skip(5)
            .step_by(4)
            .collect();
        o.extend(d.erase_strokes(&more));
    });
    out
}

/// Bajty pliku `.ops` notatki: naglowek i rekordy, jak `OpsWriter`.
pub fn ops_bytes(ops: &[Op]) -> Vec<u8> {
    let mut out = Vec::new();
    OpsFileHeader {
        version: FORMAT_VERSION,
        author: AUTHOR,
        base_lamport: 0,
    }
    .encode(&mut out);
    for op in ops {
        let mut payload = Vec::new();
        spectre_proto::encode_op(op, &mut payload);
        spectre_proto::record::write_record(spectre_proto::record::REC_OP, &payload, &mut out);
    }
    out
}

/// Ta sama kolumna stron, co dla prawdziwego PDF-u (`tiles::stack`).
fn pdf_pages() -> Vec<PageRect> {
    tiles::stack(
        &[(210.0, 297.0), (210.0, 297.0), (297.0, 210.0)],
        COLUMN_W,
        48.0,
    )
}

/// Obraz notatki wzorcowej w trybie 2D: jej kamera, tlo, strony i (dla
/// niektorych) przewijanie przyrostowe albo mokra kreska. BGRA.
pub fn render(note: &RefNote, scene: &mut Scene) -> Result<Vec<u8>> {
    scene.load_ops(&note.ops);
    scene.set_background(scene.doc.meta("bg").and_then(Background::from_meta));
    let (w, _) = scene.size();
    scene.cam.home(w as f32);
    if scene.doc.meta("pdf").is_some() {
        scene.set_pages(pdf_pages(), page_tile);
    }
    match note.name {
        "03-linie-zoom" => {
            scene.cam.zoom = 1.5;
            scene
                .cam
                .scroll_x_to(200.0, w as f32, scene.doc.content_bbox());
            scene
                .cam
                .scroll_to(300.0, scene.doc.content_bottom(), 900.0);
        }
        "04-kropki-oddalenie" => {
            scene.cam.zoom = 0.5;
            scene.cam.scroll_x_free(-700.0);
        }
        "07-pdf-zoom-przewiniety" => {
            scene.cam.zoom = 0.75;
            scene.cam.scroll_x_to(0.0, w as f32, None);
            scene
                .cam
                .scroll_to(1300.0, scene.doc.content_bottom().max(9000.0), 900.0);
        }
        "08-gesta-cienka" => {
            scene.cam.zoom = 0.25;
            scene.cam.scroll_x_to(0.0, w as f32, None);
            scene
                .cam
                .scroll_to(2000.0, scene.doc.content_bottom(), 900.0);
        }
        _ => {}
    }
    scene.dirty = Dirty::Full;
    scene.frame()?;
    match note.name {
        // Przewijanie przyrostowe: przesuniecie warstwy i dorysowanie pasa.
        "09-pelny-ekran-przewijanie" => {
            for _ in 0..12 {
                scene.scroll_by(37.0);
                scene.frame()?;
            }
            scene.scroll_by(-120.0);
            scene.frame()?;
        }
        // Mokra kreska w trakcie: czesc wypalona, czesc jako czubek.
        "02-pismo" => {
            let rec = Recording::handwriting(99, (200.0, 700.0), 1, 1, 60.0);
            let s = &rec.strokes[0];
            scene.pen_down(&s[..2]);
            for c in s[2..s.len() * 2 / 3].chunks(2) {
                scene.pen_move(c);
                scene.frame()?;
            }
        }
        _ => {}
    }
    scene.pixels()
}
