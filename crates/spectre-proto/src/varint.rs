//! LEB128 (u64) i zigzag (i64). Fundament kodowania - probki to prawie same
//! male delty, wiec varint jest tu roznica miedzy 24 a ~7 bajtami na probke.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Truncated;

#[inline]
pub fn put_u64(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

#[inline]
pub fn put_i64(out: &mut Vec<u8>, v: i64) {
    put_u64(out, zigzag(v));
}

#[inline]
pub fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

#[inline]
pub fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

/// Kursor odczytu. Kazdy blad to `Truncated` - dla formatu append-only
/// niedokonczony rekord i uszkodzony rekord znacza to samo: obciac.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.pos >= self.buf.len()
    }

    pub fn u8(&mut self) -> Result<u8, Truncated> {
        let b = *self.buf.get(self.pos).ok_or(Truncated)?;
        self.pos += 1;
        Ok(b)
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], Truncated> {
        let end = self.pos.checked_add(n).ok_or(Truncated)?;
        let s = self.buf.get(self.pos..end).ok_or(Truncated)?;
        self.pos = end;
        Ok(s)
    }

    pub fn u32_le(&mut self) -> Result<u32, Truncated> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u16_le(&mut self) -> Result<u16, Truncated> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u64_le(&mut self) -> Result<u64, Truncated> {
        let b = self.bytes(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }

    pub fn f32_le(&mut self) -> Result<f32, Truncated> {
        Ok(f32::from_bits(self.u32_le()?))
    }

    pub fn u64(&mut self) -> Result<u64, Truncated> {
        let mut v: u64 = 0;
        let mut shift = 0u32;
        loop {
            let b = self.u8()?;
            if shift >= 64 {
                return Err(Truncated);
            }
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
        }
    }

    pub fn i64(&mut self) -> Result<i64, Truncated> {
        Ok(unzigzag(self.u64()?))
    }

    pub fn usize(&mut self) -> Result<usize, Truncated> {
        usize::try_from(self.u64()?).map_err(|_| Truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zigzag_roundtrip() {
        for v in [0i64, 1, -1, 2, -2, 63, -64, i64::MAX, i64::MIN] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
    }

    #[test]
    fn varint_roundtrip() {
        let mut out = Vec::new();
        let vals = [0u64, 1, 127, 128, 300, u32::MAX as u64, u64::MAX];
        for v in vals {
            put_u64(&mut out, v);
        }
        let mut r = Reader::new(&out);
        for v in vals {
            assert_eq!(r.u64().unwrap(), v);
        }
        assert!(r.is_empty());
    }

    #[test]
    fn truncated_is_error_not_panic() {
        let mut out = Vec::new();
        put_u64(&mut out, 300);
        let mut r = Reader::new(&out[..1]);
        assert_eq!(r.u64(), Err(Truncated));
    }
}
