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
