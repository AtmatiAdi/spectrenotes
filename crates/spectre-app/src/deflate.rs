//! Kompresja zlib (RFC 1950/1951) do strumieni PDF (`/FlateDecode`): LZ77
//! z lancuchami haszy + stale kody Huffmana, jeden blok. Wlasna, bo to ~150
//! linii, a crate `flate2`/`miniz_oxide` bylby najwiekszym w binarce po
//! `windows`. Tresc strony PDF to tekst z powtarzajacymi sie operatorami
//! i cyframi - LZ77 zdejmuje z niego wiekszosc, dynamiczne kody dalyby jeszcze
//! ~10 %, ktorych nie potrzebujemy.

const WINDOW: usize = 32 * 1024;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const HASH_BITS: u32 = 15;
const MAX_CHAIN: usize = 48;

/// Dlugosci: (kod - 257) -> (dlugosc bazowa, bity dodatkowe).
const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// Odleglosci: kod -> (odleglosc bazowa, bity dodatkowe).
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            acc: 0,
            n: 0,
        }
    }

    /// `bits` najmlodszych bitow `v`, od najmlodszego (tak pakuje deflate).
    fn put(&mut self, v: u32, bits: u32) {
        self.acc |= (v as u64) << self.n;
        self.n += bits;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// Kod Huffmana idzie od najstarszego bitu - odwracamy przed `put`.
    fn code(&mut self, code: u32, bits: u32) {
        let mut r = 0u32;
        for i in 0..bits {
            r |= ((code >> i) & 1) << (bits - 1 - i);
        }
        self.put(r, bits);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

/// Stale kody Huffmana literalow/dlugosci (RFC 1951, 3.2.6).
fn put_symbol(w: &mut BitWriter, sym: u32) {
    match sym {
        0..=143 => w.code(0x30 + sym, 8),
        144..=255 => w.code(0x190 + (sym - 144), 9),
        256..=279 => w.code(sym - 256, 7),
        _ => w.code(0xC0 + (sym - 280), 8),
    }
}

fn put_length(w: &mut BitWriter, len: usize) {
    let i = LEN_BASE
        .iter()
        .rposition(|&b| b as usize <= len)
        .unwrap_or(0);
    put_symbol(w, 257 + i as u32);
    if LEN_EXTRA[i] > 0 {
        w.put((len - LEN_BASE[i] as usize) as u32, LEN_EXTRA[i] as u32);
    }
}

fn put_distance(w: &mut BitWriter, dist: usize) {
    let i = DIST_BASE
        .iter()
        .rposition(|&b| b as usize <= dist)
        .unwrap_or(0);
    w.code(i as u32, 5);
    if DIST_EXTRA[i] > 0 {
        w.put((dist - DIST_BASE[i] as usize) as u32, DIST_EXTRA[i] as u32);
    }
}

#[inline]
fn hash3(d: &[u8], i: usize) -> usize {
    let v = (d[i] as u32) << 16 | (d[i + 1] as u32) << 8 | d[i + 2] as u32;
    (v.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
}

/// Strumien zlib: naglowek, jeden blok deflate ze stalymi kodami, Adler-32.
pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.put(0x78, 8);
    w.put(0x9C, 8);
    // BFINAL=1, BTYPE=01 (stale kody).
    w.put(1, 1);
    w.put(1, 2);

    let n = data.len();
    let mut head = vec![usize::MAX; 1 << HASH_BITS];
    let mut prev = vec![usize::MAX; WINDOW];
    let mut i = 0usize;
    while i < n {
        let mut best_len = 0usize;
        let mut best_dist = 0usize;
        if i + MIN_MATCH <= n {
            let h = hash3(data, i);
            let mut cand = head[h];
            let mut chain = 0;
            let limit = (n - i).min(MAX_MATCH);
            while cand != usize::MAX && i - cand <= WINDOW && chain < MAX_CHAIN {
                let dist = i - cand;
                if dist > 0 && data[cand + best_len] == data[i + best_len] {
                    let mut l = 0;
                    while l < limit && data[cand + l] == data[i + l] {
                        l += 1;
                    }
                    if l > best_len {
                        best_len = l;
                        best_dist = dist;
                        if l == limit {
                            break;
                        }
                    }
                }
                let next = prev[cand % WINDOW];
                if next == usize::MAX || next >= cand {
                    break;
                }
                cand = next;
                chain += 1;
            }
        }
        if best_len >= MIN_MATCH {
            put_length(&mut w, best_len);
            put_distance(&mut w, best_dist);
            // Hasze dla wszystkich pozycji dopasowania - inaczej gubimy
            // kandydatow na kolejne dopasowania.
            for k in i..i + best_len {
                if k + MIN_MATCH <= n {
                    let h = hash3(data, k);
                    prev[k % WINDOW] = head[h];
                    head[h] = k;
                }
            }
            i += best_len;
        } else {
            put_symbol(&mut w, data[i] as u32);
            if i + MIN_MATCH <= n {
                let h = hash3(data, i);
                prev[i % WINDOW] = head[h];
                head[h] = i;
            }
            i += 1;
        }
    }
    put_symbol(&mut w, 256);
    let mut out = w.finish();
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    let adler = (b << 16) | a;
    out.extend_from_slice(&adler.to_be_bytes());
    out
}

/// Dekompresja zlib/deflate (RFC 1950/1951) - pelna: bloki surowe, stale
/// i dynamiczne kody Huffmana. Do czytania cudzych PDF-ow (strumienie
/// `/FlateDecode`, tablice xref w strumieniach). Na bledzie `None`.
/// `raw` = sam deflate, bez naglowka zlib.
pub fn inflate(data: &[u8], raw: bool) -> Option<Vec<u8>> {
    struct Bits<'a> {
        d: &'a [u8],
        pos: usize,
        buf: u64,
        cnt: u32,
    }
    impl Bits<'_> {
        fn need(&mut self, n: u32) -> Option<()> {
            while self.cnt < n {
                let b = *self.d.get(self.pos)?;
                self.pos += 1;
                self.buf |= (b as u64) << self.cnt;
                self.cnt += 8;
            }
            Some(())
        }
        fn bits(&mut self, n: u32) -> Option<u32> {
            if n == 0 {
                return Some(0);
            }
            self.need(n)?;
            let v = (self.buf & ((1u64 << n) - 1)) as u32;
            self.buf >>= n;
            self.cnt -= n;
            Some(v)
        }
        fn align(&mut self) {
            let r = self.cnt % 8;
            self.buf >>= r;
            self.cnt -= r;
        }
    }
    /// Kanoniczny kod Huffmana: liczba kodow danej dlugosci i symbole po kolei.
    struct Huff {
        count: [u16; 16],
        sym: Vec<u16>,
    }
    impl Huff {
        fn new(lens: &[u8]) -> Self {
            let mut count = [0u16; 16];
            for &l in lens {
                count[l as usize] += 1;
            }
            count[0] = 0;
            let mut offs = [0u16; 16];
            for i in 1..15 {
                offs[i + 1] = offs[i] + count[i];
            }
            let mut sym = vec![0u16; lens.len()];
            for (s, &l) in lens.iter().enumerate() {
                if l != 0 {
                    sym[offs[l as usize] as usize] = s as u16;
                    offs[l as usize] += 1;
                }
            }
            Self { count, sym }
        }
        fn decode(&self, b: &mut Bits) -> Option<u16> {
            let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
            for len in 1..16 {
                code |= b.bits(1)? as i32;
                let count = self.count[len] as i32;
                if code - count < first {
                    return self.sym.get((index + (code - first)) as usize).copied();
                }
                index += count;
                first += count;
                first <<= 1;
                code <<= 1;
            }
            None
        }
    }
    let start = if raw {
        0
    } else {
        if data.len() < 2
            || data[0] & 0x0f != 8
            || ((data[0] as u16) << 8 | data[1] as u16) % 31 != 0
        {
            return None;
        }
        2
    };
    let mut b = Bits {
        d: &data[start..],
        pos: 0,
        buf: 0,
        cnt: 0,
    };
    let mut out: Vec<u8> = Vec::with_capacity(data.len() * 4);
    loop {
        let last = b.bits(1)?;
        match b.bits(2)? {
            0 => {
                b.align();
                let len = b.bits(16)? as usize;
                let nlen = b.bits(16)? as usize;
                if len != !nlen & 0xffff {
                    return None;
                }
                // Po wyrownaniu bufor bitow jest pusty albo trzyma pelne bajty.
                for _ in 0..len {
                    out.push(b.bits(8)? as u8);
                }
            }
            t @ (1 | 2) => {
                let (lit, dist) = if t == 1 {
                    let mut l = [0u8; 288];
                    l[..144].fill(8);
                    l[144..256].fill(9);
                    l[256..280].fill(7);
                    l[280..].fill(8);
                    (Huff::new(&l), Huff::new(&[5u8; 30]))
                } else {
                    let nlen = b.bits(5)? as usize + 257;
                    let ndist = b.bits(5)? as usize + 1;
                    let ncode = b.bits(4)? as usize + 4;
                    const ORDER: [usize; 19] = [
                        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                    ];
                    let mut cl = [0u8; 19];
                    for &o in ORDER.iter().take(ncode) {
                        cl[o] = b.bits(3)? as u8;
                    }
                    let ch = Huff::new(&cl);
                    let mut lens = vec![0u8; nlen + ndist];
                    let mut i = 0;
                    while i < nlen + ndist {
                        let s = ch.decode(&mut b)?;
                        let (val, rep) = match s {
                            0..=15 => (s as u8, 1),
                            16 => (*lens.get(i.checked_sub(1)?)?, 3 + b.bits(2)? as usize),
                            17 => (0, 3 + b.bits(3)? as usize),
                            _ => (0, 11 + b.bits(7)? as usize),
                        };
                        if i + rep > lens.len() {
                            return None;
                        }
                        lens[i..i + rep].fill(val);
                        i += rep;
                    }
                    (Huff::new(&lens[..nlen]), Huff::new(&lens[nlen..]))
                };
                loop {
                    let s = lit.decode(&mut b)? as usize;
                    if s < 256 {
                        out.push(s as u8);
                    } else if s == 256 {
                        break;
                    } else {
                        let i = s - 257;
                        let len =
                            *LEN_BASE.get(i)? as usize + b.bits(LEN_EXTRA[i] as u32)? as usize;
                        let di = dist.decode(&mut b)? as usize;
                        let d =
                            *DIST_BASE.get(di)? as usize + b.bits(DIST_EXTRA[di] as u32)? as usize;
                        let from = out.len().checked_sub(d)?;
                        for k in 0..len {
                            out.push(out[from + k]);
                        }
                    }
                }
            }
            _ => return None,
        }
        if last == 1 {
            return Some(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dekoder tylko dla tego, co koder produkuje (jeden blok, stale kody) -
    /// wystarczy, zeby sprawdzic, ze strumien jest poprawny bit po bicie.
    fn inflate(z: &[u8]) -> Vec<u8> {
        struct R<'a> {
            d: &'a [u8],
            pos: usize,
            bit: u32,
        }
        impl R<'_> {
            fn bit(&mut self) -> u32 {
                let v = (self.d[self.pos] >> self.bit) & 1;
                self.bit += 1;
                if self.bit == 8 {
                    self.bit = 0;
                    self.pos += 1;
                }
                v as u32
            }
            fn bits(&mut self, n: u32) -> u32 {
                (0..n).fold(0, |acc, i| acc | self.bit() << i)
            }
            fn code(&mut self, n: u32) -> u32 {
                (0..n).fold(0, |acc, _| acc << 1 | self.bit())
            }
        }
        assert_eq!(z[0], 0x78);
        let mut r = R {
            d: z,
            pos: 2,
            bit: 0,
        };
        assert_eq!(r.bits(1), 1, "BFINAL");
        assert_eq!(r.bits(2), 1, "BTYPE stale");
        let mut out = Vec::new();
        loop {
            // Stale kody: 7 bitow 0..=23 -> 256..=279; 8 bitow 48..=191 ->
            // 0..=143, 192..=199 -> 280..=287; 9 bitow 400..=511 -> 144..=255.
            let c7 = r.code(7);
            let sym = if c7 <= 23 {
                256 + c7
            } else {
                let c8 = c7 << 1 | r.bit();
                if (48..=191).contains(&c8) {
                    c8 - 48
                } else if (192..=199).contains(&c8) {
                    280 + (c8 - 192)
                } else {
                    let c9 = c8 << 1 | r.bit();
                    144 + (c9 - 400)
                }
            };
            if sym < 256 {
                out.push(sym as u8);
            } else if sym == 256 {
                break;
            } else {
                let i = (sym - 257) as usize;
                let len = LEN_BASE[i] as usize + r.bits(LEN_EXTRA[i] as u32) as usize;
                let di = r.code(5) as usize;
                let dist = DIST_BASE[di] as usize + r.bits(DIST_EXTRA[di] as u32) as usize;
                let start = out.len() - dist;
                for k in 0..len {
                    out.push(out[start + k]);
                }
            }
        }
        out
    }

    /// Pelny dekoder zgadza sie z testowym na strumieniach tego kodera...
    #[test]
    fn pelny_dekoder_na_wlasnych_strumieniach() {
        let s: Vec<u8> = (0..20_000)
            .map(|i| b"0123456789 m l h f\n"[i % 19])
            .collect();
        for data in [&b""[..], b"a", b"abcabcabcabc", &s] {
            assert_eq!(super::inflate(&zlib(data), false).unwrap(), data);
        }
    }

    /// ...i czyta bloki surowe i dynamiczne, ktorych nasz koder nie robi.
    #[test]
    fn pelny_dekoder_bloki_surowe_i_dynamiczne() {
        // Blok surowy (zlib, poziom 0), z naglowkiem i Adler-32.
        let stored = [
            0x78, 0x01, 0x01, 0x05, 0x00, 0xfa, 0xff, b'h', b'e', b'l', b'l', b'o', 0x06, 0x2c,
            0x02, 0x15,
        ];
        assert_eq!(super::inflate(&stored, false).unwrap(), b"hello");
        // Blok dynamiczny (BTYPE=2): surowy deflate z .NET `DeflateStream`
        // (CompressionLevel.Optimal), wygenerowany na Windows 11.
        let dynamic = [
            0xb5, 0xca, 0xd9, 0x15, 0x82, 0x30, 0x10, 0x40, 0xd1, 0x56, 0x5e, 0x01, 0x1e, 0x8e,
            0xfb, 0xd2, 0x03, 0x9f, 0x36, 0x30, 0xc2, 0x08, 0xc1, 0x84, 0xc1, 0x2c, 0x2e, 0xa9,
            0xde, 0x2a, 0xbc, 0xdf, 0xb7, 0xb5, 0xa8, 0x01, 0xb7, 0xa4, 0x12, 0xe8, 0xcd, 0x5b,
            0x24, 0xb9, 0x8c, 0x04, 0xcd, 0x2b, 0xaa, 0x54, 0xf3, 0x1d, 0x83, 0x26, 0x2f, 0x4c,
            0x52, 0xe7, 0x86, 0xf6, 0xaf, 0xfd, 0x3a, 0x2a, 0xcf, 0xe2, 0xba, 0x07, 0xb7, 0x68,
            0xef, 0x99, 0xbb, 0x7d, 0x98, 0x4a, 0x58, 0x12, 0xf6, 0xd2, 0x48, 0x1e, 0x15, 0x2f,
            0xf5, 0x4b, 0x6f, 0x03, 0xeb, 0xcd, 0x76, 0xb7, 0x3f, 0x1c, 0x4f, 0xe7, 0x4b, 0xc3,
            0x0f,
        ];
        assert_eq!((dynamic[0] >> 1) & 3, 2, "wektor ma blok dynamiczny");
        let want = "Lorem ipsum dolor sit amet, zazolc gesla jazn. ".repeat(3)
            + "The quick brown fox jumps over the lazy dog 0123456789. ";
        let got = super::inflate(&dynamic, true).unwrap();
        assert_eq!(String::from_utf8_lossy(&got), want);
        // Uszkodzone dane: brak paniki, `None`.
        assert!(super::inflate(&[0x78, 0x9c, 0xff, 0xff, 0xff], false).is_none());
        assert!(super::inflate(&[1, 2, 3], false).is_none());
    }

    #[test]
    fn puste_i_krotkie() {
        assert_eq!(inflate(&zlib(b"")), b"");
        assert_eq!(inflate(&zlib(b"a")), b"a");
        assert_eq!(inflate(&zlib(b"abc")), b"abc");
    }

    #[test]
    fn powtorzenia_i_dlugie_dopasowania() {
        let s: Vec<u8> = (0..5000).map(|i| b"0123456789 m l h f\n"[i % 19]).collect();
        let z = zlib(&s);
        assert!(z.len() < s.len() / 10, "{} vs {}", z.len(), s.len());
        assert_eq!(inflate(&z), s);
        // Same zera: dopasowania po 258 bajtow, odleglosc 1.
        let zeros = vec![0u8; 100_000];
        let z = zlib(&zeros);
        assert!(z.len() < 1000, "{}", z.len());
        assert_eq!(inflate(&z), zeros);
    }

    #[test]
    fn losowe_bez_dopasowan_i_tresc_pdf() {
        let mut x = 0x1234_5678u32;
        let rnd: Vec<u8> = (0..20_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        assert_eq!(inflate(&zlib(&rnd)), rnd);
        let mut pdf = String::new();
        for i in 0..3000 {
            pdf.push_str(&format!(
                "{:.1} {:.1} l\n",
                i as f32 * 0.37,
                (i * 7 % 900) as f32 * 1.3
            ));
        }
        let z = zlib(pdf.as_bytes());
        assert_eq!(inflate(&z), pdf.as_bytes());
        assert!(z.len() < pdf.len() / 2, "{} vs {}", z.len(), pdf.len());
    }
}
