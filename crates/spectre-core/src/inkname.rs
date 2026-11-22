//! Nazwa pisana piorem (W19): tytul notatki, nazwa plaszczyzny i kazda inna
//! nazwa w aplikacji to mala kanwa z kreskami, nie tekst.
//!
//! # Uklad wspolrzednych
//! Kreski nazwy leza w **jednostkach nazwy**: pole ma wysokosc [`NAME_H`],
//! `y` rosnie w dol od gornej krawedzi pola, `x` od lewej krawedzi pola
//! pisania. Pismo zostaje tam, gdzie je napisano (geometria zachowana: po
//! ponownym otwarciu pola lezy w tym samym miejscu). Wyswietlanie dopasowuje
//! sie do obwiedni pisma (`InkName::fit`), wiec nazwa wyglada tak samo
//! w zakladce tytulu, na kafelku listy i w wierszu listy plaszczyzn.
//!
//! # Zapis (zgodnosc wstecz, O12)
//! Nazwa to zwykla metadana `Meta { key, value }` - operacja, ktora zna kazda
//! wersja od v0.1. Klucz to klucz nazwy z dopiskiem [`INK_SUFFIX`] (tytul:
//! `title.ink`), wartosc to tekst ASCII: [`PREFIX`] i base64url bajtow kresek
//! w tym samym kodowaniu co `StrokeAdd` (`spectre_proto::codec`). Starsza
//! wersja wczytuje taka metadana jak kazda inna, nie rozumie jej i zostawia
//! w op-logu nietknieta; tekstowy `title` dziala w niej dalej. Pusta wartosc
//! = nazwa wyczyszczona (LWW jak dla kazdej metadanej).

use spectre_proto::codec::{decode_stroke, encode_stroke};
use spectre_proto::varint::{put_u64, Reader};
use spectre_proto::StrokeData;

use crate::document::Bbox;

/// Wysokosc pola nazwy w jednostkach nazwy.
pub const NAME_H: f32 = 40.0;
/// Szerokosc pola nazwy w jednostkach nazwy - najdluzsza nazwa, jaka da sie
/// napisac. Pole pisania ma te proporcje (`NAME_W` : `NAME_H`), a zakladka
/// tytulu jest jego miniatura: kreska od brzegu do brzegu pola wypelnia ja
/// cala (poprawka po tescie 2026-11-22).
pub const NAME_W: f32 = 190.0;
/// Dopisek klucza metadanej z pismem: `title` -> `title.ink`.
pub const INK_SUFFIX: &str = ".ink";
/// Klucz metadanej z odrecznym tytulem notatki.
pub const TITLE_INK_KEY: &str = "title.ink";
/// Poczatek wartosci; podbijany przy zmianie kodowania. Nieznany poczatek =
/// nazwa nowszej wersji: pokazywana jak brak pisma, ale nietknieta w logu.
pub const PREFIX: &str = "ink1:";
/// Najmniejsza wysokosc pisma przy dopasowaniu, jako ulamek pola nazwy.
pub const MIN_FILL: f32 = 0.4;

/// Jak nazwa wchodzi w prostokat (`InkName::fit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Pismo na wysokosc prostokata, do lewej (listy).
    Left,
    /// Pismo na wysokosc prostokata, wysrodkowane (zakladka tytulu).
    Center,
    /// Cale pole nazwy 1:1 (pole pisania).
    Box,
    /// Miniatura pola pisania (`NAME_W` x `NAME_H`) wpisana w prostokat
    /// i wysrodkowana: pismo lezy tam, gdzie je napisano, a pelne pole
    /// wypelnia prostokat o tych proporcjach (zakladka tytulu).
    Miniature,
}

/// Gorna granica liczby kresek - chroni przed alokacja na uszkodzonej wartosci.
const MAX_STROKES: usize = 4096;

