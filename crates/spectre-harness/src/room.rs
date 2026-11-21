//! Scenariusz pokoju (T4) i scena wydajnosci (T5) w trybie 3D.
//!
//! Pokoj powstaje tak, jak zrobilby to uzytkownik:
//! - plaszczyzna rysowania ustawiana **tylko kompasem** - najpierw obrot
//!   wokol srodka, potem osobne przesuniecie (W17), bez przycisku "nowa
//!   plaszczyzna" (W9);
//! - kamera na wprost plaszczyzny (jak klik w sciane kostki) i kreski
//!   z odtwarzanego ruchu piora: punkty plaszczyzny rzutowane na ekran,
//!   paczkami jak `WM_POINTER`, przez rzut odwrotny z powrotem na plaszczyzne.
//!
//! Swiat: y w dol. Podloga y = 0, x 0..400, z 100..400 (4 x 3 m, 1 m = 100 j.),
//! sciany wysokie na 250 j.

use spectre_core::background::Background;
use spectre_core::space::compass::{self, Handle};
use spectre_core::space::{Camera3, Pose, V3};
use spectre_core::{PlaneId, Rgba};
use spectre_proto::Sample;
use windows::core::Result;

use crate::pen::{PenMsg, Rng, SAMPLE_US};
use crate::scene::Scene;

/// Kroki kompasu: (uchwyt, kierunek, strefa 0..4, ile razy).
pub type Steps<'a> = &'a [(Handle, f64, usize, usize)];

pub fn compass_steps(pose: &mut Pose, steps: Steps) {
    for &(h, dir, zone, n) in steps {
        for _ in 0..n {
            compass::apply(pose, h, dir, zone);
        }
    }
}

/// Kreska narysowana piorem po plaszczyznie rysowania: lamana w ukladzie
/// plaszczyzny -> piksele ekranu co ~2 px -> odtworzenie paczkami po 2 probki.
pub fn draw_local(scene: &mut Scene, pts: &[(f64, f64)], seed: u32) -> Result<()> {
    let st = scene.three.as_ref().expect("tryb 3D");
    let (cam, draw) = (st.cam, st.draw);
    let mut samples = Vec::new();
    let mut r = Rng(seed | 1);
    let mut t = 0u64;
    for w in pts.windows(2) {
        let (a, b) = (draw.to_world(w[0].0, w[0].1), draw.to_world(w[1].0, w[1].1));
        let (Some(pa), Some(pb)) = (cam.project(a), cam.project(b)) else {
            continue;
        };
        let n = ((pb.0 - pa.0).hypot(pb.1 - pa.1) / 2.0).ceil().max(1.0) as usize;
        for k in 0..n {
            let u = k as f64 / n as f64;
            samples.push(Sample {
                x: (pa.0 + (pb.0 - pa.0) * u) as f32 + (r.unit() - 0.5) * 0.3,
                y: (pa.1 + (pb.1 - pa.1) * u) as f32 + (r.unit() - 0.5) * 0.3,
                pressure: 0.55 + 0.2 * r.unit(),
                t_us: t,
                ..Default::default()
            });
            t += SAMPLE_US;
        }
    }
    if samples.len() < 2 {
        return Ok(());
    }
    for (i, c) in samples.chunks(2).enumerate() {
        scene.pen(&if i == 0 {
            PenMsg::Down(c.to_vec())
        } else {
            PenMsg::Move(c.to_vec())
        });
        scene.frame()?;
    }
    scene.pen(&PenMsg::Up);
    scene.frame()
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<(f64, f64)> {
    vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]
}

/// Kamera na wprost plaszczyzny rysowania (klik w sciane kostki / "patrz na").
fn look(scene: &mut Scene, zoom: f64) {
    let st = scene.three.as_mut().unwrap();
    let d = st.draw;
    st.cam.look_at_plane(&d);
    st.cam.zoom = zoom;
    scene.touch_3d(true);
}

/// Wynik scenariusza: liczba plaszczyzn po kazdym kroku i plaszczyzny scian.
#[derive(Debug, Default)]
pub struct RoomLog {
    pub planes_after: Vec<(String, usize)>,
    pub ids: Vec<(String, PlaneId)>,
}

