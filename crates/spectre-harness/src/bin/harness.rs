//! `spectre-harness` - uprzaz testowa z wiersza polecen (`docs/08-UPRZAZ-TESTOWA.md`).
//!
//! ```text
//! spectre-harness t1 <katalog> [--png]       obrazy i bajty notatek wzorcowych
//! spectre-harness t1-compare <a> <b>         roznica dwoch przebiegow t1
//! spectre-harness latency [--warp] [--rec plik] [--per-msg N] [--strokes N]
//! spectre-harness record <plik>              zapis syntetycznego nagrania piora
//! spectre-harness idle                       licznik klatek w spoczynku (T5.3)
//! spectre-harness ops-roundtrip <we> <wy>    plik .ops przez czytnik tej wersji (T2.2)
//! ```

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use spectre_harness::notes::{ops_bytes, reference_notes, render};
use spectre_harness::pen::Recording;
use spectre_harness::{png, Scene};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let res = match cmd {
        "t1" => t1(&args),
        "t1-compare" => t1_compare(&args),
        "latency" => latency(&args),
        "record" => record(&args),
        "idle" => idle(),
        "ops-roundtrip" => ops_roundtrip(&args),
        _ => {
            eprintln!("uzycie: spectre-harness t1|t1-compare|latency|record|idle (docs/08)");
            std::process::exit(2);
        }
    };
    if let Err(e) = res {
        eprintln!("blad: {e}");
        std::process::exit(1);
    }
}

type Res = Result<(), String>;

fn t1(args: &[String]) -> Res {
    let dir = PathBuf::from(args.get(1).ok_or("brak katalogu")?);
    let want_png = args.iter().any(|a| a == "--png");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut index = String::new();
    for note in reference_notes() {
        let t0 = Instant::now();
        let mut scene = Scene::warp(note.size.0, note.size.1).map_err(|e| e.to_string())?;
        let px = render(&note, &mut scene).map_err(|e| format!("{}: {e}", note.name))?;
        let bytes = ops_bytes(&note.ops);
        std::fs::write(dir.join(format!("{}.ops", note.name)), &bytes)
            .map_err(|e| e.to_string())?;
        std::fs::write(dir.join(format!("{}.bgra", note.name)), &px).map_err(|e| e.to_string())?;
        if want_png {
            png::write(
                &dir.join(format!("{}.png", note.name)),
                note.size.0,
                note.size.1,
                &px,
            )
            .map_err(|e| e.to_string())?;
        }
        let line = format!(
            "{} {}x{} img={:016x} ops={:016x} ops_len={} ({:.0} ms)\n",
            note.name,
            note.size.0,
            note.size.1,
            png::hash(&px),
            png::fnv(&bytes),
            bytes.len(),
            t0.elapsed().as_secs_f32() * 1000.0
        );
        print!("{line}");
        index.push_str(&line);
    }
    std::fs::write(dir.join("index.txt"), index).map_err(|e| e.to_string())
}

fn t1_compare(args: &[String]) -> Res {
    let a = PathBuf::from(args.get(1).ok_or("brak katalogu a")?);
    let b = PathBuf::from(args.get(2).ok_or("brak katalogu b")?);
    let mut fails = 0;
    for note in reference_notes() {
        let read = |d: &Path, ext: &str| std::fs::read(d.join(format!("{}.{ext}", note.name)));
        let (pa, pb) = (read(&a, "bgra"), read(&b, "bgra"));
        let (oa, ob) = (read(&a, "ops"), read(&b, "ops"));
        let (Ok(pa), Ok(pb), Ok(oa), Ok(ob)) = (pa, pb, oa, ob) else {
            println!("{}: brak plikow", note.name);
            fails += 1;
            continue;
        };
        let dpx = png::diff_pixels(&pa, &pb);
        let same_ops = oa == ob;
        println!(
            "{}: piksele rozne {} z {} ({}), plik .ops {} ({} B)",
            note.name,
            dpx,
            pa.len() / 4,
            if dpx == 0 { "T1.1 ok" } else { "T1.1 NIE" },
            if same_ops {
                "bajt w bajt (T1.2 ok)"
            } else {
                "ROZNY (T1.2 NIE)"
            },
            oa.len()
        );
        if dpx != 0 || !same_ops {
            fails += 1;
        }
    }
    if fails > 0 {
        return Err(format!("{fails} notatek rozni sie od wzorca"));
    }
    println!("T1.1 i T1.2: wszystkie notatki zgodne z wzorcem");
    Ok(())
}

