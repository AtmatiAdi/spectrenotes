//! T1 w CI: plaszczyzna zerowa bez zmian wzgledem v0.4.5.
//!
//! - bajty plikow `.ops` notatek wzorcowych (T1.2) - zawsze, bo to czysty CPU;
//! - obraz WARP (T1.1) - porownanie ze skrotem z wzorca tylko na maszynie
//!   odniesienia (`SPECTRENOTES_GOLDEN_IMG=1`): WARP innej wersji Windows
//!   moze roznic sie o piksele antyaliasingu. Pelne porownanie piksel
//!   w piksel z przebiegiem v0.4.5: `spectre-harness t1-compare` (docs/08).

use spectre_harness::notes::{ops_bytes, reference_notes, render};
use spectre_harness::{png, Scene};

fn golden() -> Vec<(String, u64, u64, usize)> {
    include_str!("../golden/t1-v0.4.5.txt")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split_whitespace();
            let name = it.next().unwrap().to_string();
            let _size = it.next();
            let field = |s: Option<&str>, k: &str| s.unwrap().strip_prefix(k).unwrap().to_string();
            let img = u64::from_str_radix(&field(it.next(), "img="), 16).unwrap();
            let ops = u64::from_str_radix(&field(it.next(), "ops="), 16).unwrap();
            let len = field(it.next(), "ops_len=").parse().unwrap();
            (name, img, ops, len)
        })
        .collect()
}

#[test]
fn t1_2_plik_notatki_2d_bajt_w_bajt_jak_v045() {
    let golden = golden();
    let notes = reference_notes();
    assert_eq!(golden.len(), notes.len());
    for (note, (name, _, ops, len)) in notes.iter().zip(&golden) {
        assert_eq!(note.name, name);
        let bytes = ops_bytes(&note.ops);
        assert_eq!(bytes.len(), *len, "{name}: dlugosc pliku");
        assert_eq!(png::fnv(&bytes), *ops, "{name}: bajty pliku");
    }
}

#[test]
fn t1_1_obraz_powtarzalny_i_zgodny_z_wzorcem() {
    let check_golden = std::env::var_os("SPECTRENOTES_GOLDEN_IMG").is_some();
    let golden = golden();
    for (note, (name, img, _, _)) in reference_notes().iter().zip(&golden) {
        // W CI tylko lzejsze notatki - pelny zestaw na maszynie odniesienia.
        if !check_golden && note.size.0 > 1440 {
            continue;
        }
        let mut a = Scene::warp(note.size.0, note.size.1).unwrap();
        let pa = render(note, &mut a).unwrap();
        let mut b = Scene::warp(note.size.0, note.size.1).unwrap();
        let pb = render(note, &mut b).unwrap();
        assert_eq!(
            png::diff_pixels(&pa, &pb),
            0,
            "{name}: obraz niepowtarzalny"
        );
        if check_golden {
            assert_eq!(png::hash(&pa), *img, "{name}: obraz inny niz w v0.4.5");
        }
    }
}

/// T5.3 w uprzezy: bez zdarzen scena nie oddaje klatek.
#[test]
fn spoczynek_bez_klatek() {
    use spectre_harness::pen::Recording;
    let mut s = Scene::warp(640, 480).unwrap();
    s.replay(
        &Recording::handwriting(1, (50.0, 100.0), 1, 1, 30.0),
        1,
        false,
    )
    .unwrap();
    let f0 = s.renderer.presents();
    assert!(f0 > 0);
    // Petla zdarzen bez zdarzen: nic brudnego, wiec nic nie rysuje.
    for _ in 0..100 {
        if s.dirty != spectre_harness::scene::Dirty::Clean {
            s.frame().unwrap();
        }
    }
    assert_eq!(s.renderer.presents(), f0);
}
