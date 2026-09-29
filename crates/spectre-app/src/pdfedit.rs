//! Dopisywanie do cudzego PDF-a: **aktualizacja przyrostowa** (PDF 7.5.6).
//! Oryginalne bajty zostaja nietkniete, na koncu pliku dochodza nowe wersje
//! zmienionych obiektow i nowa tablica xref z `/Prev` na stara. Tak dopisuja
//! adnotacje przegladarki PDF - tekst, linki, formularze i zakladki
//! oryginalu zostaja, bo nikt ich nie przepisuje.
//!
//! Czytamy tylko tyle, ile trzeba, zeby znalezc strony: xref (tablica albo
//! strumien, z predyktorem PNG), strumienie obiektow, drzewo stron
//! z dziedziczonymi `MediaBox`/`CropBox`/`Rotate`. Nowa tresc strony to
//! `[q, oryginal..., Q + nasza warstwa]` - oryginal nie moze nam zostawic
//! zmienionego ukladu wspolrzednych.

use std::collections::{HashMap, HashSet};

use crate::deflate;

#[derive(Debug, Clone, PartialEq)]
pub enum Obj {
    /// Liczba, napis, `true`/`false`/`null` - dokladnie tak, jak w pliku
    /// (zapisujemy z powrotem bajt w bajt, bez ponownego kodowania napisow).
    Raw(Vec<u8>),
    /// Nazwa bez `/`, w postaci z pliku (z ewentualnymi `#xx`).
    Name(Vec<u8>),
    Array(Vec<Obj>),
    Dict(Dict),
    Ref(u32, u16),
}

pub type Dict = Vec<(Vec<u8>, Obj)>;

pub fn dict_get<'a>(d: &'a Dict, key: &str) -> Option<&'a Obj> {
    d.iter().find(|(k, _)| k == key.as_bytes()).map(|(_, v)| v)
}

pub fn dict_set(d: &mut Dict, key: &str, v: Obj) {
    match d.iter_mut().find(|(k, _)| k == key.as_bytes()) {
        Some(slot) => slot.1 = v,
        None => d.push((key.as_bytes().to_vec(), v)),
    }
}

impl Obj {
    pub fn num(&self) -> Option<f64> {
        match self {
            Obj::Raw(r) => std::str::from_utf8(r).ok()?.parse().ok(),
            _ => None,
        }
    }
    pub fn name(&self) -> Option<&[u8]> {
        match self {
            Obj::Name(n) => Some(n),
            _ => None,
        }
    }
    pub fn int(v: i64) -> Obj {
        Obj::Raw(v.to_string().into_bytes())
    }
    pub fn real(v: f64) -> Obj {
        Obj::Raw(fmt_num(v).into_bytes())
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        match self {
            Obj::Raw(r) => out.extend_from_slice(r),
            Obj::Name(n) => {
                out.push(b'/');
                out.extend_from_slice(n);
            }
            Obj::Ref(n, g) => out.extend_from_slice(format!("{n} {g} R").as_bytes()),
            Obj::Array(a) => {
                out.push(b'[');
                for (i, o) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(b' ');
                    }
                    o.write(out);
                }
                out.push(b']');
            }
            Obj::Dict(d) => {
                out.extend_from_slice(b"<<");
                for (k, v) in d {
                    out.push(b'/');
                    out.extend_from_slice(k);
                    out.push(b' ');
                    v.write(out);
                }
                out.extend_from_slice(b">>");
            }
        }
    }
}

pub fn fmt_num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".into()
    } else {
        s.into()
    }
}

#[derive(Debug, PartialEq)]
pub enum Error {
    Encrypted,
    Broken(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Encrypted => write!(f, "the PDF is encrypted"),
            Error::Broken(w) => write!(f, "unsupported PDF structure ({w})"),
        }
    }
}

type R<T> = Result<T, Error>;
/// Rozpakowany strumien obiektow: dane i (numer, offset) kazdego obiektu.
type ObjStm = (Vec<u8>, Vec<(u32, usize)>);

// ----- lekser / parser ------------------------------------------------------

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\n' | b'\r' | b'\t' | b'\x0c' | b'\0')
}

fn is_delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

struct Lexer<'a> {
    d: &'a [u8],
    pos: usize,
}