fn latency(args: &[String]) -> Res {
    let warp = args.iter().any(|a| a == "--warp");
    let per_msg: usize = arg(args, "--per-msg")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let strokes: usize = arg(args, "--strokes")
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000);
    let (w, h) = (2880u32, 1800u32);
    let mut scene = if warp {
        Scene::warp(w, h)
    } else {
        Scene::gpu(w, h)
    }
    .map_err(|e| e.to_string())?;
    println!("adapter: {}", scene.renderer.adapter_name());
    // Tlo: gesta notatka, jak u uzytkownika (kreski w warstwie suchej).
    let notes = reference_notes();
    if strokes > 0 {
        let bg_note = notes.iter().find(|n| n.name == "08-gesta-cienka").unwrap();
        let n = strokes.min(bg_note.ops.len());
        scene.load_ops(&bg_note.ops[..n]);
        scene.cam.home(w as f32);
        scene.frame().map_err(|e| e.to_string())?;
    }
    let rec = match arg(args, "--rec") {
        Some(p) => Recording::from_text(&std::fs::read_to_string(&p).map_err(|e| e.to_string())?)
            .ok_or("zly plik nagrania")?,
        None => Recording::handwriting(7, (120.0, 200.0), 6, 6, 40.0),
    };
    // Rozgrzewka: pierwsze kreski buduja pedzle i cache.
    scene
        .replay(
            &Recording::handwriting(3, (100.0, 1400.0), 1, 3, 40.0),
            per_msg,
            true,
        )
        .map_err(|e| e.to_string())?;
    let lat = scene
        .replay(&rec, per_msg, true)
        .map_err(|e| e.to_string())?;
    println!(
        "probki: {}, paczka: {per_msg}, klatki: {}",
        rec.sample_count(),
        lat.frames
    );
    println!("zdarzenie -> present (CPU):   {}", lat.cpu);
    println!("zdarzenie -> klatka gotowa:   {}", lat.gpu);
    Ok(())
}

fn record(args: &[String]) -> Res {
    let path = args.get(1).ok_or("brak pliku")?;
    let rec = Recording::handwriting(7, (120.0, 200.0), 6, 6, 40.0);
    std::fs::write(path, rec.to_text()).map_err(|e| e.to_string())
}

/// T5.3 w uprzezy: po ostatnim zdarzeniu scena nie oddaje zadnej klatki.
/// Petla "aplikacji" rysuje tylko wtedy, gdy cos jest brudne - jak okno.
fn idle() -> Res {
    let mut scene = Scene::warp(1440, 900).map_err(|e| e.to_string())?;
    let rec = Recording::handwriting(5, (100.0, 200.0), 1, 2, 40.0);
    scene.replay(&rec, 1, false).map_err(|e| e.to_string())?;
    let f0 = scene.renderer.presents();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(2) {
        if scene.dirty != spectre_harness::scene::Dirty::Clean {
            scene.frame().map_err(|e| e.to_string())?;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let n = scene.renderer.presents() - f0;
    println!("klatki w spoczynku przez 2 s: {n}");
    if n != 0 {
        return Err("scena rysuje w spoczynku".into());
    }
    Ok(())
}

/// T2.2: plik `.ops` (np. notatka 3D z nowszej wersji) przez czytnik i zapis
/// **tej** wersji, jak przy otwarciu i dalszym pisaniu: wszystkie operacje
/// zapisane z powrotem, a za nimi jedna nowa kreska tego samego autora.
/// Wynik: liczby operacji, nieznanych, pominietych i czy plik byl uciety.
fn ops_roundtrip(args: &[String]) -> Res {
    use spectre_core::{Document, OpKind, Rgba, Sample, StrokeData};
    use spectre_proto::{OpsReader, OpsWriter};
    let src = PathBuf::from(args.get(1).ok_or("brak pliku wejsciowego")?);
    let dst = PathBuf::from(args.get(2).ok_or("brak pliku wyjsciowego")?);
    let parsed = OpsReader::read_path(&src)
        .map_err(|e| e.to_string())?
        .ok_or("zly naglowek")?;
    let unknown = parsed
        .ops
        .iter()
        .filter(|o| matches!(o.kind, OpKind::Unknown { .. }))
        .count();
    let mut doc = Document::new(parsed.header.author);
    for op in &parsed.ops {
        doc.apply(op);
    }
    let _ = std::fs::remove_file(&dst);
    let mut w = OpsWriter::open(&dst, parsed.header.author, parsed.header.base_lamport)
        .map_err(|e| e.to_string())?;
    for op in &parsed.ops {
        w.append(op);
    }
    let new = doc.add_stroke(StrokeData {
        tool: 0,
        color: Rgba::rgb(216, 216, 216),
        base_width: 3.2,
        samples: (0..20)
            .map(|i| Sample {
                x: 100.0 + i as f32 * 3.0,
                y: 100.0,
                pressure: 0.5,
                t_us: i * 3760,
                ..Default::default()
            })
            .collect(),
    });
    w.append(&new);
    w.sync().map_err(|e| e.to_string())?;
    let id = match new.kind {
        OpKind::StrokeAdd { id, .. } => id.seq,
        _ => 0,
    };
    println!(
        "operacje {} (nieznane {unknown}, pominiete {}), uciety: {}, zywe kreski: {}, nowa kreska seq {id} lamport {}",
        parsed.ops.len(),
        parsed.skipped,
        parsed.truncated,
        doc.live_count(),
        new.lamport
    );
    Ok(())
}