/// Nazwa pisana piorem. Niezmienna po zbudowaniu: edycja to nowa nazwa.
#[derive(Debug, Clone, Default)]
pub struct InkName {
    strokes: Vec<StrokeData>,
    /// Odcisk zapisu, liczony raz przy budowie (patrz `fingerprint`).
    fp: u64,
}

/// Rownosc = te same kreski (odcisk z nich wynika).
impl PartialEq for InkName {
    fn eq(&self, other: &Self) -> bool {
        self.strokes == other.strokes
    }
}

impl InkName {
    /// Nazwa z kresek w jednostkach nazwy. Kreski zostaja tam, gdzie je
    /// napisano (bez przesuwania - pole pisania otwiera sie z pismem w tym
    /// samym miejscu), i sa kwantyzowane tak jak przy zapisie, wiec nazwa
    /// w pamieci jest **identyczna** z ta, ktora wroci z dysku (T7).
    pub fn from_strokes(strokes: Vec<StrokeData>) -> Self {
        let strokes: Vec<StrokeData> = strokes
            .into_iter()
            .filter(|s| !s.samples.is_empty())
            .collect();
        let t0 = strokes
            .iter()
            .flat_map(|s| s.samples.iter().map(|p| p.t_us))
            .min()
            .unwrap_or(0);
        let shifted: Vec<StrokeData> = strokes
            .into_iter()
            .map(|mut s| {
                for p in &mut s.samples {
                    // Czas wzgledny: liczby male, a nazwa niezalezna od chwili pisania.
                    p.t_us -= t0;
                }
                s
            })
            .collect();
        let raw = Self::with(shifted);
        // Kwantyzacja: ta sama droga co zapis i odczyt.
        Self::decode(&raw.encode()).unwrap_or_default()
    }

    /// Kreski bez normalizacji - podglad w trakcie pisania (pole pisania
    /// pokazuje pismo dokladnie tam, gdzie bylo pioro). Do zapisu sluzy
    /// [`InkName::from_strokes`].
    pub fn from_raw(strokes: Vec<StrokeData>) -> Self {
        Self::with(strokes)
    }

    fn with(strokes: Vec<StrokeData>) -> Self {
        let mut n = Self { strokes, fp: 0 };
        n.fp = fnv(n.encode().as_bytes());
        n
    }