#[derive(Debug, PartialEq)]
enum Tok<'a> {
    DictOpen,
    DictClose,
    ArrOpen,
    ArrClose,
    Name(&'a [u8]),
    /// Liczba, napis, slowo kluczowe - surowe bajty.
    Word(&'a [u8]),
    Str(&'a [u8]),
}

impl<'a> Lexer<'a> {
    fn skip_ws(&mut self) {
        while let Some(&c) = self.d.get(self.pos) {
            if is_ws(c) {
                self.pos += 1;
            } else if c == b'%' {
                while let Some(&c) = self.d.get(self.pos) {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn next(&mut self) -> Option<Tok<'a>> {
        self.skip_ws();
        let start = self.pos;
        let c = *self.d.get(self.pos)?;
        match c {
            b'<' if self.d.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2;
                Some(Tok::DictOpen)
            }
            b'>' if self.d.get(self.pos + 1) == Some(&b'>') => {
                self.pos += 2;
                Some(Tok::DictClose)
            }
            b'[' => {
                self.pos += 1;
                Some(Tok::ArrOpen)
            }
            b']' => {
                self.pos += 1;
                Some(Tok::ArrClose)
            }
            b'/' => {
                self.pos += 1;
                while let Some(&c) = self.d.get(self.pos) {
                    if is_ws(c) || is_delim(c) {
                        break;
                    }
                    self.pos += 1;
                }
                Some(Tok::Name(&self.d[start + 1..self.pos]))
            }
            b'(' => {
                let mut depth = 0usize;
                while let Some(&c) = self.d.get(self.pos) {
                    self.pos += 1;
                    match c {
                        b'\\' => self.pos += 1,
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                return Some(Tok::Str(&self.d[start..self.pos]));
                            }
                        }
                        _ => {}
                    }
                }
                None
            }
            b'<' => {
                while let Some(&c) = self.d.get(self.pos) {
                    self.pos += 1;
                    if c == b'>' {
                        return Some(Tok::Str(&self.d[start..self.pos]));
                    }
                }
                None
            }
            _ => {
                while let Some(&c) = self.d.get(self.pos) {
                    if is_ws(c) || is_delim(c) {
                        break;
                    }
                    self.pos += 1;
                }
                if self.pos == start {
                    // Samotny `)`, `>`, `{` - smiec; pomijamy bajt.
                    self.pos += 1;
                }
                Some(Tok::Word(&self.d[start..self.pos]))
            }
        }
    }

    fn peek(&mut self) -> Option<Tok<'a>> {
        let p = self.pos;
        let t = self.next();
        self.pos = p;
        t
    }

    fn is_uint(w: &[u8]) -> bool {
        !w.is_empty() && w.iter().all(u8::is_ascii_digit)
    }

    /// Obiekt od biezacej pozycji; `n g R` rozpoznajemy, podgladajac dwa tokeny.
    fn object(&mut self, depth: usize) -> Option<Obj> {
        if depth > 64 {
            return None;
        }
        match self.next()? {
            Tok::DictOpen => {
                let mut d = Dict::new();
                loop {
                    match self.next()? {
                        Tok::DictClose => return Some(Obj::Dict(d)),
                        Tok::Name(k) => {
                            let v = self.object(depth + 1)?;
                            d.push((k.to_vec(), v));
                        }
                        _ => return None,
                    }
                }
            }
            Tok::ArrOpen => {
                let mut a = Vec::new();
                loop {
                    if self.peek()? == Tok::ArrClose {
                        self.next();
                        return Some(Obj::Array(a));
                    }
                    a.push(self.object(depth + 1)?);
                }
            }
            Tok::Name(n) => Some(Obj::Name(n.to_vec())),
            Tok::Str(s) => Some(Obj::Raw(s.to_vec())),
            Tok::Word(w) => {
                if Self::is_uint(w) {
                    let save = self.pos;
                    if let Some(Tok::Word(g)) = self.next() {
                        if Self::is_uint(g) {
                            if let Some(Tok::Word(b"R")) = self.next() {
                                let n = std::str::from_utf8(w).ok()?.parse().ok()?;
                                let g = std::str::from_utf8(g).ok()?.parse().ok()?;
                                return Some(Obj::Ref(n, g));
                            }
                        }
                    }
                    self.pos = save;
                }
                Some(Obj::Raw(w.to_vec()))
            }
            Tok::DictClose | Tok::ArrClose => None,
        }
    }
}

#[cfg(test)]
pub fn parse(d: &[u8]) -> Option<Obj> {
    Lexer { d, pos: 0 }.object(0)
}

// ----- dokument -------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Loc {
    Offset(usize),
    InStream(u32, u32),
}

