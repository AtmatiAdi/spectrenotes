//! `spectre-harness` - uprzaz testowa z wiersza polecen (`docs/08-UPRZAZ-TESTOWA.md`).
//!
//! ```text
//! spectre-harness t1 <katalog> [--png]       obrazy i bajty notatek wzorcowych
//! spectre-harness t1-compare <a> <b>         roznica dwoch przebiegow t1
//! spectre-harness latency [--warp] [--rec plik] [--per-msg N] [--strokes N]
//! spectre-harness record <plik>              zapis syntetycznego nagrania piora
//! spectre-harness idle                       licznik klatek w spoczynku (T5.3)
//! spectre-harness ops-roundtrip <we> <wy>    plik .ops przez czytnik tej wersji (T2.2)
//! spectre-harness t2-write <plik>            notatka z 5 plaszczyznami (T2.1, T2.2)
//! spectre-harness t2-check <plik> [<wzor>]   plaszczyzny i kreski po odczycie (T2.2)
//! spectre-harness t4 <katalog>               scenariusz pokoju w 3D, zrzuty co 90 st. (T4)
//! spectre-harness t5 [--warp]                wydajnosc 3D: obrot, przepieczenie, pamiec, pioro (T5)
//! ```

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use spectre_core::space::Pose;
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
        "t2-write" => t2_write(&args),
        "t2-check" => t2_check(&args),
        "t4" => t4(&args),
        "t5" => t5(&args),
        "dbg3d" => dbg3d(&args),
        "t51" => t51(&args),
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

/// Notatka wzorcowa T2: kreski na plaszczyznie zerowej i na pieciu
/// plaszczyznach ustawionych kompasem, jeden autor.
fn t2_note() -> Vec<spectre_core::Op> {
    use spectre_core::space::compass::{self, Handle};
    use spectre_core::space::{Pose, V3};
    use spectre_core::Document;
    let mut d = Document::new(spectre_harness::notes::AUTHOR);
    let mut ops = Vec::new();
    let rec = Recording::handwriting(5, (100.0, 100.0), 2, 3, 30.0);
    let data = |s: &Vec<spectre_proto::Sample>| spectre_proto::StrokeData {
        tool: 0,
        color: spectre_proto::Rgba::rgb(216, 216, 216),
        base_width: 3.2,
        samples: s.clone(),
    };
    for s in &rec.strokes {
        ops.extend(d.add_stroke_on(&Pose::ZERO, data(s)));
    }
    for k in 0..5 {
        let mut p = Pose::new(0.0, 0.0, V3::new(300.0 * k as f64, 0.0, 0.0));
        for _ in 0..=k {
            compass::apply(&mut p, Handle::RingY, 1.0, 1);
        }
        compass::apply(&mut p, Handle::RingX, -1.0, 2);
        compass::apply(&mut p, Handle::ArrowN, 1.0, 0);
        for s in &rec.strokes {
            ops.extend(d.add_stroke_on(&p, data(s)));
        }
    }
    ops
}

fn t2_write(args: &[String]) -> Res {
    let path = PathBuf::from(args.get(1).ok_or("brak pliku")?);
    let bytes = spectre_harness::notes::ops_bytes(&t2_note());
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    println!("zapisano {} B", bytes.len());
    Ok(())
}

/// Odczyt notatki 3D: plaszczyzny, kreski na kazdej i czy wszystkie operacje
/// wzorca (`<wzor>`, np. plik z `t2-write`) sa w pliku bez zmian.
fn t2_check(args: &[String]) -> Res {
    use spectre_core::Document;
    use spectre_proto::OpsReader;
    let path = PathBuf::from(args.get(1).ok_or("brak pliku")?);
    let parsed = OpsReader::read_path(&path)
        .map_err(|e| e.to_string())?
        .ok_or("zly naglowek")?;
    let mut d = Document::new(parsed.header.author);
    for op in &parsed.ops {
        d.apply(op);
    }
    let planes = d.planes();
    println!(
        "plaszczyzny {}: kreski {:?}; zerowa {}; uciety {}, pominiete {}",
        planes.len(),
        planes.iter().map(|p| p.live).collect::<Vec<_>>(),
        d.live_count(),
        parsed.truncated,
        parsed.skipped
    );
    if let Some(w) = args.get(2) {
        let want = OpsReader::read_path(Path::new(w))
            .map_err(|e| e.to_string())?
            .ok_or("zly wzor")?;
        let missing = want.ops.iter().filter(|o| !parsed.ops.contains(o)).count();
        let file = std::fs::read(&path).map_err(|e| e.to_string())?;
        let base = std::fs::read(w).map_err(|e| e.to_string())?;
        let prefix = file.starts_with(&base);
        println!("operacje wzorca brakujace: {missing}; wzorzec jest poczatkiem pliku bajt w bajt: {prefix}");
        if missing > 0 || !prefix || planes.len() != 5 {
            return Err("T2.2 NIE".into());
        }
        println!("T2.2 ok");
    }
    Ok(())
}