/// Buduje pokoj w scenie (juz w 3D). Zwraca dziennik krokow.
pub fn build_room(scene: &mut Scene) -> Result<RoomLog> {
    let mut log = RoomLog::default();
    let note = |scene: &Scene, log: &mut RoomLog, what: &str| {
        log.planes_after
            .push((what.into(), scene.doc.planes().len()));
        if let Some(id) = scene.draw_plane_id() {
            log.ids.push((what.into(), id));
        }
    };
    // 1. Podloga: srodek kompasu przeciagniety na (200, 0, 0) po plaszczyznie
    //    zerowej, obrot X -90 (dwa kroki 45 w strefie 1), potem przesuniecie
    //    po plaszczyznie do srodka podlogi (200, 0, 250).
    {
        let st = scene.three.as_mut().unwrap();
        compass::move_center(&mut st.draw, V3::new(200.0, 0.0, 0.0));
        compass_steps(&mut st.draw, &[(Handle::RingX, -1.0, 0, 2)]);
        compass_steps(
            &mut st.draw,
            &[(Handle::ArrowV, -1.0, 0, 2), (Handle::ArrowV, -1.0, 1, 5)],
        );
        assert!((st.draw.origin - V3::new(200.0, 0.0, 250.0)).len() < 1e-6);
        scene.touch_3d(true);
    }
    look(scene, 1.6);
    draw_local(scene, &rect(-200.0, -150.0, 200.0, 150.0), 1)?;
    note(scene, &mut log, "podloga");
    // Deski podlogi.
    for k in 1..4 {
        let v = -150.0 + k as f64 * 75.0;
        draw_local(scene, &[(-190.0, v), (190.0, v)], 10 + k)?;
    }
    note(scene, &mut log, "podloga-deski");

    // 2. Tylna sciana (z = 400): obrot X +90 wokol srodka, potem przesuniecie
    //    wzdluz normalnej o 150 i w gore o 125.
    {
        let st = scene.three.as_mut().unwrap();
        compass_steps(&mut st.draw, &[(Handle::RingX, 1.0, 0, 2)]);
        assert!(
            (st.draw.origin - V3::new(200.0, 0.0, 250.0)).len() < 1e-6,
            "obrot nie rusza srodka"
        );
        compass_steps(
            &mut st.draw,
            &[
                (Handle::ArrowN, 1.0, 0, 1),
                (Handle::ArrowN, 1.0, 1, 5),
                (Handle::ArrowV, -1.0, 0, 1),
                (Handle::ArrowV, -1.0, 1, 2),
                (Handle::ArrowV, -1.0, 2, 5),
            ],
        );
        assert!((st.draw.origin - V3::new(200.0, -125.0, 400.0)).len() < 1e-6);
        scene.touch_3d(true);
    }
    look(scene, 1.6);
    draw_local(scene, &rect(-200.0, -125.0, 200.0, 125.0), 2)?;
    note(scene, &mut log, "sciana-tylna");
    draw_local(scene, &rect(-70.0, -70.0, 70.0, 20.0), 3)?;
    draw_local(scene, &[(0.0, -70.0), (0.0, 20.0)], 4)?;
    draw_local(scene, &[(-70.0, -25.0), (70.0, -25.0)], 5)?;
    note(scene, &mut log, "okno-tylne");

    // 3. Przednia sciana (z = 100): przesuniecie wzdluz normalnej o -300.
    {
        let st = scene.three.as_mut().unwrap();
        compass_steps(&mut st.draw, &[(Handle::ArrowN, -1.0, 0, 3)]);
        scene.touch_3d(true);
    }
    look(scene, 1.6);
    draw_local(scene, &rect(-200.0, -125.0, 200.0, 125.0), 6)?;
    note(scene, &mut log, "sciana-przednia");
    draw_local(
        scene,
        &[(-45.0, 125.0), (-45.0, -30.0), (45.0, -30.0), (45.0, 125.0)],
        7,
    )?;
    note(scene, &mut log, "drzwi");

    // 4. Lewa sciana (x = 0): obrot Y +90 wokol srodka, potem przesuniecie
    //    wzdluz normalnej o -200 i po scianie o -150.
    {
        let st = scene.three.as_mut().unwrap();
        compass_steps(&mut st.draw, &[(Handle::RingY, 1.0, 0, 2)]);
        compass_steps(
            &mut st.draw,
            &[
                (Handle::ArrowN, -1.0, 0, 2),
                (Handle::ArrowU, -1.0, 0, 1),
                (Handle::ArrowU, -1.0, 1, 5),
            ],
        );
        assert!((st.draw.origin - V3::new(0.0, -125.0, 250.0)).len() < 1e-6);
        scene.touch_3d(true);
    }
    look(scene, 1.6);
    draw_local(scene, &rect(-150.0, -125.0, 150.0, 125.0), 8)?;
    note(scene, &mut log, "sciana-lewa");
    draw_local(scene, &rect(-60.0, -60.0, 60.0, 10.0), 9)?;
    note(scene, &mut log, "okno-lewe");

    // 5. Prawa sciana (x = 400): przesuniecie wzdluz normalnej o 400.
    {
        let st = scene.three.as_mut().unwrap();
        compass_steps(&mut st.draw, &[(Handle::ArrowN, 1.0, 0, 4)]);
        scene.touch_3d(true);
    }
    look(scene, 1.6);
    draw_local(scene, &rect(-150.0, -125.0, 150.0, 125.0), 20)?;
    note(scene, &mut log, "sciana-prawa");
    draw_local(scene, &rect(-50.0, -50.0, 50.0, 30.0), 21)?;
    draw_local(scene, &[(0.0, -50.0), (0.0, 30.0)], 22)?;
    note(scene, &mut log, "okno-prawe");

    // 6. Wybor podlogi z listy (W14): plaszczyzna rysowania = podloga,
    //    kamera bez zmian; kreska nie tworzy nowej plaszczyzny.
    {
        let floor = log.ids.iter().find(|(w, _)| w == "podloga").unwrap().1;
        let pose = scene.doc.plane_pose(floor).unwrap();
        let st = scene.three.as_mut().unwrap();
        st.draw = pose;
        scene.touch_3d(true);
    }
    look(scene, 1.6);
    // Dywan na podlodze.
    draw_local(scene, &rect(-80.0, -50.0, 80.0, 50.0), 30)?;
    note(scene, &mut log, "dywan-z-listy");
    Ok(log)
}