pub struct Doc<'a> {
    pub data: &'a [u8],
    xref: HashMap<u32, (Loc, u16)>,
    pub trailer: Dict,
    /// Offset ostatniej sekcji xref (`/Prev` aktualizacji).
    pub startxref: usize,
    /// Ostatnia sekcja xref jest strumieniem - aktualizacja tez musi byc.
    pub xref_is_stream: bool,
    pub size: u32,
    objstm_cache: std::cell::RefCell<HashMap<u32, ObjStm>>,
}

/// Strona oryginalu: numer obiektu, slownik i to, co dziedziczy.
#[derive(Debug, Clone)]
pub struct Page {
    pub num: u32,
    pub gen: u16,
    pub dict: Dict,
    pub media: [f64; 4],
    pub crop: Option<[f64; 4]>,
    pub rotate: i32,
}

impl Page {
    /// Pole widoczne (to, co renderuje przegladarka i nasz ekran).
    pub fn visible_box(&self) -> [f64; 4] {
        let b = self.crop.unwrap_or(self.media);
        // Przyciecie do MediaBox (tak liczy Windows.Data.Pdf).
        [
            b[0].max(self.media[0]),
            b[1].max(self.media[1]),
            b[2].min(self.media[2]),
            b[3].min(self.media[3]),
        ]
    }
}

fn find_last(d: &[u8], pat: &[u8]) -> Option<usize> {
    d.windows(pat.len()).rposition(|w| w == pat)
}

fn norm_box(o: &Obj) -> Option<[f64; 4]> {
    let Obj::Array(a) = o else { return None };
    if a.len() != 4 {
        return None;
    }
    let v: Vec<f64> = a.iter().map(|x| x.num()).collect::<Option<_>>()?;
    Some([
        v[0].min(v[2]),
        v[1].min(v[3]),
        v[0].max(v[2]),
        v[1].max(v[3]),
    ])
}

impl<'a> Doc<'a> {
    pub fn open(data: &'a [u8]) -> R<Self> {
        let tail_from = data.len().saturating_sub(2048);
        let sx = find_last(&data[tail_from..], b"startxref").ok_or(Error::Broken("startxref"))?
            + tail_from;
        let mut lx = Lexer {
            d: data,
            pos: sx + 9,
        };
        let startxref = match lx.next() {
            Some(Tok::Word(w)) => std::str::from_utf8(w)
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or(Error::Broken("startxref"))?,
            _ => return Err(Error::Broken("startxref")),
        };
        let mut doc = Doc {
            data,
            xref: HashMap::new(),
            trailer: Dict::new(),
            startxref,
            xref_is_stream: false,
            size: 0,
            objstm_cache: Default::default(),
        };
        let mut next = Some(startxref);
        let mut seen = HashSet::new();
        let mut first = true;
        while let Some(off) = next.take() {
            if !seen.insert(off) || seen.len() > 256 {
                break;
            }
            let (trailer, is_stream) = doc.read_section(off)?;
            if first {
                doc.trailer = trailer.clone();
                doc.xref_is_stream = is_stream;
                first = false;
            }
            // Plik hybrydowy: tablica + strumien z obiektami skompresowanymi.
            if let Some(x) = dict_get(&trailer, "XRefStm").and_then(Obj::num) {
                let _ = doc.read_section(x as usize);
            }
            next = dict_get(&trailer, "Prev")
                .and_then(Obj::num)
                .map(|v| v as usize);
        }
        if dict_get(&doc.trailer, "Encrypt").is_some() {
            return Err(Error::Encrypted);
        }
        doc.size = dict_get(&doc.trailer, "Size")
            .and_then(Obj::num)
            .ok_or(Error::Broken("trailer /Size"))? as u32;
        let max_obj = doc.xref.keys().copied().max().unwrap_or(0);
        doc.size = doc.size.max(max_obj + 1);
        Ok(doc)
    }

