//! Strony PDF jako bitmapy - przez renderer wbudowany w Windows
//! (`Windows.Data.Pdf`, ten sam co w Edge). Bez zaleznosci i bez wlasnego
//! interpretera PDF: rozmiary stron i piksele daje system.
//!
//! Obiekty WinRT zyja w watku, ktory je utworzyl (watek renderu w aplikacji),
//! a wywolania asynchroniczne sa tu czekane synchronicznie (`join`) - to
//! watek roboczy, nie watek okna.

use std::path::Path;

use windows::core::{Result, GUID, HSTRING};
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Foundation::Rect;
use windows::Storage::StorageFile;
use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};

/// `BitmapEncoder.BmpEncoderId`: nieskompresowany BMP - dekodowanie to
/// przepisanie wierszy, bez inflate jak przy PNG (domyslnym).
const BMP_ENCODER: GUID = GUID::from_u128(0x69be8bb4_d66d_47c8_865a_ed1589433782);

/// Rozmiar strony tak, jak sie ja oglada (po obrocie `/Rotate` i przycieciu
/// do CropBox), w DIP-ach (1/96 cala).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSize {
    pub w: f32,
    pub h: f32,
}

/// Piksele BGRA z przezroczystoscia premultiplied (renderer daje nieprzezroczyste).
pub struct Pixels {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

pub struct PdfFile {
    doc: PdfDocument,
}

impl PdfFile {
    pub fn open(path: &Path) -> Result<Self> {
        // WinRT wymaga zainicjowanego COM w watku. Watek renderu dostaje MTA;
        // w watku okna (STA) wywolanie odpada z RPC_E_CHANGED_MODE - to
        // w porzadku, apartament juz jest.
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))?.join()?;
        let doc = PdfDocument::LoadFromFileAsync(&file)?.join()?;
        Ok(Self { doc })
    }

    /// Zaszyfrowany (haslo do otwarcia) - `open` i tak by go nie otworzyl,
    /// ale haslo tylko do edycji otwiera, a zapisac sie nie da.
    pub fn is_protected(&self) -> bool {
        self.doc.IsPasswordProtected().unwrap_or(false)
    }

    pub fn pages(&self) -> Result<Vec<PageSize>> {
        let n = self.doc.PageCount()?;
        (0..n)
            .map(|i| {
                let s = self.doc.GetPage(i)?.Size()?;
                Ok(PageSize {
                    w: s.Width,
                    h: s.Height,
                })
            })
            .collect()
    }

    /// Wycinek strony `src` (w DIP-ach strony, `None` = cala) w szerokosci
    /// `dest_w` pikseli; wysokosc wynika z proporcji wycinka.
    pub fn render(&self, page: u32, src: Option<Rect>, dest_w: u32) -> Result<Pixels> {
        let page = self.doc.GetPage(page)?;
        let opts = PdfPageRenderOptions::new()?;
        if let Some(r) = src {
            opts.SetSourceRect(r)?;
        }
        opts.SetDestinationWidth(dest_w.max(1))?;
        opts.SetBitmapEncoderId(BMP_ENCODER)?;
        let stream = InMemoryRandomAccessStream::new()?;
        page.RenderWithOptionsToStreamAsync(&stream, &opts)?
            .join()?;
        let size = stream.Size()? as u32;
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        reader.LoadAsync(size)?.join()?;
        let mut bmp = vec![0u8; size as usize];
        reader.ReadBytes(&mut bmp)?;
        decode_bmp(&bmp)
            .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
    }
}

/// `BitmapEncoder.JpegEncoderId`.
const JPEG_ENCODER: GUID = GUID::from_u128(0x1a34f5c1_4a5a_46dc_b644_1f4567e7a676);

/// Strona jako JPEG (do PDF-a zastepczego, gdy oryginalu nie da sie
/// dopisac): bajty i rozmiar w pikselach odczytany z naglowka SOF.
pub struct Jpeg {
    pub w: u32,
    pub h: u32,
    pub bytes: Vec<u8>,
}

