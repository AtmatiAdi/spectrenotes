//! Zrzut okna do PNG (zalacznik do feedbacku). GDI kopiuje piksele z ekranu
//! w prostokacie klienta okna - to, co uzytkownik widzi, lacznie z tym, co
//! ewentualnie lezy na wierzchu - po `DwmFlush`, zeby ostatnia zaprezentowana
//! klatka byla juz zlozona przez DWM. PNG koduje systemowy WIC (zero
//! zaleznosci, jak dekodowanie avataru w `image`).

use std::ffi::c_void;
use std::mem::size_of;

use windows::core::Result;
use windows::Win32::Foundation::{E_FAIL, HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{
    BitBlt, ClientToScreen, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, SRCCOPY,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA,
    IWICImagingFactory, WICBitmapEncoderNoCache,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET,
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

/// Obszar klienta okna jako PNG (BGRA bez alfy). Wolac z watku okna, po
/// narysowaniu klatki, ktora ma trafic na zrzut.
pub fn window_png(hwnd: HWND) -> Result<Vec<u8>> {
    let (w, h, bgra) = grab(hwnd)?;
    encode_png(w, h, &bgra)
}

fn grab(hwnd: HWND) -> Result<(u32, u32, Vec<u8>)> {
    unsafe {
        let _ = DwmFlush();
        let mut rc = RECT::default();
        GetClientRect(hwnd, &mut rc)?;
        let mut origin = POINT {
            x: rc.left,
            y: rc.top,
        };
        let _ = ClientToScreen(hwnd, &mut origin);
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        if w <= 0 || h <= 0 {
            return Err(windows::core::Error::from_hresult(E_FAIL));
        }
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // Ujemna wysokosc = wiersze od gory, jak w PNG.
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let dib = match CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(d) => d,
            Err(e) => {
                let _ = DeleteDC(mem);
                ReleaseDC(None, screen);
                return Err(e);
            }
        };
        let old = SelectObject(mem, dib.into());
        let blit = BitBlt(mem, 0, 0, w, h, Some(screen), origin.x, origin.y, SRCCOPY);
        let n = (w as usize) * (h as usize) * 4;
        let mut out = vec![0u8; n];
        if blit.is_ok() && !bits.is_null() {
            std::ptr::copy_nonoverlapping(bits as *const u8, out.as_mut_ptr(), n);
        }
        SelectObject(mem, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        blit?;
        // GDI zostawia w kanale alfa smieci (zwykle 0) - zrzut ma byc kryjacy.
        for px in out.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Ok((w as u32, h as u32, out))
    }
}

/// Koduje BGRA (wiersze od gory, `w*4` bajtow kazdy) do PNG.
pub fn encode_png(w: u32, h: u32, bgra: &[u8]) -> Result<Vec<u8>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream =
            SHCreateMemStream(None).ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
        let encoder = factory.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        let mut props = None;
        encoder.CreateNewFrame(&mut frame, &mut props)?;
        let frame = frame.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
        frame.Initialize(props.as_ref())?;
        frame.SetSize(w, h)?;
        let mut fmt = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut fmt)?;
        frame.WritePixels(h, w * 4, bgra)?;
        frame.Commit()?;
        encoder.Commit()?;
        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME)?;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let size = stat.cbSize as usize;
        let mut out = vec![0u8; size];
        let mut read = 0u32;
        stream
            .Read(out.as_mut_ptr() as *mut c_void, size as u32, Some(&mut read))
            .ok()?;
        out.truncate(read as usize);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_w_obie_strony() {
        // 2x1: czerwony, niebieski.
        let bgra = [0u8, 0, 255, 255, 255, 0, 0, 255];
        let png = encode_png(2, 1, &bgra).unwrap();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let back = crate::image::decode(&png, 64).unwrap();
        assert_eq!((back.w, back.h), (2, 1));
        assert_eq!(&back.bgra[..], &bgra[..]);
    }
}