    /// Sekcja xref pod `off`; wpisy nowsze (czytane wczesniej) wygrywaja.
    fn read_section(&mut self, off: usize) -> R<(Dict, bool)> {
        let d = self.data;
        let mut lx = Lexer { d, pos: off };
        lx.skip_ws();
        if d.get(lx.pos..lx.pos + 4) == Some(b"xref") {
            lx.pos += 4;
            loop {
                match lx.next() {
                    Some(Tok::Word(b"trailer")) => break,
                    Some(Tok::Word(s)) => {
                        let start: u32 = std::str::from_utf8(s)
                            .ok()
                            .and_then(|v| v.parse().ok())
                            .ok_or(Error::Broken("xref"))?;
                        let count: u32 = match lx.next() {
                            Some(Tok::Word(c)) => std::str::from_utf8(c)
                                .ok()
                                .and_then(|v| v.parse().ok())
                                .ok_or(Error::Broken("xref"))?,
                            _ => return Err(Error::Broken("xref")),
                        };
                        for i in 0..count {
                            let (o, g, t) = (lx.next(), lx.next(), lx.next());
                            let (Some(Tok::Word(o)), Some(Tok::Word(g)), Some(Tok::Word(t))) =
                                (o, g, t)
                            else {
                                return Err(Error::Broken("xref entry"));
                            };
                            if t == b"n" {
                                let o: usize = std::str::from_utf8(o)
                                    .ok()
                                    .and_then(|v| v.parse().ok())
                                    .unwrap_or(0);
                                let g: u16 = std::str::from_utf8(g)
                                    .ok()
                                    .and_then(|v| v.parse().ok())
                                    .unwrap_or(0);
                                self.xref.entry(start + i).or_insert((Loc::Offset(o), g));
                            }
                        }
                    }
                    _ => return Err(Error::Broken("xref")),
                }
            }
            match lx.object(0) {
                Some(Obj::Dict(t)) => Ok((t, false)),
                _ => Err(Error::Broken("trailer")),
            }
        } else {
            let (dict, body) = self.read_indirect_at(off)?;
            let body = body.ok_or(Error::Broken("xref stream"))?;
            if dict_get(&dict, "Type").and_then(Obj::name) != Some(b"XRef") {
                return Err(Error::Broken("xref stream type"));
            }
            let data = self.decode(&dict, body)?;
            let w: Vec<usize> = match dict_get(&dict, "W") {
                Some(Obj::Array(a)) => a.iter().map(|x| x.num().unwrap_or(0.0) as usize).collect(),
                _ => return Err(Error::Broken("xref /W")),
            };
            if w.len() != 3 || w.iter().any(|&x| x > 8) {
                return Err(Error::Broken("xref /W"));
            }
            let size = dict_get(&dict, "Size").and_then(Obj::num).unwrap_or(0.0) as u32;
            let index: Vec<u32> = match dict_get(&dict, "Index") {
                Some(Obj::Array(a)) => a.iter().map(|x| x.num().unwrap_or(0.0) as u32).collect(),
                _ => vec![0, size],
            };
            let row = w[0] + w[1] + w[2];
            let field = |r: &[u8], from: usize, n: usize| -> u64 {
                r[from..from + n]
                    .iter()
                    .fold(0u64, |a, &b| a << 8 | b as u64)
            };
            let mut p = 0;
            for pair in index.chunks(2) {
                let (start, count) = (pair[0], *pair.get(1).unwrap_or(&0));
                for i in 0..count {
                    let Some(r) = data.get(p..p + row) else {
                        break;
                    };
                    p += row;
                    let t = if w[0] == 0 { 1 } else { field(r, 0, w[0]) };
                    let f2 = field(r, w[0], w[1]);
                    let f3 = field(r, w[0] + w[1], w[2]);
                    let loc = match t {
                        1 => (Loc::Offset(f2 as usize), f3 as u16),
                        2 => (Loc::InStream(f2 as u32, f3 as u32), 0),
                        _ => continue,
                    };
                    self.xref.entry(start + i).or_insert(loc);
                }
            }
            Ok((dict, true))
        }
    }

    /// `n g obj <obiekt> [stream ... endstream]` pod offsetem: slownik/obiekt
    /// i zakres danych strumienia.
    fn read_indirect_at(&self, off: usize) -> R<(Dict, Option<std::ops::Range<usize>>)> {
        let (obj, stream) = self.read_any_at(off)?;
        match obj {
            Obj::Dict(d) => Ok((d, stream)),
            _ => Err(Error::Broken("expected dictionary")),
        }
    }

    fn read_any_at(&self, off: usize) -> R<(Obj, Option<std::ops::Range<usize>>)> {
        let d = self.data;
        let mut lx = Lexer { d, pos: off };
        let (Some(Tok::Word(_)), Some(Tok::Word(_)), Some(Tok::Word(b"obj"))) =
            (lx.next(), lx.next(), lx.next())
        else {
            return Err(Error::Broken("indirect object"));
        };
        let obj = lx.object(0).ok_or(Error::Broken("object"))?;
        if let (Obj::Dict(dict), Some(Tok::Word(b"stream"))) = (&obj, lx.next()) {
            let mut s = lx.pos;
            if d.get(s) == Some(&b'\r') {
                s += 1;
            }
            if d.get(s) == Some(&b'\n') {
                s += 1;
            }
            let len = match dict_get(dict, "Length") {
                Some(Obj::Ref(n, _)) => self.resolve(*n).and_then(|o| o.num()),
                Some(o) => o.num(),
                None => None,
            };
            let end = match len {
                Some(l) if s + (l as usize) <= d.len() => s + l as usize,
                // Zla dlugosc - szukamy `endstream`.
                _ => {
                    s + d[s..]
                        .windows(9)
                        .position(|w| w == b"endstream")
                        .ok_or(Error::Broken("endstream"))?
                }
            };
            return Ok((obj, Some(s..end)));
        }
        Ok((obj, None))
    }