impl PdfFile {
    pub fn render_jpeg(&self, page: u32, dest_w: u32) -> Result<Jpeg> {
        let page = self.doc.GetPage(page)?;
        let opts = PdfPageRenderOptions::new()?;
        opts.SetDestinationWidth(dest_w.max(1))?;
        opts.SetBitmapEncoderId(JPEG_ENCODER)?;
        let stream = InMemoryRandomAccessStream::new()?;
        page.RenderWithOptionsToStreamAsync(&stream, &opts)?
            .join()?;
        let size = stream.Size()? as u32;
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        reader.LoadAsync(size)?.join()?;
        let mut bytes = vec![0u8; size as usize];
        reader.ReadBytes(&mut bytes)?;
        let (w, h) = jpeg_size(&bytes)
            .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
        Ok(Jpeg { w, h, bytes })
    }
}

/// Wymiary z pierwszego znacznika SOF (0xFFC0..0xFFCF bez C4, C8, CC).
fn jpeg_size(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    while i + 9 < b.len() {
        if b[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = b[i + 1];
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if (0xC0..=0xCF).contains(&m) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32;
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32;
            return Some((w, h));
        }
        i += 2 + len;
    }
    None
}

/// BMP 32 bpp od kodera WIC: naglowek pliku (14 B), DIB (40 lub 124 B),
/// wiersze od dolu (wysokosc dodatnia) albo od gory (ujemna).
fn decode_bmp(b: &[u8]) -> Option<Pixels> {
    let u32_at = |o: usize| {
        b.get(o..o + 4)
            .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    };
    if b.get(0..2)? != b"BM" {
        return None;
    }
    let off = u32_at(10)? as usize;
    let w = u32_at(18)? as i32;
    let h = u32_at(22)? as i32;
    let bpp = u16::from_le_bytes(b.get(28..30)?.try_into().ok()?);
    if w <= 0 || h == 0 || !(bpp == 32 || bpp == 24) {
        return None;
    }
    let (w, bottom_up) = (w as usize, h > 0);
    let h = h.unsigned_abs() as usize;
    let src_px = bpp as usize / 8;
    let stride = (w * src_px).div_ceil(4) * 4;
    b.get(off..off + stride * h)?;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let sy = if bottom_up { h - 1 - y } else { y };
        let row = &b[off + sy * stride..off + sy * stride + w * src_px];
        let dst = &mut out[y * w * 4..(y + 1) * w * 4];
        if src_px == 4 {
            dst.copy_from_slice(row);
            // Renderer daje strone nieprzezroczysta; kanal alfa bywa zerem.
            for px in dst.chunks_exact_mut(4) {
                px[3] = 255;
            }
        } else {
            for (d, s) in dst.chunks_exact_mut(4).zip(row.chunks_exact(3)) {
                d[..3].copy_from_slice(s);
                d[3] = 255;
            }
        }
    }
    Some(Pixels {
        w: w as u32,
        h: h as u32,
        bgra: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pomiar na prawdziwym pliku: `SPECTRENOTES_PDF=sciezka cargo test ...
    /// czas_renderu_strony -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn czas_renderu_strony() {
        let path = std::env::var("SPECTRENOTES_PDF").expect("SPECTRENOTES_PDF");
        let t = std::time::Instant::now();
        let f = PdfFile::open(Path::new(&path)).unwrap();
        let pages = f.pages().unwrap();
        println!(
            "otwarcie: {:.1} ms, stron {}",
            t.elapsed().as_secs_f64() * 1e3,
            pages.len()
        );
        println!("strona 0: {:?} DIP", pages[0]);
        for &w in &[720u32, 1440, 2880] {
            for i in 0..pages.len().min(3) as u32 {
                let t = std::time::Instant::now();
                let px = f.render(i, None, w).unwrap();
                println!(
                    "strona {i} w {w} px -> {}x{}: {:.1} ms",
                    px.w,
                    px.h,
                    t.elapsed().as_secs_f64() * 1e3
                );
            }
        }
        // Wycinek: gorna cwiartka strony w pelnej szerokosci panelu.
        let p = pages[0];
        let t = std::time::Instant::now();
        let px = f
            .render(
                0,
                Some(Rect {
                    X: 0.0,
                    Y: 0.0,
                    Width: p.w,
                    Height: p.h / 4.0,
                }),
                2880,
            )
            .unwrap();
        println!(
            "wycinek 1/4 w 2880 px -> {}x{}: {:.1} ms",
            px.w,
            px.h,
            t.elapsed().as_secs_f64() * 1e3
        );
    }

    /// Podglad stron pliku tak, jak widzi je Windows (i Edge):
    /// `SPECTRENOTES_PDF=plik SPECTRENOTES_PDF_SHOT=prefiks cargo test ...
    /// zrzut_stron -- --ignored`. Zapisuje `prefiks-N.bmp` (szerokosc 1200 px).
    #[test]
    #[ignore]
    fn zrzut_stron() {
        let path = std::env::var("SPECTRENOTES_PDF").expect("SPECTRENOTES_PDF");
        let prefix = std::env::var("SPECTRENOTES_PDF_SHOT").expect("SPECTRENOTES_PDF_SHOT");
        let f = PdfFile::open(Path::new(&path)).unwrap();
        let pages = f.pages().unwrap();
        for (i, p) in pages.iter().enumerate() {
            let px = f.render(i as u32, None, 1200).unwrap();
            let mut bmp = Vec::new();
            let size = 54 + px.bgra.len() as u32;
            bmp.extend_from_slice(b"BM");
            bmp.extend_from_slice(&size.to_le_bytes());
            bmp.extend_from_slice(&[0; 4]);
            bmp.extend_from_slice(&54u32.to_le_bytes());
            bmp.extend_from_slice(&40u32.to_le_bytes());
            bmp.extend_from_slice(&(px.w as i32).to_le_bytes());
            bmp.extend_from_slice(&(-(px.h as i32)).to_le_bytes());
            bmp.extend_from_slice(&1u16.to_le_bytes());
            bmp.extend_from_slice(&32u16.to_le_bytes());
            bmp.extend_from_slice(&[0; 24]);
            bmp.extend_from_slice(&px.bgra);
            std::fs::write(format!("{prefix}-{i}.bmp"), bmp).unwrap();
            println!("strona {i}: {:?} DIP -> {}x{}", p, px.w, px.h);
        }
    }

    #[test]
    fn bmp_od_dolu_i_od_gory() {
        // 2x2, 32 bpp, od dolu: wiersz 0 w pliku to dolny wiersz obrazu.
        let mut b = Vec::new();
        b.extend_from_slice(b"BM");
        b.extend_from_slice(&[0; 8]);
        b.extend_from_slice(&54u32.to_le_bytes());
        b.extend_from_slice(&40u32.to_le_bytes());
        b.extend_from_slice(&2i32.to_le_bytes());
        b.extend_from_slice(&2i32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&32u16.to_le_bytes());
        b.extend_from_slice(&[0; 24]);
        b.extend_from_slice(&[1, 1, 1, 0, 2, 2, 2, 0, 3, 3, 3, 0, 4, 4, 4, 0]);
        let px = decode_bmp(&b).unwrap();
        assert_eq!((px.w, px.h), (2, 2));
        assert_eq!(&px.bgra[0..4], &[3, 3, 3, 255], "gorny wiersz obrazu");
        assert_eq!(&px.bgra[8..12], &[1, 1, 1, 255]);
        // Ta sama tresc od gory (wysokosc ujemna).
        b[22..26].copy_from_slice(&(-2i32).to_le_bytes());
        let px = decode_bmp(&b).unwrap();
        assert_eq!(&px.bgra[0..4], &[1, 1, 1, 255]);
    }
}