fn ms(t: Instant) -> f32 {
    t.elapsed().as_secs_f32() * 1000.0
}

/// T4: scenariusz pokoju na WARP. Zapisuje zrzuty co 90 stopni do `<katalog>`.
fn t4(args: &[String]) -> Res {
    use spectre_core::{OpKind, PlaneId};
    use spectre_harness::room::{build_room, grid, room_camera};
    let dir = PathBuf::from(args.get(1).ok_or("brak katalogu")?);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (w, h) = (1440u32, 900u32);
    let e = |e: windows::core::Error| e.to_string();
    let mut scene = Scene::warp(w, h).map_err(e)?;
    scene.enter_3d(grid());
    let log = build_room(&mut scene).map_err(e)?;
    let mut fails = Vec::new();
    println!("plaszczyzny po krokach: {:?}", log.planes_after);
    let expect = [1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 5];
    let got: Vec<usize> = log.planes_after.iter().map(|x| x.1).collect();
    if got != expect {
        fails.push(format!("liczba plaszczyzn {got:?} zamiast {expect:?}"));
    }
    let plane_ops = scene
        .ops
        .iter()
        .filter(|o| matches!(o.kind, OpKind::PlaneAdd { .. }))
        .count();
    println!(
        "operacje PlaneAdd: {plane_ops} (5 = jedna na sciane/podloge, bez przycisku tworzenia)"
    );
    if plane_ops != 5 {
        fails.push("PlaneAdd != 5".into());
    }
    let log_draw = scene.three.as_ref().unwrap().draw;
    // Obrot kamery o 360 stopni w 36 krokach, zrzut co 90.
    {
        let st = scene.three.as_mut().unwrap();
        st.cam = room_camera(w as f64, h as f64);
    }
    let mut times = Vec::new();
    let mut shots = Vec::new();
    for step in 0..36 {
        let settled = step % 9 == 0;
        scene.touch_3d(settled);
        let t = Instant::now();
        scene.frame().map_err(e)?;
        times.push(ms(t));
        if settled {
            let px = scene.pixels().map_err(e)?;
            let name = format!("pokoj-{:03}.png", step * 10);
            png::write(&dir.join(&name), w, h, &px).map_err(|e| e.to_string())?;
            // Migotanie: ten sam kadr jeszcze raz (pelne zlozenie) - identyczny.
            scene.touch_3d(true);
            scene.frame().map_err(e)?;
            let again = scene.pixels().map_err(e)?;
            let d = png::diff_pixels(&px, &again);
            println!("{name}: kolejny kadr rozni sie o {d} px");
            if d != 0 {
                fails.push(format!("{name} migocze ({d} px)"));
            }
            shots.push(px);
        }
        let st = scene.three.as_mut().unwrap();
        st.cam.orbit(10.0, 0.0);
    }
    let s = spectre_harness::stats::summary(&times);
    println!("klatka obrotu (WARP): {s}");
    // Kreski kazdej plaszczyzny widac przez inne: piksele zapalone, gdy
    // widoczna jest tylko ona, sa zapalone takze w pelnej scenie (z tolerancja
    // 1 px na antyaliasing). Obie sceny pieczone od zera w tej samej kamerze.
    {
        let st = scene.three.as_mut().unwrap();
        st.cam = room_camera(w as f64, h as f64);
        st.grid = None;
        st.draw = Pose::new(0.0, 0.0, spectre_core::space::V3::new(0.0, 0.0, -5000.0));
    }
    scene.renderer.clear_3d();
    scene.touch_3d(true);
    scene.frame().map_err(e)?;
    let full = &scene.pixels().map_err(e)?;
    let lit1 =
        |p: &[u8], i: usize| p[i * 4..i * 4 + 3].iter().map(|&c| c as u32).sum::<u32>() > 120;
    let lit = |p: &[u8], i: usize| {
        if p.as_ptr() != full.as_ptr() {
            return lit1(p, i);
        }
        let (x, y) = ((i % w as usize) as i64, (i / w as usize) as i64);
        (-1..=1).any(|dy| {
            (-1..=1).any(|dx| {
                let (xx, yy) = (x + dx, y + dy);
                xx >= 0
                    && yy >= 0
                    && xx < w as i64
                    && yy < h as i64
                    && lit1(p, (yy * w as i64 + xx) as usize)
            })
        })
    };
    let all_ops = scene.ops.clone();
    let ids: Vec<PlaneId> = scene.doc.planes().iter().map(|p| p.id).collect();
    for only in &ids {
        let mut s2 = Scene::warp(w, h).map_err(e)?;
        s2.load_ops(&all_ops);
        s2.enter_3d(None);
        {
            let st = s2.three.as_mut().unwrap();
            st.cam = room_camera(w as f64, h as f64);
            st.draw = Pose::new(0.0, 0.0, spectre_core::space::V3::new(0.0, 0.0, -5000.0));
        }
        let mut hide = vec![s2.doc.set_plane_prop(PlaneId::ZERO, "visible", "0")];
        for id in &ids {
            if id != only {
                hide.push(s2.doc.set_plane_prop(*id, "visible", "0"));
            }
        }
        s2.touch_3d(true);
        s2.frame().map_err(e)?;
        let alone = s2.pixels().map_err(e)?;
        let n = alone.len() / 4;
        let (on, missing) = (0..n).fold((0, 0), |(on, m), i| {
            if lit(&alone, i) {
                (on + 1, m + usize::from(!lit(full, i)))
            } else {
                (on, m)
            }
        });
        let name = scene.doc.plane_auto_name(*only);
        println!("{name}: kresek {on} px, niewidocznych w pelnej scenie {missing}");
        if missing > 0 {
            fails.push(format!("{name}: {missing} z {on} px zasloniete"));
        }
    }
    // Zapis, zamkniecie, otwarcie: ten sam obraz (obie sceny pieczone od zera).
    {
        let st = scene.three.as_mut().unwrap();
        st.grid = grid();
        st.draw = log_draw;
    }
    // Cache obrysow pamieta zoom, przy ktorym kreske narysowano - po otwarciu
    // go nie ma, wiec porownanie danych idzie bez niego w obu scenach.
    scene.renderer.clear_3d();
    scene.renderer.clear_geometry();
    scene.touch_3d(true);
    scene.frame().map_err(e)?;
    let before = scene.pixels().map_err(e)?;
    let bytes = spectre_harness::notes::ops_bytes(&all_ops);
    std::fs::write(dir.join("pokoj.ops"), &bytes).map_err(|e| e.to_string())?;
    let parsed = spectre_proto::OpsReader::parse(&bytes).ok_or("zly plik")?;
    let mut s3 = Scene::warp(w, h).map_err(e)?;
    s3.load_ops(&parsed.ops);
    s3.enter_3d(grid());
    {
        let a = scene.three.as_ref().unwrap();
        let st = s3.three.as_mut().unwrap();
        st.cam = room_camera(w as f64, h as f64);
        st.draw = log_draw;
        let _ = a;
    }
    s3.touch_3d(true);
    s3.frame().map_err(e)?;
    let reopened = s3.pixels().map_err(e)?;
    // Plik trzyma probki co 1/32 j. (jak w 2D): obraz z pamieci rozni sie od
    // obrazu z pliku tylko antyaliasingiem. Drugi zapis i otwarcie - identyczny.
    // Wyraznie inny = zaden piksel w otoczeniu 1 px drugiego obrazu nie jest
    // podobny (krawedz cienkiej linii przesunieta o ulamek piksela to nie zmiana).
    let px_at = |p: &[u8], x: i64, y: i64| -> Option<[u8; 3]> {
        (x >= 0 && y >= 0 && x < w as i64 && y < h as i64).then(|| {
            let i = ((y * w as i64 + x) * 4) as usize;
            [p[i], p[i + 1], p[i + 2]]
        })
    };
    let close = |a: [u8; 3], b: [u8; 3]| a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= 64);
    let mut strong = 0;
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let a = px_at(&before, x, y).unwrap();
            if close(a, px_at(&reopened, x, y).unwrap()) {
                continue;
            }
            let near = (-1..=1).any(|dy| {
                (-1..=1).any(|dx| px_at(&reopened, x + dx, y + dy).is_some_and(|b| close(a, b)))
            });
            if !near {
                strong += 1;
            }
        }
    }
    let soft = png::diff_pixels(&before, &reopened);
    let bytes2 =
        spectre_harness::notes::ops_bytes(&spectre_proto::OpsReader::parse(&bytes).unwrap().ops);
    let mut s4 = Scene::warp(w, h).map_err(e)?;
    s4.load_ops(
        &spectre_proto::OpsReader::parse(&bytes2)
            .ok_or("zly plik")?
            .ops,
    );
    s4.enter_3d(grid());
    {
        let st = s4.three.as_mut().unwrap();
        st.cam = room_camera(w as f64, h as f64);
        st.draw = log_draw;
    }
    s4.touch_3d(true);
    s4.frame().map_err(e)?;
    let again = s4.pixels().map_err(e)?;
    let d2 = png::diff_pixels(&reopened, &again);
    println!(
        "po zapisie i otwarciu: wzgledem pamieci {soft} px antyaliasingu ({:.2}%), {strong} px wyraznie innych; drugi zapis i otwarcie: {d2} px (plik bajt w bajt: {})",
        soft as f64 * 100.0 / (w * h) as f64,
        bytes2 == bytes
    );
    if strong != 0 || d2 != 0 || bytes2 != bytes {
        fails.push(format!("po otwarciu inny obraz ({strong} / {d2} px)"));
    }
    if fails.is_empty() {
        println!("T4 (bez wzorcow [U]): zaliczony");
        Ok(())
    } else {
        Err(format!("T4 NIE: {fails:?}"))
    }
}