/// Kamera ogladajaca pokoj z ukosa (start obrotu T4).
pub fn room_camera(w: f64, h: f64) -> Camera3 {
    Camera3 {
        target: V3::new(200.0, -125.0, 250.0),
        yaw: 30.0,
        pitch: -25.0,
        zoom: 1.1,
        fov: spectre_core::space::FOV_DEG,
        w,
        h,
    }
}

/// Siatka jak w ustawieniach 2D (kratka 40 j.).
pub fn grid() -> Option<Background> {
    Some(Background::default())
}

/// Scena wydajnosci T5: `planes` plaszczyzn (pionowych i poziomych, w kolo
/// celu) po `per_plane` kresek pisma (lacznie ok. 20 tys.).
pub fn perf_scene(scene: &mut Scene, planes: usize, per_plane: usize) {
    let mut r = Rng(0x51D3);
    let color = Rgba::rgb(216, 216, 216);
    for p in 0..planes {
        let a = p as f64 / planes as f64 * 360.0;
        let mut pose = Pose::new(0.0, 0.0, V3::new(0.0, 0.0, 0.0));
        // Co trzecia pozioma, reszta pionowe dookola, rozne odleglosci.
        if p % 3 == 0 {
            pose.rx = -90.0;
            pose.origin = V3::new(0.0, 200.0 - (p as f64) * 12.0, 0.0);
        } else {
            pose.ry = a;
            let rad = 300.0 + (p % 5) as f64 * 90.0;
            pose.origin = V3::new(rad * a.to_radians().sin(), 0.0, rad * a.to_radians().cos());
        }
        for k in 0..per_plane {
            let (x0, y0) = (
                r.unit() as f64 * 500.0 - 250.0,
                r.unit() as f64 * 350.0 - 175.0,
            );
            let mut s = Vec::with_capacity(30);
            let (mut x, mut y) = (x0, y0);
            for i in 0..30u64 {
                x += (r.unit() as f64 - 0.5) * 8.0;
                y += (r.unit() as f64 - 0.5) * 6.0;
                s.push(Sample {
                    x: x as f32,
                    y: y as f32,
                    pressure: 0.3 + 0.5 * r.unit(),
                    t_us: i * SAMPLE_US,
                    ..Default::default()
                });
            }
            let _ = k;
            scene.doc.add_stroke_on(
                &pose,
                spectre_proto::StrokeData {
                    tool: 0,
                    color,
                    base_width: 2.4,
                    samples: s,
                },
            );
        }
    }
}