    /// Dane strumienia po filtrach (obslugujemy brak filtra i FlateDecode
    /// z predyktorem - tego uzywaja xref i strumienie obiektow).
    fn decode(&self, dict: &Dict, range: std::ops::Range<usize>) -> R<Vec<u8>> {
        let raw = &self.data[range];
        let filters: Vec<&[u8]> = match dict_get(dict, "Filter") {
            None => vec![],
            Some(Obj::Name(n)) => vec![n],
            Some(Obj::Array(a)) => a.iter().filter_map(Obj::name).collect(),
            _ => return Err(Error::Broken("filter")),
        };
        let mut out = raw.to_vec();
        for f in filters {
            if f != b"FlateDecode" && f != b"Fl" {
                return Err(Error::Broken("stream filter"));
            }
            out = deflate::inflate(&out, false)
                .or_else(|| deflate::inflate(&out, true))
                .ok_or(Error::Broken("flate"))?;
        }
        let parms = match dict_get(dict, "DecodeParms") {
            Some(Obj::Dict(p)) => Some(p.clone()),
            Some(Obj::Array(a)) => a.iter().find_map(|o| match o {
                Obj::Dict(p) => Some(p.clone()),
                _ => None,
            }),
            _ => None,
        };
        if let Some(p) = parms {
            let pred = dict_get(&p, "Predictor").and_then(Obj::num).unwrap_or(1.0) as u32;
            let cols = dict_get(&p, "Columns").and_then(Obj::num).unwrap_or(1.0) as usize;
            if pred >= 10 {
                out = png_unpredict(&out, cols).ok_or(Error::Broken("predictor"))?;
            } else if pred != 1 {
                return Err(Error::Broken("TIFF predictor"));
            }
        }
        Ok(out)
    }

    /// Obiekt o numerze `n` (bez strumienia - te czytamy przez `stream_of`).
    pub fn resolve(&self, n: u32) -> Option<Obj> {
        match self.xref.get(&n)?.0 {
            Loc::Offset(o) => self.read_any_at(o).ok().map(|(o, _)| o),
            Loc::InStream(s, i) => self.in_objstm(s, i),
        }
    }

    fn in_objstm(&self, s: u32, index: u32) -> Option<Obj> {
        if !self.objstm_cache.borrow().contains_key(&s) {
            let Loc::Offset(off) = self.xref.get(&s)?.0 else {
                return None;
            };
            let (dict, range) = self.read_indirect_at(off).ok()?;
            let data = self.decode(&dict, range?).ok()?;
            let n = dict_get(&dict, "N")?.num()? as usize;
            let first = dict_get(&dict, "First")?.num()? as usize;
            let mut lx = Lexer { d: &data, pos: 0 };
            let mut heads = Vec::with_capacity(n);
            for _ in 0..n {
                let (Some(Tok::Word(a)), Some(Tok::Word(b))) = (lx.next(), lx.next()) else {
                    break;
                };
                let num = std::str::from_utf8(a).ok()?.parse().ok()?;
                let off: usize = std::str::from_utf8(b).ok()?.parse().ok()?;
                heads.push((num, first + off));
            }
            self.objstm_cache.borrow_mut().insert(s, (data, heads));
        }
        let cache = self.objstm_cache.borrow();
        let (data, heads) = cache.get(&s)?;
        let &(_, off) = heads.get(index as usize)?;
        Lexer { d: data, pos: off }.object(0)
    }

    fn deref(&self, o: &Obj) -> Option<Obj> {
        match o {
            Obj::Ref(n, _) => self.resolve(*n),
            o => Some(o.clone()),
        }
    }

    pub fn root_pages(&self) -> R<(u32, Dict)> {
        let root = dict_get(&self.trailer, "Root").ok_or(Error::Broken("/Root"))?;
        let Some(Obj::Dict(cat)) = self.deref(root) else {
            return Err(Error::Broken("catalog"));
        };
        let Some(Obj::Ref(n, _)) = dict_get(&cat, "Pages") else {
            return Err(Error::Broken("/Pages"));
        };
        match self.resolve(*n) {
            Some(Obj::Dict(d)) => Ok((*n, d)),
            _ => Err(Error::Broken("page tree")),
        }
    }

