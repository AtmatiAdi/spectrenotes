//! `spectrenotes --bench`: mierzy koszt renderera bez piora w reku.
//!
//! Tworzy okno, wypelnia dokument syntetycznymi kreskami i mierzy czas
//! pojedynczych operacji: przebudowa, przewiniecie przyrostowe o 2 px,
//! prezentacja z HUD-em i bez. Wynik na stdout. To jest odpowiedz na pytanie
//! "czy przewijanie jest wolne, bo CPU, czy bo Present blokuje".

use std::time::Instant;

use spectre_core::{AuthorId, Camera, Document, Rgba, Sample, StrokeData};
use spectre_ink::InkConfig;
use spectre_render::{Overlay, PresentMode, Renderer};
use spectre_shell_win::window;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DispatchMessageW, PeekMessageW, ShowWindow, TranslateMessage, MSG, PM_REMOVE,
    SW_SHOW,
};

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, w, l)
}

fn pump() {
    unsafe {
        let mut m = MSG::default();
        while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}

fn synthetic_doc(strokes: usize, samples: usize, width: f32, height_total: f32) -> Document {
    let mut doc = Document::new(AuthorId(1));
    let mut seed = 0x9E37_79B9u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32) / (u32::MAX as f32)
    };
    for i in 0..strokes {
        let x0 = rnd() * width;
        let y0 = (i as f32 / strokes as f32) * height_total;
        let mut s = Vec::with_capacity(samples);
        let (mut x, mut y) = (x0, y0);
        for k in 0..samples {
            x += (rnd() - 0.5) * 6.0;
            y += (rnd() - 0.4) * 4.0;
            s.push(Sample {
                x,
                y,
                pressure: 0.3 + 0.5 * rnd(),
                tilt_x: 0.0,
                tilt_y: 0.0,
                t_us: k as u64 * 3760,
            });
        }
        doc.add_stroke(StrokeData {
            tool: 0,
            color: Rgba::rgb(216, 216, 216),
            base_width: 3.2,
            samples: s,
        });
    }
    doc
}

fn time<F: FnMut()>(iters: usize, mut f: F) -> (f32, f32) {
    let mut max = 0f32;
    let t0 = Instant::now();
    for _ in 0..iters {
        let t = Instant::now();
        f();
        max = max.max(t.elapsed().as_secs_f32() * 1000.0);
    }
    (t0.elapsed().as_secs_f32() * 1000.0 / iters as f32, max)
}

pub fn run() -> windows::core::Result<()> {
    window::init_process();
    let hwnd = window::create_window("SpectreBench", "bench", proc_, 1400, 900)?;
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    pump();
    let (w, h) = window::client_size(hwnd);
    let mut r = Renderer::new(hwnd, w, h)?;
    let ink = InkConfig::default();
    println!(
        "GPU: {}   okno: {w}x{h}   tearing: {}",
        r.adapter_name(),
        r.tearing_supported()
    );

    // (kresek, probek na kreske, wysokosc rolki). Ostatni przypadek to kryterium
    // Etapu 2: 100 000 kresek - dlugi notatnik, krotkie kreski, ~250 stron.
    for &(n, len, height) in &[
        (200usize, 150usize, 20_000.0f32),
        (2000, 150, 20_000.0),
        (10000, 150, 20_000.0),
        (100_000, 60, 450_000.0),
    ] {
        let doc = synthetic_doc(n, len, w as f32, height);
        let mut cam = Camera::default();
        // Zimny rebuild: budowa obrysow kazdej widocznej kreski - to placi
        // zmiana notatki.
        let t0 = std::time::Instant::now();
        r.rebuild(&doc, &cam, &ink)?;
        let rebuild_cold = t0.elapsed().as_secs_f32() * 1000.0;

        let (rebuild_avg, rebuild_max) = time(20, || {
            r.rebuild(&doc, &cam, &ink).unwrap();
        });

        let mut old = cam.scroll_y;
        let (scroll_avg, scroll_max) = time(300, || {
            old = cam.scroll_y;
            cam.scroll_to(cam.scroll_y + 2.0, 1e9, h as f32);
            r.scroll(&doc, &cam, old, &ink).unwrap();
        });

        // Gumka: przerysowanie prostokata wielkosci typowej kreski (ok. 300x120 px).
        let region = spectre_core::Bbox {
            min_x: 400.0,
            min_y: cam.scroll_y + 300.0,
            max_x: 700.0,
            max_y: cam.scroll_y + 420.0,
        };
        let (repaint_avg, repaint_max) = time(120, || {
            r.repaint(&doc, &cam, &ink, region).unwrap();
        });

        let hud = "linia 1\nlinia 2\nlinia 3\nlinia 4\nlinia 5";
        let (present_avg, present_max) = time(120, || {
            r.present(
                &[],
                Rgba::rgb(1, 1, 1),
                &cam,
                Overlay::default(),
                PresentMode::Immediate,
            )
            .unwrap();
        });
        let (present_hud_avg, present_hud_max) = time(120, || {
            r.present(
                &[],
                Rgba::rgb(1, 1, 1),
                &cam,
                Overlay {
                    hud: Some(hud),
                    cursor: None,
                    ui: &[],
                    blobs: &[],
                },
                PresentMode::Immediate,
            )
            .unwrap();
        });
        let (latest_avg, latest_max) = time(120, || {
            r.present(
                &[],
                Rgba::rgb(1, 1, 1),
                &cam,
                Overlay::default(),
                PresentMode::Latest,
            )
            .unwrap();
        });
        let (vsync_avg, vsync_max) = time(60, || {
            r.present(
                &[],
                Rgba::rgb(1, 1, 1),
                &cam,
                Overlay::default(),
                PresentMode::VSync,
            )
            .unwrap();
        });
        pump();

        println!(
            "\n{n} kresek x {len} probek (widocznych ~{}):\n\
             rebuild zimny    {rebuild_cold:7.2} ms\n\
             rebuild          {rebuild_avg:7.2} ms  (max {rebuild_max:6.2})\n\
             scroll +2px      {scroll_avg:7.3} ms  (max {scroll_max:6.2})\n\
             repaint 300x120  {repaint_avg:7.3} ms  (max {repaint_max:6.2})\n\
             present immediate{present_avg:7.3} ms  (max {present_max:6.2})\n\
             present +HUD     {present_hud_avg:7.3} ms  (max {present_hud_max:6.2})\n\
             present latest   {latest_avg:7.3} ms  (max {latest_max:6.2})\n\
             present vsync    {vsync_avg:7.3} ms  (max {vsync_max:6.2})",
            doc.visible_in(cam.visible(w as f32, h as f32)).count(),
        );
    }
    Ok(())
}
