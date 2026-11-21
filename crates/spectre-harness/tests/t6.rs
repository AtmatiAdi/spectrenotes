//! T6.5 (obraz): wejscie w 3D, rysowanie na obroconej plaszczyznie, obrot
//! kamery i wyjscie przywracaja notatke 2D piksel w piksel (zoom, miejsce).
//! Logika przyciskow (3D, "2D", blokada) jest w `spectre-app/src/three.rs`
//! i `app.rs`; tu sprawdzamy obraz i kamere ta sama droga rysowania.

use spectre_core::space::compass::{self, Handle};
use spectre_harness::notes::{reference_notes, render};
use spectre_harness::room::draw_local;
use spectre_harness::{png, Scene};

#[test]
fn t6_5_wyjscie_z_3d_piksel_w_piksel() {
    for name in ["03-linie-zoom", "06-pdf", "10-gumka-cofanie"] {
        let notes = reference_notes();
        let note = notes.iter().find(|n| n.name == name).unwrap();
        let mut s = Scene::warp(note.size.0, note.size.1).unwrap();
        let before = render(note, &mut s).unwrap();
        let cam = s.cam;
        s.enter_3d(None);
        {
            let st = s.three.as_mut().unwrap();
            compass::apply(&mut st.draw, Handle::RingY, 1.0, 1);
            compass::apply(&mut st.draw, Handle::RingX, -1.0, 2);
        }
        s.touch_3d(true);
        s.frame().unwrap();
        draw_local(&mut s, &[(-50.0, 0.0), (50.0, 30.0), (80.0, -20.0)], 3).unwrap();
        s.three.as_mut().unwrap().cam.orbit(35.0, -20.0);
        s.touch_3d(false);
        s.frame().unwrap();
        s.three.as_mut().unwrap().cam.zoom *= 1.7;
        s.touch_3d(true);
        s.frame().unwrap();
        s.exit_3d();
        assert_eq!(s.cam, cam, "{name}: kamera 2D");
        s.frame().unwrap();
        let after = s.pixels().unwrap();
        assert_eq!(
            png::diff_pixels(&before, &after),
            0,
            "{name}: obraz 2D po wyjsciu z 3D"
        );
    }
}