    /// Strony w kolejnosci, z odziedziczonymi polami.
    pub fn pages(&self) -> R<Vec<Page>> {
        let (root, _) = self.root_pages()?;
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        self.walk(root, None, None, 0, &mut out, &mut seen, 0)?;
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(
        &self,
        n: u32,
        media: Option<[f64; 4]>,
        crop: Option<[f64; 4]>,
        rotate: i32,
        out: &mut Vec<Page>,
        seen: &mut HashSet<u32>,
        depth: usize,
    ) -> R<()> {
        if depth > 64 || !seen.insert(n) {
            return Err(Error::Broken("page tree cycle"));
        }
        let Some(Obj::Dict(d)) = self.resolve(n) else {
            return Err(Error::Broken("page node"));
        };
        let media = dict_get(&d, "MediaBox")
            .and_then(|o| self.deref(o))
            .and_then(|o| norm_box(&o))
            .or(media);
        let crop = dict_get(&d, "CropBox")
            .and_then(|o| self.deref(o))
            .and_then(|o| norm_box(&o))
            .or(crop);
        let rotate = dict_get(&d, "Rotate")
            .and_then(|o| self.deref(o))
            .and_then(|o| o.num())
            .map_or(rotate, |v| v as i32);
        let is_pages = dict_get(&d, "Type").and_then(Obj::name) == Some(b"Pages")
            || dict_get(&d, "Kids").is_some();
        if is_pages {
            let Some(Obj::Array(kids)) = dict_get(&d, "Kids").and_then(|o| self.deref(o)) else {
                return Err(Error::Broken("/Kids"));
            };
            for k in kids {
                if let Obj::Ref(kn, _) = k {
                    self.walk(kn, media, crop, rotate, out, seen, depth + 1)?;
                }
            }
        } else {
            let gen = self.xref.get(&n).map_or(0, |e| e.1);
            out.push(Page {
                num: n,
                gen,
                dict: d,
                // Brak MediaBox nigdzie w drzewie - A4 jak w wiekszosci czytnikow.
                media: media.unwrap_or([0.0, 0.0, 595.0, 842.0]),
                crop,
                rotate: rotate.rem_euclid(360) / 90 * 90,
            });
        }
        Ok(())
    }
}

/// Predyktory PNG (typ filtra na poczatku kazdego wiersza).
fn png_unpredict(data: &[u8], cols: usize) -> Option<Vec<u8>> {
    let row = cols + 1;
    let mut out = Vec::with_capacity(data.len() / row * cols);
    let mut prev = vec![0u8; cols];
    for r in data.chunks(row) {
        if r.len() < row {
            break;
        }
        let (t, src) = (r[0], &r[1..]);
        let mut cur = vec![0u8; cols];
        for i in 0..cols {
            let a = if i > 0 { cur[i - 1] as i32 } else { 0 };
            let b = prev[i] as i32;
            let c = if i > 0 { prev[i - 1] as i32 } else { 0 };
            let x = src[i] as i32;
            cur[i] = match t {
                0 => x,
                1 => x + a,
                2 => x + b,
                3 => x + (a + b) / 2,
                4 => {
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    x + if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                _ => return None,
            } as u8;
        }
        out.extend_from_slice(&cur);
        prev = cur;
    }
    Some(out)
}

// ----- zapis aktualizacji -----------------------------------------------------

/// Aktualizacja przyrostowa: nowe obiekty i nowe wersje starych.
pub struct Update<'a> {
    doc: &'a Doc<'a>,
    out: Vec<u8>,
    /// (numer, generacja, offset) zapisanych obiektow.
    written: Vec<(u32, u16, usize)>,
    next: u32,
}

impl<'a> Update<'a> {
    pub fn new(doc: &'a Doc<'a>) -> Self {
        let mut out = doc.data.to_vec();
        if !out.ends_with(b"\n") {
            out.push(b'\n');
        }
        Self {
            doc,
            out,
            written: Vec::new(),
            next: doc.size,
        }
    }

    pub fn alloc(&mut self) -> u32 {
        self.next += 1;
        self.next - 1
    }

    pub fn put(&mut self, num: u32, gen: u16, obj: &Obj) {
        self.written.push((num, gen, self.out.len()));
        self.out
            .extend_from_slice(format!("{num} {gen} obj\n").as_bytes());
        obj.write(&mut self.out);
        self.out.extend_from_slice(b"\nendobj\n");
    }

