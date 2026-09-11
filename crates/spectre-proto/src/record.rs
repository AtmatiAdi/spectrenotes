//! Ramkowanie rekordow: `u32 dlugosc | u8 typ | payload | u32 crc32(typ+payload)`.
//!
//! CRC na rekord, bo plik append-only przerwany utrata zasilania musi dac sie
//! odczytac do ostatniego **calego** rekordu. Ogon obcinamy przy starcie.

use crate::crc32::crc32;
use crate::varint::Reader;

/// Typ rekordu. Wartosci sa czescia formatu - nie zmieniac.
pub const REC_OP: u8 = 1;
/// Zarezerwowane na payload szyfrowany (poza v1), zeby doszedl bez migracji.
pub const REC_ENCRYPTED: u8 = 2;

/// Gorna granica dlugosci rekordu - chroni przed alokacja na smieciowym naglowku.
pub const MAX_RECORD_LEN: u32 = 64 * 1024 * 1024;

pub fn write_record(kind: u8, payload: &[u8], out: &mut Vec<u8>) {
    let len = 1 + payload.len() as u32;
    out.extend_from_slice(&len.to_le_bytes());
    let start = out.len();
    out.push(kind);
    out.extend_from_slice(payload);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_le_bytes());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordError {
    /// Za malo bajtow na caly rekord - normalne na koncu przerwanego zapisu.
    Truncated,
    /// Rekord ma zla sume - uszkodzony, od tego miejsca plik jest niewiarygodny.
    BadCrc,
    /// Dlugosc poza rozsadkiem - to nie jest naglowek rekordu.
    BadLength,
}

/// Odczytuje jeden rekord od biezacej pozycji. Przy bledzie pozycja czytnika
/// jest niezdefiniowana - wolajacy ma uzyc `pos()` sprzed wywolania.
pub fn read_record<'a>(r: &mut Reader<'a>) -> Result<(u8, &'a [u8]), RecordError> {
    let len = r.u32_le().map_err(|_| RecordError::Truncated)?;
    if len == 0 || len > MAX_RECORD_LEN {
        return Err(RecordError::BadLength);
    }
    let body = r.bytes(len as usize).map_err(|_| RecordError::Truncated)?;
    let crc = r.u32_le().map_err(|_| RecordError::Truncated)?;
    if crc32(body) != crc {
        return Err(RecordError::BadCrc);
    }
    Ok((body[0], &body[1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_i_obciecie() {
        let mut buf = Vec::new();
        write_record(REC_OP, b"hello", &mut buf);
        write_record(REC_OP, b"world", &mut buf);

        let mut r = Reader::new(&buf);
        assert_eq!(read_record(&mut r).unwrap(), (REC_OP, &b"hello"[..]));
        assert_eq!(read_record(&mut r).unwrap(), (REC_OP, &b"world"[..]));
        assert!(r.is_empty());

        // Utrata zasilania w polowie drugiego rekordu.
        let cut = &buf[..buf.len() - 3];
        let mut r = Reader::new(cut);
        assert!(read_record(&mut r).is_ok());
        assert_eq!(read_record(&mut r), Err(RecordError::Truncated));
    }

    #[test]
    fn przeklamany_bajt_to_bad_crc() {
        let mut buf = Vec::new();
        write_record(REC_OP, b"hello", &mut buf);
        buf[6] ^= 0x01;
        let mut r = Reader::new(&buf);
        assert_eq!(read_record(&mut r), Err(RecordError::BadCrc));
    }
}
