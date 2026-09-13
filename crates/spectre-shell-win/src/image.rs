//! Dekodowanie obrazkow (PNG/JPEG/GIF/WebP...) przez systemowy WIC - do avataru
//! z GitHuba. Zero zaleznosci: koder jest w Windows. Wynik to BGRA
//! premultiplied, gotowy dla bitmapy Direct2D. Wolac z watku sync
//! (inicjalizuje COM na biezacym watku, jesli trzeba).

use windows::core::Result;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

/// Zdekodowany obrazek: szerokosc, wysokosc, piksele BGRA premultiplied.
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

/// Dekoduje `bytes`; obrazki wieksze niz `max_side` px sa zmniejszane
/// (filtr Fant), zeby nie trzymac megapikseli dla kolka 44 px.
pub fn decode(bytes: &[u8], max_side: u32) -> Result<Image> {
    unsafe {
        // S_FALSE (juz zainicjowany) i RPC_E_CHANGED_MODE (inny model) nie
        // przeszkadzaja - COM na tym watku dziala.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = factory.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let decoder = factory.CreateDecoderFromStream(
            &stream,
            std::ptr::null(),
            WICDecodeMetadataCacheOnDemand,
        )?;
        let frame = decoder.GetFrame(0)?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h)?;
        if w == 0 || h == 0 {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_FAIL,
            ));
        }
        let mut source: IWICBitmapSource = frame.into();
        let side = w.max(h);
        if side > max_side {
            let scale = max_side as f32 / side as f32;
            w = ((w as f32 * scale).round() as u32).max(1);
            h = ((h as f32 * scale).round() as u32).max(1);
            let scaler = factory.CreateBitmapScaler()?;
            scaler.Initialize(&source, w, h, WICBitmapInterpolationModeFant)?;
            source = scaler.into();
        }
        let conv = factory.CreateFormatConverter()?;
        conv.Initialize(
            &source,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        let stride = w * 4;
        let mut bgra = vec![0u8; (stride * h) as usize];
        conv.CopyPixels(std::ptr::null(), stride, &mut bgra)?;
        Ok(Image { w, h, bgra })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2x2 PNG (czerwony, zielony / niebieski, bialy), 8-bit RGBA.
    const PNG_2X2: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72,
        0xB6, 0x0D, 0x24, 0x00, 0x00, 0x00, 0x01, 0x73, 0x52, 0x47, 0x42, 0x00, 0xAE, 0xCE, 0x1C,
        0xE9, 0x00, 0x00, 0x00, 0x04, 0x67, 0x41, 0x4D, 0x41, 0x00, 0x00, 0xB1, 0x8F, 0x0B, 0xFC,
        0x61, 0x05, 0x00, 0x00, 0x00, 0x09, 0x70, 0x48, 0x59, 0x73, 0x00, 0x00, 0x0E, 0xC3, 0x00,
        0x00, 0x0E, 0xC3, 0x01, 0xC7, 0x6F, 0xA8, 0x64, 0x00, 0x00, 0x00, 0x13, 0x49, 0x44, 0x41,
        0x54, 0x18, 0x57, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0x0C, 0x19, 0x18, 0xFE, 0x83, 0x01,
        0x00, 0x49, 0xC8, 0x09, 0xF7, 0x96, 0xDE, 0x4D, 0x2E, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
        0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn png_do_bgra() {
        let img = decode(PNG_2X2, 64).unwrap();
        assert_eq!((img.w, img.h), (2, 2));
        assert_eq!(img.bgra.len(), 16);
        // Pierwszy piksel czerwony: B=0 G=0 R=255 A=255.
        assert_eq!(&img.bgra[0..4], &[0, 0, 255, 255]);
        // Ostatni bialy.
        assert_eq!(&img.bgra[12..16], &[255, 255, 255, 255]);
    }

    #[test]
    fn zmniejszanie() {
        let img = decode(PNG_2X2, 1).unwrap();
        assert_eq!((img.w, img.h), (1, 1));
        assert_eq!(img.bgra.len(), 4);
    }

    #[test]
    fn smieci_to_blad() {
        assert!(decode(b"not an image", 64).is_err());
    }
}