/// T5: wydajnosc 3D na karcie (scena 50 plaszczyzn, 20 tys. kresek).
fn t5(args: &[String]) -> Res {
    use spectre_core::space::compass::{self, Handle};
    use spectre_core::space::{Camera3, FOV_DEG, V3};
    use spectre_harness::room::perf_scene;
    let warp = args.iter().any(|a| a == "--warp");
    let (w, h) = (2880u32, 1800u32);
    let e = |e: windows::core::Error| e.to_string();
    let mut scene = if warp {
        Scene::warp(w, h)
    } else {
        Scene::gpu(w, h)
    }
    .map_err(e)?;
    println!("adapter: {}", scene.renderer.adapter_name());
    perf_scene(&mut scene, 50, 400);
    println!(
        "plaszczyzny {}, kreski {}",
        scene.doc.planes().len(),
        scene.doc.live_count_all()
    );
    scene.enter_3d(Some(spectre_core::background::Background::default()));
    {
        let st = scene.three.as_mut().unwrap();
        st.cam = Camera3 {
            target: V3::new(0.0, 0.0, 0.0),
            yaw: 20.0,
            pitch: -20.0,
            zoom: 1.6,
            fov: FOV_DEG,
            w: w as f64,
            h: h as f64,
        };
    }
    let t = Instant::now();
    let mut frames = 0;
    loop {
        scene.frame().map_err(e)?;
        frames += 1;
        if !scene.three.as_ref().unwrap().dirty {
            break;
        }
    }
    scene.renderer.gpu_wait().map_err(e)?;
    println!(
        "pierwsze pieczenie calej sceny (zimny cache obrysow): {:.1} ms w {frames} klatkach",
        ms(t)
    );
    // T5.2: obrot kostka - same skladania.
    let mut frame = Vec::new();
    let (mut bake, mut draw, mut baked) = (Vec::new(), Vec::new(), 0);
    for _ in 0..240 {
        scene.three.as_mut().unwrap().cam.orbit(1.5, 0.1);
        scene.touch_3d(false);
        let t = Instant::now();
        scene.frame().map_err(e)?;
        scene.renderer.gpu_wait().map_err(e)?;
        frame.push(ms(t));
        let l = scene.three.as_ref().unwrap().last;
        bake.push(l.bake_ms);
        draw.push(l.draw_ms);
        baked += l.baked;
    }
    println!(
        "  w tym pieczenie CPU {}, skladanie CPU {}, przepieczonych {baked}",
        spectre_harness::stats::summary(&bake),
        spectre_harness::stats::summary(&draw)
    );
    let s = spectre_harness::stats::summary(&frame);
    println!(
        "T5.2 klatka obrotu: {s}; {:.0} kl./s srednio",
        1000.0 / s.mean
    );
    // T5.4: przepieczenie po zatrzymaniu.
    scene.three.as_mut().unwrap().cam.orbit(35.0, -10.0);
    scene.touch_3d(false);
    scene.frame().map_err(e)?;
    scene.touch_3d(true);
    let t = Instant::now();
    let (mut frames, mut worst, mut baked) = (0, 0f32, 0);
    loop {
        let tf = Instant::now();
        scene.frame().map_err(e)?;
        scene.renderer.gpu_wait().map_err(e)?;
        worst = worst.max(ms(tf));
        frames += 1;
        baked += scene.three.as_ref().unwrap().last.baked;
        if !scene.three.as_ref().unwrap().dirty {
            break;
        }
    }
    let rebake = ms(t);
    let st = scene.three.as_ref().unwrap().last;
    println!(
        "T5.4 przepieczenie: {rebake:.1} ms w {frames} klatkach (najdluzsza {worst:.1} ms), przepieczone {baked} z {}",
        st.planes
    );
    // T5.5: pamiec GPU - bitmapy plaszczyzn + warstwy okna.
    let window = 4 * (w as usize * h as usize * 4);
    println!(
        "T5.5 pamiec: bitmapy plaszczyzn {:.0} MB + bufory okna {:.0} MB",
        scene.renderer.tex3d_bytes() as f64 / 1048576.0,
        window as f64 / 1048576.0
    );
    // T5.1: pioro na plaszczyznie pod katem 30 stopni.
    {
        let st = scene.three.as_mut().unwrap();
        let mut p = spectre_core::space::Pose::new(0.0, 0.0, st.cam.target);
        compass::apply(&mut p, Handle::RingX, -1.0, 1);
        compass::apply(&mut p, Handle::RingX, -1.0, 1);
        st.draw = p;
        st.cam.yaw = 0.0;
        st.cam.pitch = 0.0;
        // Ta sama skala co notatka 2D odniesienia (100%).
        st.cam.zoom = 1.0;
    }
    scene.touch_3d(true);
    scene.frame().map_err(e)?;
    let rec = Recording::handwriting(7, (500.0, 500.0), 6, 6, 40.0);
    scene
        .replay(
            &Recording::handwriting(3, (400.0, 1400.0), 1, 3, 40.0),
            1,
            true,
        )
        .map_err(e)?;
    let c0 = scene.three.as_ref().unwrap().composes;
    let lat3 = scene.replay(&rec, 1, true).map_err(e)?;
    println!(
        "  skladan sceny podczas pisania: {} na {} kresek",
        scene.three.as_ref().unwrap().composes - c0,
        rec.strokes.len()
    );
    println!(
        "T5.1 pioro 3D (30 st., 50 plaszczyzn): do present {}",
        lat3.cpu
    );
    println!(
        "T5.1 pioro 3D (30 st., 50 plaszczyzn): do klatki   {}",
        lat3.gpu
    );
    // Odniesienie 2D w tym samym przebiegu.
    let mut s2 = if warp {
        Scene::warp(w, h)
    } else {
        Scene::gpu(w, h)
    }
    .map_err(e)?;
    let notes = reference_notes();
    let dense = notes.iter().find(|n| n.name == "08-gesta-cienka").unwrap();
    s2.load_ops(&dense.ops);
    s2.cam.home(w as f64 as f32);
    s2.frame().map_err(e)?;
    s2.replay(
        &Recording::handwriting(3, (400.0, 1400.0), 1, 3, 40.0),
        1,
        true,
    )
    .map_err(e)?;
    let lat2 = s2.replay(&rec, 1, true).map_err(e)?;
    println!(
        "T5.1 pioro 2D (odniesienie):           do present {}",
        lat2.cpu
    );
    println!(
        "T5.1 pioro 2D (odniesienie):           do klatki   {}",
        lat2.gpu
    );
    println!(
        "T5.1 roznica 3D - 2D: do klatki p50 {:+.3} ms, p99 {:+.3} ms",
        lat3.gpu.p50 - lat2.gpu.p50,
        lat3.gpu.p99 - lat2.gpu.p99
    );
    // T5.3: spoczynek.
    let f0 = scene.renderer.presents();
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(2) {
        let dirty = scene.three.as_ref().unwrap().dirty;
        if dirty {
            scene.frame().map_err(e)?;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    println!(
        "T5.3 klatki w spoczynku (2 s): {}",
        scene.renderer.presents() - f0
    );
    Ok(())
}

/// Diagnostyka: pokoj z siatka, kolejno bez plaszczyzn i z sama podloga.
fn dbg3d(args: &[String]) -> Res {
    use spectre_core::PlaneId;
    use spectre_harness::room::{build_room, grid, room_camera};
    let dir = PathBuf::from(args.get(1).ok_or("brak katalogu")?);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let e = |e: windows::core::Error| e.to_string();
    let (w, h) = (1440u32, 900u32);
    let mut scene = Scene::warp(w, h).map_err(e)?;
    scene.enter_3d(grid());
    build_room(&mut scene).map_err(e)?;
    scene.three.as_mut().unwrap().cam = room_camera(w as f64, h as f64);
    let ids: Vec<PlaneId> = scene.doc.planes().iter().map(|p| p.id).collect();
    for (k, id) in ids.iter().enumerate() {
        scene
            .doc
            .set_plane_prop(*id, "visible", if k == 0 { "1" } else { "0" });
    }
    scene.touch_3d(true);
    scene.frame().map_err(e)?;
    let px = scene.pixels().map_err(e)?;
    png::write(&dir.join("tylko-podloga.png"), w, h, &px).map_err(|e| e.to_string())?;
    scene.doc.set_plane_prop(ids[0], "visible", "0");
    scene.touch_3d(true);
    scene.frame().map_err(e)?;
    let px = scene.pixels().map_err(e)?;
    png::write(&dir.join("bez-plaszczyzn.png"), w, h, &px).map_err(|e| e.to_string())?;
    for id in &ids {
        scene.doc.set_plane_prop(*id, "visible", "1");
    }
    {
        let st = scene.three.as_mut().unwrap();
        st.grid = None;
        st.grid3d = true;
        // Plaszczyzna rysowania: lewa sciana, srodek przy oknie.
        st.draw = Pose::new(0.0, 90.0, spectre_core::space::V3::new(0.0, -125.0, 250.0));
    }
    scene.touch_3d(true);
    scene.frame().map_err(e)?;
    let px = scene.pixels().map_err(e)?;
    png::write(&dir.join("ramka-siatka3d.png"), w, h, &px).map_err(|e| e.to_string())?;
    Ok(())
}

/// T5.1 dokladniej: 2D (gesta notatka) i 3D (50 plaszczyzn, rysowanie na
/// plaszczyznie pod katem 30 st.) na przemian, `--rounds` rund; mediany
/// p50 i p99 z rund. Te same nagranie i paczki co w `latency`.
fn t51(args: &[String]) -> Res {
    use spectre_core::space::compass::{self, Handle};
    use spectre_core::space::{Camera3, FOV_DEG, V3};
    use spectre_harness::room::perf_scene;
    let rounds: usize = arg(args, "--rounds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    let (w, h) = (2880u32, 1800u32);
    let e = |e: windows::core::Error| e.to_string();
    let mut s3 = Scene::gpu(w, h).map_err(e)?;
    perf_scene(&mut s3, 50, 400);
    s3.enter_3d(Some(spectre_core::background::Background::default()));
    {
        let st = s3.three.as_mut().unwrap();
        st.cam = Camera3 {
            target: V3::new(0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            // Ta sama skala co notatka 2D odniesienia (100%) w glebi celu.
            zoom: 1.0,
            fov: FOV_DEG,
            w: w as f64,
            h: h as f64,
        };
        let mut p = Pose::new(0.0, 0.0, st.cam.target);
        compass::apply(&mut p, Handle::RingX, -1.0, 1);
        compass::apply(&mut p, Handle::RingX, -1.0, 1);
        st.draw = p;
    }
    while {
        s3.frame().map_err(e)?;
        s3.three.as_ref().unwrap().dirty
    } {}
    let mut s2 = Scene::gpu(w, h).map_err(e)?;
    let notes = reference_notes();
    let dense = notes.iter().find(|n| n.name == "08-gesta-cienka").unwrap();
    s2.load_ops(&dense.ops);
    s2.cam.home(w as f32);
    s2.frame().map_err(e)?;
    let rec = Recording::handwriting(7, (500.0, 500.0), 3, 6, 40.0);
    let warm = Recording::handwriting(3, (400.0, 1400.0), 1, 3, 40.0);
    s2.replay(&warm, 1, true).map_err(e)?;
    s3.replay(&warm, 1, true).map_err(e)?;
    let (mut a50, mut a99, mut b50, mut b99) = (vec![], vec![], vec![], vec![]);
    let (mut c2, mut c3) = (vec![], vec![]);
    for r in 0..rounds {
        let l2 = s2.replay(&rec, 1, true).map_err(e)?;
        let l3 = s3.replay(&rec, 1, true).map_err(e)?;
        println!(
            "runda {r}: 2D p50 {:.3} p99 {:.3} | 3D p50 {:.3} p99 {:.3} ms (do klatki gotowej)",
            l2.gpu.p50, l2.gpu.p99, l3.gpu.p50, l3.gpu.p99
        );
        if r == rounds - 1 {
            for (name, s) in [("2D", &s2), ("3D", &s3)] {
                let mut t = s.trace.clone();
                t.sort_by(|a, b| b.0.total_cmp(&a.0));
                let top = &t[..t.len() / 100];
                let burns = top.iter().filter(|x| x.1).count();
                let early = top.iter().filter(|x| x.2 < 3).count();
                let all_burn: Vec<f32> = s.trace.iter().filter(|x| x.1).map(|x| x.0).collect();
                println!(
                    "  {name}: najwolniejszy 1%: {} klatek, z wypaleniem {burns}, pierwsze 3 probki kreski {early}; wypalenia: {}",
                    top.len(),
                    spectre_harness::stats::summary(&all_burn)
                );
                let desc: Vec<String> = top
                    .iter()
                    .take(12)
                    .map(|x| format!("{:.2}/{:.2}@{}", x.0, x.3, x.2))
                    .collect();
                println!("    gpu/cpu@probka: {}", desc.join(" "));
            }
        }
        c2.push((l2.cpu.p50, l2.cpu.p99));
        c3.push((l3.cpu.p50, l3.cpu.p99));
        a50.push(l2.gpu.p50);
        a99.push(l2.gpu.p99);
        b50.push(l3.gpu.p50);
        b99.push(l3.gpu.p99);
    }
    let med = |v: &mut Vec<f32>| {
        v.sort_by(|a, b| a.total_cmp(b));
        v[v.len() / 2]
    };
    let (m2_50, m2_99, m3_50, m3_99) = (med(&mut a50), med(&mut a99), med(&mut b50), med(&mut b99));
    println!(
        "do klatki gotowej, mediana: 2D p50 {m2_50:.3} p99 {m2_99:.3} | 3D p50 {m3_50:.3} p99 {m3_99:.3} ms | roznica p50 {:+.3} p99 {:+.3} ms",
        m3_50 - m2_50,
        m3_99 - m2_99
    );
    let v = |c: &Vec<(f32, f32)>, i: usize| {
        let mut x: Vec<f32> = c.iter().map(|p| if i == 0 { p.0 } else { p.1 }).collect();
        med(&mut x)
    };
    let (p2, q2, p3, q3) = (v(&c2, 0), v(&c2, 1), v(&c3, 0), v(&c3, 1));
    println!(
        "do Present (jak HUD), mediana: 2D p50 {p2:.3} p99 {q2:.3} | 3D p50 {p3:.3} p99 {q3:.3} ms | roznica p50 {:+.3} p99 {:+.3} ms",
        p3 - p2,
        q3 - q2
    );
    Ok(())
}
