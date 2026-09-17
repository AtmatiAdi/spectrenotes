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
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
    131, 163, 195, 227, 258,
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
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
    13, 13,
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
            pdf.push_str(&format!("{:.1} {:.1} l\n", i as f32 * 0.37, (i * 7 % 900) as f32 * 1.3));
        }
        let z = zlib(pdf.as_bytes());
        assert_eq!(inflate(&z), pdf.as_bytes());
        assert!(z.len() < pdf.len() / 2, "{} vs {}", z.len(), pdf.len());
    }
}