    pub fn strokes(&self) -> &[StrokeData] {
        &self.strokes
    }

    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty()
    }

    /// Obwiednia pisma (z grubosciami kresek); `None` dla pustej nazwy.
    pub fn bbox(&self) -> Option<Bbox> {
        self.strokes.iter().map(Bbox::of).reduce(|a, b| Bbox {
            min_x: a.min_x.min(b.min_x),
            min_y: a.min_y.min(b.min_y),
            max_x: a.max_x.max(b.max_x),
            max_y: a.max_y.max(b.max_y),
        })
    }

    /// Szerokosc pisma w jednostkach nazwy (obwiednia, z grubosciami kresek).
    pub fn width(&self) -> f32 {
        self.bbox().map_or(0.0, |b| b.max_x - b.min_x)
    }

    /// Dopasowanie do prostokata `w` x `h` (piksele): skala i przesuniecie
    /// poczatku ukladu nazwy wzgledem lewego gornego rogu prostokata.
    ///
    /// `Left` i `Center`: pismo wypelnia wysokosc prostokata wedlug **swojej**
    /// wysokosci (pismo zwykle zajmuje polowe pola - liczone od pola byloby
    /// w zakladce dwa razy mniejsze), ale nie mniejszej niz [`MIN_FILL`] pola,
    /// zeby pojedyncza kreska pozioma nie urosla w gruby pasek. Za szerokie
    /// pismo zweza sie do szerokosci. `Box`: cale pole nazwy 1:1 w wysokosci
    /// prostokata, bez zwezania - pole pisania, gdzie kreska ma lezec dokladnie
    /// pod piorem.
    pub fn fit(&self, w: f32, h: f32, mode: Fit) -> (f32, f32, f32) {
        if mode == Fit::Box {
            return ((h / NAME_H).max(0.0), 0.0, 0.0);
        }
        if mode == Fit::Miniature {
            let k = (h / NAME_H).min(w / NAME_W).max(0.0);
            return (k, (w - NAME_W * k) * 0.5, (h - NAME_H * k) * 0.5);
        }
        let b = self.bbox().unwrap_or(Bbox {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 0.0,
            max_y: NAME_H,
        });
        let ink_h = (b.max_y - b.min_y).max(NAME_H * MIN_FILL);
        let mid = (b.min_y + b.max_y) * 0.5;
        let name_w = (b.max_x - b.min_x).max(1.0);
        let mut k = (h / ink_h).max(0.0);
        if name_w * k > w {
            k = (w / name_w).max(0.0);
        }
        // Obwiednia pisma (nie lewa krawedz pola) na lewej krawedzi albo w srodku.
        let dx = if mode == Fit::Center {
            (w - name_w * k) * 0.5
        } else {
            0.0
        } - b.min_x * k;
        let dy = h * 0.5 - mid * k;
        (k, dx, dy)
    }

    /// Kreska nazwy do narysowania w skali `k` (piksele ekranu na jednostke
    /// nazwy). `base_width` kreski nazwy to grubosc piora w **pikselach ekranu**
    /// (tak jak kreska kanwy przy 100%), nie w jednostkach nazwy: nazwa ma
    /// te sama grubosc linii w polu pisania, zakladce, menu i liscie
    /// plaszczyzn, jak ustawione pioro. Geometria budowana w jednostkach nazwy
    /// dostaje wiec grubosc podzielona przez skale.
    pub fn screen_stroke(data: &StrokeData, k: f32) -> StrokeData {
        StrokeData {
            base_width: data.base_width / k.max(1e-6),
            ..data.clone()
        }
    }

    /// Wartosc metadanej: `ink1:` + base64url bez dopelnienia. Pusta nazwa =
    /// pusty tekst (nazwa wyczyszczona).
    pub fn encode(&self) -> String {
        if self.strokes.is_empty() {
            return String::new();
        }
        let mut bytes = Vec::new();
        put_u64(&mut bytes, self.strokes.len() as u64);
        for s in &self.strokes {
            encode_stroke(s, &mut bytes);
        }
        let mut out = String::with_capacity(PREFIX.len() + bytes.len() * 4 / 3 + 4);
        out.push_str(PREFIX);
        b64_encode(&bytes, &mut out);
        out
    }

    /// Odczyt wartosci metadanej. Pusty tekst = pusta nazwa; nieznany poczatek
    /// albo uszkodzone bajty = `None` (pokazuje sie zastepczy napis, a wartosc
    /// zostaje w logu).
    pub fn decode(value: &str) -> Option<Self> {
        if value.is_empty() {
            return Some(Self::default());
        }
        let body = value.strip_prefix(PREFIX)?;
        let bytes = b64_decode(body)?;
        let mut r = Reader::new(&bytes);
        let n = r.usize().ok()?;
        if n > MAX_STROKES || n > r.remaining() {
            return None;
        }
        let mut strokes = Vec::with_capacity(n);
        for _ in 0..n {
            strokes.push(decode_stroke(&mut r).ok()?);
        }
        if !r.is_empty() {
            return None;
        }
        Some(Self::with(strokes))
    }

    /// Odcisk do kluczy pamieci podrecznej (geometria w rendererze).
    /// FNV-1a wartosci zapisu - dwie identyczne nazwy maja ten sam odcisk.
    pub fn fingerprint(&self) -> u64 {
        self.fp
    }
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Klucz metadanej z pismem dla klucza nazwy (`title` -> `title.ink`,
/// `plane.3.name` -> `plane.3.name.ink`).
pub fn ink_key(name_key: &str) -> String {
    format!("{name_key}{INK_SUFFIX}")
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn b64_encode(bytes: &[u8], out: &mut String) {
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let v = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        let n = chunk.len() + 1;
        for i in 0..n {
            out.push(B64[((v >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
}

fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        } as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    // Reszta krotsza niz bajt to dopelnienie zerami z `b64_encode`.
    if bits >= 6 || acc != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;
    use spectre_proto::{AuthorId, Rgba, Sample};

    fn stroke(x0: f32, y0: f32, n: usize) -> StrokeData {
        StrokeData {
            tool: 0,
            color: Rgba::rgb(216, 216, 216),
            base_width: 2.3,
            samples: (0..n)
                .map(|i| Sample {
                    x: x0 + i as f32 * 0.731,
                    y: y0 + (i as f32 * 0.37).sin() * 6.1,
                    pressure: 0.3 + (i as f32 * 0.11).cos().abs() * 0.6,
                    tilt_x: 12.4,
                    tilt_y: -7.6,
                    t_us: 9_000_000 + i as u64 * 3761,
                })
                .collect(),
        }
    }

    fn sample_name() -> InkName {
        InkName::from_strokes(vec![
            stroke(13.3, 20.0, 40),
            stroke(50.1, 18.2, 25),
            stroke(80.0, 30.0, 1),
        ])
    }

    #[test]
    fn base64_w_obie_strony() {
        for n in 0..40usize {
            let bytes: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            let mut s = String::new();
            b64_encode(&bytes, &mut s);
            assert_eq!(b64_decode(&s).unwrap(), bytes, "n = {n}");
        }
        assert!(b64_decode("a").is_none());
        assert!(b64_decode("ab=c").is_none());
    }

    #[test]
    fn zapis_i_odczyt_daje_identyczna_nazwe() {
        let name = sample_name();
        let back = InkName::decode(&name.encode()).unwrap();
        assert_eq!(back, name);
        // I drugi raz bez zmian - nie ma dryfu kwantyzacji.
        assert_eq!(
            InkName::decode(&back.encode()).unwrap().encode(),
            name.encode()
        );
    }

    /// Poprawka po tescie (2026-11-22): pismo w srodku pola po zapisie
    /// i odczycie lezy w tym samym miejscu - bez wyrownania do lewej.
    #[test]
    fn geometria_zachowana_po_zapisie() {
        let raw = vec![stroke(180.3, 20.0, 40), stroke(230.1, 18.2, 25)];
        let name = InkName::from_strokes(raw.clone());
        let back = InkName::decode(&name.encode()).unwrap();
        for (a, b) in raw.iter().zip(back.strokes()) {
            for (p, q) in a.samples.iter().zip(&b.samples) {
                assert!((p.x - q.x).abs() <= 1.0 / 64.0, "x {} -> {}", p.x, q.x);
                assert!((p.y - q.y).abs() <= 1.0 / 64.0, "y {} -> {}", p.y, q.y);
            }
        }
        assert!(back.bbox().unwrap().min_x > 170.0);
        assert_eq!(name.strokes()[0].samples[0].t_us, 0);
    }

    #[test]
    fn pusta_i_uszkodzona_wartosc() {
        assert!(InkName::decode("").unwrap().is_empty());
        assert_eq!(InkName::default().encode(), "");
        assert!(InkName::decode("ink9:AAAA").is_none());
        assert!(InkName::decode("ink1:###").is_none());
        let mut enc = sample_name().encode();
        enc.truncate(enc.len() - 5);
        assert!(InkName::decode(&enc).is_none());
        // Ogromna liczba kresek w naglowku nie alokuje.
        let mut s = String::from(PREFIX);
        let mut b = Vec::new();
        put_u64(&mut b, u64::MAX >> 1);
        b64_encode(&b, &mut s);
        assert!(InkName::decode(&s).is_none());
    }

    #[test]
    fn dopasowanie_do_pola() {
        let name = sample_name();
        let wn = name.width();
        let b = name.bbox().unwrap();
        let ink_h = b.max_y - b.min_y;
        assert!(ink_h > NAME_H * MIN_FILL);
        // Pole pisania: cale pole nazwy 1:1, bez przesuniec i bez zwezania.
        assert_eq!(name.fit(10.0, 80.0, Fit::Box), (2.0, 0.0, 0.0));
        // Szerokie pole: wysokosc pisma = wysokosc prostokata, pismo w srodku.
        let (k, dx, dy) = name.fit(10_000.0, 80.0, Fit::Left);
        assert!((k - 80.0 / ink_h).abs() < 1e-4);
        assert!(
            (dx + b.min_x * k).abs() < 1e-3,
            "lewa krawedz pisma na lewej krawedzi pola"
        );
        assert!((dy + b.min_y * k).abs() < 1e-3, "gora pisma na gorze pola");
        // Waskie pole: zwezone do szerokosci, dalej wysrodkowane w pionie.
        let (k, dx, dy) = name.fit(wn, 80.0, Fit::Center);
        assert!((k - 1.0).abs() < 1e-4);
        assert!((dx + b.min_x).abs() < 1e-3);
        assert!((dy - (40.0 - (b.min_y + b.max_y) * 0.5)).abs() < 1e-3);
        // Wysrodkowanie w poziomie.
        let (k, dx, _) = name.fit(10_000.0, 40.0, Fit::Center);
        assert!((dx + b.min_x * k - (10_000.0 - wn * k) * 0.5).abs() < 1e-2);
        // Pojedyncza kreska pozioma nie rosnie ponad MIN_FILL pola.
        let flat = InkName::from_strokes(vec![StrokeData {
            samples: (0..10)
                .map(|i| Sample {
                    x: i as f32 * 3.0,
                    y: 20.0,
                    pressure: 0.5,
                    ..Default::default()
                })
                .collect(),
            ..stroke(0.0, 0.0, 1)
        }]);
        let (k, _, _) = flat.fit(10_000.0, 40.0, Fit::Left);
        assert!((k - 1.0 / MIN_FILL).abs() < 1e-4);
    }

    /// Grubosc piora w pikselach ekranu: w kazdej skali po przeliczeniu linia
    /// ma te sama grubosc na ekranie, a zapis jej nie zmienia.
    #[test]
    fn grubosc_piora_niezalezna_od_skali() {
        let mut s = stroke(10.0, 20.0, 12);
        s.base_width = 1.4;
        let name = InkName::from_strokes(vec![s]);
        let back = InkName::decode(&name.encode()).unwrap();
        assert_eq!(back.strokes()[0].base_width, 1.4);
        for k in [0.25f32, 0.65, 1.0, 3.25] {
            let d = InkName::screen_stroke(&back.strokes()[0], k);
            assert!((d.base_width * k - 1.4).abs() < 1e-5, "k = {k}");
            assert_eq!(d.samples, back.strokes()[0].samples);
        }
    }

    #[test]
    fn odcisk_rozroznia_nazwy() {
        let a = sample_name();
        let b = InkName::from_strokes(vec![stroke(1.0, 1.0, 10)]);
        assert_eq!(a.fingerprint(), a.clone().fingerprint());
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    /// Metadana z pismem przechodzi przez `Document` jak kazda inna (LWW),
    /// takze dla nazwy plaszczyzny.
    #[test]
    fn nazwa_w_dokumencie_lww() {
        let mut d = Document::new(AuthorId(7));
        let a = sample_name();
        let b = InkName::from_strokes(vec![stroke(3.0, 5.0, 12)]);
        let op1 = d.set_meta(&ink_key("plane.2.name"), &a.encode());
        let op2 = d.set_meta(&ink_key("plane.2.name"), &b.encode());
        let mut other = Document::new(AuthorId(8));
        other.apply(&op2);
        other.apply(&op1);
        let got = InkName::decode(other.meta("plane.2.name.ink").unwrap()).unwrap();
        assert_eq!(got, b);
    }
}