    /// Nowy strumien (skompresowany), zwraca numer obiektu.
    pub fn stream(&mut self, data: &[u8]) -> u32 {
        let num = self.alloc();
        let z = deflate::zlib(data);
        self.written.push((num, 0, self.out.len()));
        self.out.extend_from_slice(
            format!(
                "{num} 0 obj\n<< /Length {} /Filter /FlateDecode >>\nstream\n",
                z.len()
            )
            .as_bytes(),
        );
        self.out.extend_from_slice(&z);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
        num
    }

    pub fn finish(mut self) -> Vec<u8> {
        let mut trailer = Dict::new();
        for key in ["Root", "Info", "ID"] {
            if let Some(v) = dict_get(&self.doc.trailer, key) {
                dict_set(&mut trailer, key, v.clone());
            }
        }
        dict_set(&mut trailer, "Prev", Obj::int(self.doc.startxref as i64));
        if self.doc.xref_is_stream {
            // Strumien xref: tez obiekt, wiec wpisuje takze siebie.
            let me = self.alloc();
            let at = self.out.len();
            self.written.push((me, 0, at));
            dict_set(&mut trailer, "Size", Obj::int(self.next as i64));
            self.written.sort_by_key(|w| w.0);
            let mut rows = Vec::with_capacity(self.written.len() * 7);
            let mut index = Vec::new();
            for &(n, g, off) in &self.written {
                rows.push(1u8);
                rows.extend_from_slice(&(off as u32).to_be_bytes());
                rows.extend_from_slice(&g.to_be_bytes());
                index.push(Obj::int(n as i64));
                index.push(Obj::int(1));
            }
            dict_set(&mut trailer, "Type", Obj::Name(b"XRef".to_vec()));
            dict_set(
                &mut trailer,
                "W",
                Obj::Array(vec![Obj::int(1), Obj::int(4), Obj::int(2)]),
            );
            dict_set(&mut trailer, "Index", Obj::Array(index));
            dict_set(&mut trailer, "Length", Obj::int(rows.len() as i64));
            self.out
                .extend_from_slice(format!("{me} 0 obj\n").as_bytes());
            Obj::Dict(trailer).write(&mut self.out);
            self.out.extend_from_slice(b"\nstream\n");
            self.out.extend_from_slice(&rows);
            self.out.extend_from_slice(b"\nendstream\nendobj\n");
            self.out
                .extend_from_slice(format!("startxref\n{at}\n%%EOF\n").as_bytes());
        } else {
            let at = self.out.len();
            dict_set(&mut trailer, "Size", Obj::int(self.next as i64));
            self.written.sort_by_key(|w| w.0);
            self.out.extend_from_slice(b"xref\n");
            for &(n, g, off) in &self.written {
                self.out
                    .extend_from_slice(format!("{n} 1\n{off:010} {g:05} n \n").as_bytes());
            }
            self.out.extend_from_slice(b"trailer\n");
            Obj::Dict(trailer).write(&mut self.out);
            self.out
                .extend_from_slice(format!("\nstartxref\n{at}\n%%EOF\n").as_bytes());
        }
        self.out
    }
}

/// Macierz `cm`: jednostki canvasu -> przestrzen uzytkownika strony.
/// `page` - prostokat strony na canvasie (x, y, w, h), `vis` - pole widoczne
/// strony, `rotate` - `/Rotate` (0/90/180/270, zgodnie ze wskazowkami).
pub fn canvas_to_page(page: (f64, f64, f64, f64), vis: [f64; 4], rotate: i32) -> [f64; 6] {
    let (px, py, pw, ph) = page;
    let (x0, y0, x1, y1) = (vis[0], vis[1], vis[2], vis[3]);
    let (cw, ch) = (x1 - x0, y1 - y0);
    match rotate {
        90 => [
            0.0,
            ch / pw,
            cw / ph,
            0.0,
            x0 - py * cw / ph,
            y0 - px * ch / pw,
        ],
        180 => [
            -cw / pw,
            0.0,
            0.0,
            ch / ph,
            x1 + px * cw / pw,
            y0 - py * ch / ph,
        ],
        270 => [
            0.0,
            -ch / pw,
            -cw / ph,
            0.0,
            x1 + py * cw / ph,
            y1 + px * ch / pw,
        ],
        _ => [
            cw / pw,
            0.0,
            0.0,
            -ch / ph,
            x0 - px * cw / pw,
            y1 + py * ch / ph,
        ],
    }
}

pub fn apply(m: &[f64; 6], x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_obiektow() {
        let o =
            parse(b"<< /Type /Page /Kids [1 0 R 2 0 R] /Name (a (b) \\) c) /H <ABCD> /N -1.5 >>")
                .unwrap();
        let Obj::Dict(d) = &o else { panic!() };
        assert_eq!(dict_get(d, "Type").and_then(Obj::name), Some(&b"Page"[..]));
        assert_eq!(
            dict_get(d, "Kids"),
            Some(&Obj::Array(vec![Obj::Ref(1, 0), Obj::Ref(2, 0)]))
        );
        assert_eq!(
            dict_get(d, "Name"),
            Some(&Obj::Raw(b"(a (b) \\) c)".to_vec()))
        );
        assert_eq!(dict_get(d, "N").and_then(Obj::num), Some(-1.5));
        let mut w = Vec::new();
        o.write(&mut w);
        assert_eq!(parse(&w).unwrap(), o, "zapis i odczyt daja to samo");
    }

    /// Narzedzie do testow na zywo: z `SPECTRENOTES_PDF` robi plik, w ktorym
    /// strona 1 ma `/Rotate 90`, strona 2 `/Rotate 270`, a strona 3 `CropBox`
    /// wciety z kazdej strony - `SPECTRENOTES_PDF_OUT` (aktualizacja przyrostowa).
    #[test]
    #[ignore]
    fn przygotuj_obrocone_i_przyciete() {
        let src = std::fs::read(std::env::var("SPECTRENOTES_PDF").unwrap()).unwrap();
        let doc = Doc::open(&src).unwrap();
        let pages = doc.pages().unwrap();
        let mut up = Update::new(&doc);
        for (i, rot) in [(0usize, 90i64), (1, 270)] {
            let mut d = pages[i].dict.clone();
            dict_set(&mut d, "Rotate", Obj::int(rot));
            up.put(pages[i].num, pages[i].gen, &Obj::Dict(d));
        }
        let m = pages[2].media;
        let mut d = pages[2].dict.clone();
        dict_set(
            &mut d,
            "CropBox",
            Obj::Array(
                [m[0] + 60.0, m[1] + 150.0, m[2] - 90.0, m[3] - 40.0]
                    .iter()
                    .map(|&v| Obj::real(v))
                    .collect(),
            ),
        );
        up.put(pages[2].num, pages[2].gen, &Obj::Dict(d));
        std::fs::write(std::env::var("SPECTRENOTES_PDF_OUT").unwrap(), up.finish()).unwrap();
    }

    #[test]
    fn predyktor_png_up() {
        // Dwa wiersze po 3 kolumny, filtr Up (2).
        let data = [2, 1, 2, 3, 2, 1, 1, 1];
        assert_eq!(png_unpredict(&data, 3).unwrap(), vec![1, 2, 3, 2, 3, 4]);
    }

    #[test]
    fn macierz_canvas_strona_dla_obrotow() {
        let page = (0.0, 100.0, 2880.0, 4000.0);
        let vis = [0.0, 0.0, 600.0, 800.0];
        // Bez obrotu: lewy gorny rog strony na canvasie -> (0, 800).
        let m = canvas_to_page(page, vis, 0);
        assert_eq!(apply(&m, 0.0, 100.0), (0.0, 800.0));
        assert_eq!(apply(&m, 2880.0, 4100.0), (600.0, 0.0));
        // 90: strona ogladana na lezaco ma szerokosc = wysokosc pola.
        let page90 = (0.0, 0.0, 800.0, 600.0);
        let m = canvas_to_page(page90, vis, 90);
        // Lewy gorny rog ekranu to (x0, y0) pola po obrocie w prawo.
        let (x, y) = apply(&m, 0.0, 0.0);
        assert!((x - 0.0).abs() < 1e-9 && (y - 0.0).abs() < 1e-9, "{x} {y}");
        let (x, y) = apply(&m, 800.0, 0.0);
        assert!(
            (x - 0.0).abs() < 1e-9 && (y - 800.0).abs() < 1e-9,
            "{x} {y}"
        );
        let m = canvas_to_page(page, vis, 180);
        assert_eq!(apply(&m, 0.0, 100.0), (600.0, 0.0));
        let m = canvas_to_page(page90, vis, 270);
        let (x, y) = apply(&m, 0.0, 0.0);
        assert!(
            (x - 600.0).abs() < 1e-9 && (y - 800.0).abs() < 1e-9,
            "{x} {y}"
        );
    }
}
