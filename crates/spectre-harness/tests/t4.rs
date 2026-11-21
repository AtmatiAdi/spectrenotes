//! T4 w CI (WARP): scenariusz pokoju bez wzorcow [U] - plaszczyzny powstaja
//! same, po jednej na sciane; kreska na plaszczyznie z listy nie tworzy nowej;
//! kadr bez migotania. Pelny przebieg ze zrzutami: `spectre-harness t4`.

use spectre_core::OpKind;
use spectre_harness::room::{build_room, grid, room_camera};
use spectre_harness::{png, Scene};

#[test]
fn t4_pokoj_plaszczyzny_i_migotanie() {
    let (w, h) = (640u32, 400u32);
    let mut scene = Scene::warp(w, h).unwrap();
    scene.enter_3d(grid());
    let log = build_room(&mut scene).unwrap();
    let got: Vec<usize> = log.planes_after.iter().map(|x| x.1).collect();
    assert_eq!(got, [1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 5]);
    let adds = scene
        .ops
        .iter()
        .filter(|o| matches!(o.kind, OpKind::PlaneAdd { .. }))
        .count();
    assert_eq!(adds, 5);
    scene.three.as_mut().unwrap().cam = room_camera(w as f64, h as f64);
    scene.touch_3d(true);
    scene.frame().unwrap();
    let a = scene.pixels().unwrap();
    scene.touch_3d(true);
    scene.frame().unwrap();
    let b = scene.pixels().unwrap();
    assert_eq!(png::diff_pixels(&a, &b), 0);
    assert!(
        a.chunks_exact(4).filter(|p| p[0] > 150).count() > 1000,
        "pokoj widoczny"
    );
    // Wyjscie z 3D wraca do kamery 2D sprzed wejscia.
    let before = scene.three.as_ref().unwrap().saved_2d;
    scene.exit_3d();
    assert_eq!(scene.cam, before);
}
