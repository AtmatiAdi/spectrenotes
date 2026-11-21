//! PNG bez zaleznosci: zapis RGB z filtrem 0 i kompresja zlib z aplikacji
//! (`spectre-app/src/deflate.rs`), odczyt tego, co zapisalismy (i kazdego PNG
//! RGB/RGBA 8 bit z filtrami 0-4). Piksele w pamieci to BGRA jak z renderera.

use std::path::Path;

#[path = "../../spectre-app/src/deflate.rs"]
#[allow(dead_code)]
mod deflate;

use spectre_proto::crc32::crc32;

const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// BGRA (alfa pomijana) -> bajty PNG RGB.
pub fn encode(w: u32, h: u32, bgra: &[u8]) -> Vec<u8> {
    assert_eq!(bgra.len(), (w * h * 4) as usize);
    let mut raw = Vec::with_capacity((w * 3 + 1) as usize * h as usize);
    for row in bgra.chunks_exact(w as usize * 4) {
        raw.push(0);
        for px in row.chunks_exact(4) {
            raw.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(&SIG);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &deflate::zlib(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

pub fn write(path: &Path, w: u32, h: u32, bgra: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, encode(w, h, bgra))
}

/// Bajty PNG -> (w, h, BGRA z alfa 255). `None`, gdy to nie jest PNG
/// RGB/RGBA 8 bit bez przeplotu.
pub fn decode(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut rest = bytes.strip_prefix(&SIG)?;
    let (mut w, mut h, mut ch) = (0u32, 0u32, 0usize);
    let mut idat = Vec::new();
    while rest.len() >= 12 {
        let len = u32::from_be_bytes(rest[..4].try_into().ok()?) as usize;
        let kind = &rest[4..8];
        let data = rest.get(8..8 + len)?;
        match kind {
            b"IHDR" => {
                w = u32::from_be_bytes(data[..4].try_into().ok()?);
                h = u32::from_be_bytes(data[4..8].try_into().ok()?);
                if data[8] != 8 || data[12] != 0 {
                    return None;
                }
                ch = match data[9] {
                    2 => 3,
                    6 => 4,
                    _ => return None,
                };
            }
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        rest = &rest[12 + len..];
    }
    let raw = deflate::inflate(&idat, false)?;
    let stride = w as usize * ch;
    if raw.len() < (stride + 1) * h as usize {
        return None;
    }
    let mut cur = vec![0u8; stride];
    let mut prev = vec![0u8; stride];
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        let line = &raw[y * (stride + 1)..(y + 1) * (stride + 1)];
        let f = line[0];
        for i in 0..stride {
            let x = line[1 + i];
            let a = if i >= ch { cur[i - ch] } else { 0 };
            let b = prev[i];
            let c = if i >= ch { prev[i - ch] } else { 0 };
            cur[i] = match f {
                0 => x,
                1 => x.wrapping_add(a),
                2 => x.wrapping_add(b),
                3 => x.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => {
                    let p = a as i16 + b as i16 - c as i16;
                    let (pa, pb, pc) = (
                        (p - a as i16).abs(),
                        (p - b as i16).abs(),
                        (p - c as i16).abs(),
                    );
                    x.wrapping_add(if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    })
                }
                _ => return None,
            };
        }
        for px in cur.chunks_exact(ch) {
            out.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        std::mem::swap(&mut cur, &mut prev);
    }
    Some((w, h, out))
}

pub fn read(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    decode(&std::fs::read(path).ok()?)
}

/// Liczba pikseli, ktore roznia sie w ktorymkolwiek kanale koloru (alfa
/// pomijana: renderer jej nie uzywa). Rozne rozmiary = wszystkie piksele.
pub fn diff_pixels(a: &[u8], b: &[u8]) -> usize {
    if a.len() != b.len() {
        return a.len().max(b.len()) / 4;
    }
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(p, q)| p[..3] != q[..3])
        .count()
}

/// Skrot FNV-1a pikseli koloru (bez alfy) - do porownania z wzorcem bez pliku.
pub fn hash(bgra: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for px in bgra.chunks_exact(4) {
        for &b in &px[..3] {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// Skrot FNV-1a dowolnych bajtow (plik `.ops`).
pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zapis_i_odczyt_daja_te_same_piksele() {
        let (w, h) = (37u32, 11u32);
        let px: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i * 7) as u8, (i * 13) as u8, (i * 29) as u8, 255])
            .collect();
        let bytes = encode(w, h, &px);
        let (w2, h2, back) = decode(&bytes).unwrap();
        assert_eq!((w2, h2), (w, h));
        assert_eq!(diff_pixels(&px, &back), 0);
        assert_eq!(hash(&px), hash(&back));
    }
}
