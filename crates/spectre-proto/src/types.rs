//! Typy modelu danych. Wszystko, co tu jest, ma reprezentacje bajtowa w `codec`.

use std::fmt;

/// Autor = para (uzytkownik, urzadzenie), np. `adi@spectre-x360`.
///
/// Przechowywany jako 64-bitowy skrot nazwy - w kazdej operacji i w kazdym
/// identyfikatorze kreski, wiec musi byc maly. Pelna nazwa zyje w nazwie
/// katalogu `ops/<nazwa>/`. Kolizja skrotu miedzy kilkoma autorami jednego
/// space'u jest zaniedbywalna (2^-64 na pare).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct AuthorId(pub u64);

impl AuthorId {
    /// FNV-1a - deterministyczny, bez zaleznosci, wystarczajacy dla kilku autorow.
    pub fn from_name(name: &str) -> Self {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in name.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        AuthorId(h)
    }
}

impl fmt::Debug for AuthorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Author({:016x})", self.0)
    }
}

/// Identyfikator kreski: autor + jego lokalny licznik. Unikalny bez uzgadniania.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StrokeId {
    pub author: AuthorId,
    pub seq: u64,
}

/// Identyfikator plaszczyzny (tryb 3D): jak `StrokeId`, z tego samego licznika
/// autora. `PlaneId::ZERO` to plaszczyzna zerowa, czyli zwykla notatka 2D -
/// jej kreski sa zwyklymi `StrokeAdd` i nie wymagaja zadnej operacji.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlaneId {
    pub author: AuthorId,
    pub seq: u64,
}

impl PlaneId {
    pub const ZERO: PlaneId = PlaneId {
        author: AuthorId(0),
        seq: 0,
    };

    pub fn is_zero(&self) -> bool {
        *self == Self::ZERO
    }
}

/// Ustawienie plaszczyzny w przestrzeni: obrot wokol X, potem wokol Y
/// (stopnie, wartosci bezwzgledne z kompasu) i srodek. Jednostki jak na
/// canvasie (1 j. = 1 px przy 100%). Matematyka: `spectre_core::space`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PlanePose {
    pub rx: f64,
    pub ry: f64,
    pub origin: [f64; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
}

/// Pojedyncza probka piora, dokladnie tak jak zaraportowal ja digitizer.
///
/// `x`/`y` w pikselach canvasu (subpikselowo), `pressure` 0..1,
/// tilt w stopniach, `t_us` w mikrosekundach z zegara wysokiej rozdzielczosci.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Sample {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub t_us: u64,
}

/// Kreska w calosci. Niezmienna po utworzeniu - edycja to nowa kreska.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StrokeData {
    /// Zarezerwowane na zakreslacz itp. W v1 zawsze 0 = pioro.
    pub tool: u8,
    pub color: Rgba,
    pub base_width: f32,
    pub samples: Vec<Sample>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpKind {
    StrokeAdd {
        id: StrokeId,
        data: StrokeData,
    },
    /// Nagrobek. Nieodwracalny - cofniecie wymazania to `StrokeAdd` z nowym id.
    StrokeErase {
        id: StrokeId,
    },
    /// Metadane notatki (tytul itp.), LWW po (lamport, author).
    Meta {
        key: String,
        value: String,
    },
    /// Nowa plaszczyzna (tryb 3D). Niezmienna - inne ustawienie to inna
    /// plaszczyzna. Powstaje sama, przy pierwszej kresce na niej (W9).
    PlaneAdd {
        id: PlaneId,
        pose: PlanePose,
    },
    /// Kreska na plaszczyznie innej niz zerowa. Probki w ukladzie plaszczyzny
    /// (u, v od jej srodka). Wersje sprzed 3D jej nie rysuja (O12).
    StrokeOn {
        id: StrokeId,
        plane: PlaneId,
        data: StrokeData,
    },
    /// Wlasciwosc plaszczyzny (widocznosc, nazwa), LWW po (lamport, author).
    PlaneProp {
        id: PlaneId,
        key: String,
        value: String,
    },
    /// Operacja z nowszej wersji formatu, ktorej ta wersja nie zna (O12).
    ///
    /// Czytnik jej nie odrzuca i nie ucina na niej pliku: trzyma jej tag
    /// i bajty tresci bez zmian, `encode_op` oddaje je bajt w bajt, a
    /// `Document::apply` ja pomija (rysunek jej nie widzi). Dzieki temu
    /// notatka zapisana nowsza wersja otwiera sie w starszej z tym, co ta
    /// rozumie, i nie traci reszty - ani na dysku, ani w synchronizacji.
    Unknown {
        tag: u8,
        body: Vec<u8>,
    },
}

/// Operacja w logu. Para `(lamport, author)` jest unikalna i wyznacza
/// deterministyczna kolejnosc u wszystkich peerow.
#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    pub author: AuthorId,
    pub lamport: u64,
    pub kind: OpKind,
}

impl Op {
    /// Klucz porzadkujacy: najpierw lamport, potem autor.
    pub fn key(&self) -> (u64, AuthorId) {
        (self.lamport, self.author)
    }
}
