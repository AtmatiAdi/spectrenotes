use std::collections::HashMap;
use std::mem::size_of;

use spectre_core::{Bbox, Camera, Document, StrokeId};
use spectre_ink::{InkConfig, Segment};
use spectre_proto::Rgba;
use windows::core::{Interface, Result, HSTRING};
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
    D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1BitmapBrush1, ID2D1Brush, ID2D1DeviceContext,
    ID2D1DeviceContext1, ID2D1Factory1, ID2D1GeometryRealization, ID2D1StrokeStyle1,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_BRUSH_PROPERTIES1,
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_CAP_STYLE_ROUND, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_INTERPOLATION_MODE_CUBIC,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_LINE_JOIN_ROUND, D2D1_ROUNDED_RECT,
    D2D1_STROKE_STYLE_PROPERTIES1, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, IDXGIAdapter1, IDXGIDevice, IDXGIDevice3, IDXGIFactory6, IDXGISurface,
    IDXGISwapChain1, IDXGISwapChain2, DXGI_ADAPTER_FLAG, DXGI_ADAPTER_FLAG_SOFTWARE,
    DXGI_CREATE_FACTORY_FLAGS, DXGI_FEATURE_PRESENT_ALLOW_TEARING,
    DXGI_GPU_PREFERENCE_MINIMUM_POWER, DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT,
    DXGI_PRESENT_ALLOW_TEARING, DXGI_SCALING_NONE, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows_numerics::{Matrix3x2, Vector2};

use crate::geometry;
use crate::tess::stroke_segments;

/// Czysta czern: na AMOLED piksel jest wtedy fizycznie zgaszony (Z7).
pub const BG_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

/// Jak prezentowac klatke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentMode {
    /// Tearing: najnizsza latencja, dla mokrego atramentu. Rozdarcie klatki przy
    /// cienkiej linii jest niewidoczne.
    Immediate,
    /// Bez czekania na vblank i bez tearingu - DWM bierze najnowsza klatke.
    /// Dla przewijania: rozdarcie byloby widoczne jako uskok tresci, a czekanie
    /// na vblank blokuje watek i kolejkuje komunikaty piora.
    Latest,
    VSync,
}

/// Prymityw UI w pikselach ekranu. Logika UI zyje w aplikacji; renderer tylko rysuje.
#[derive(Debug, Clone)]
pub enum UiPrim {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Rgba,
        /// Promien zaokraglenia rogow.
        r: f32,
    },
    Outline {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Rgba,
        width: f32,
        r: f32,
    },
    Circle {
        x: f32,
        y: f32,
        radius: f32,
        color: Rgba,
    },
    Text {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        text: String,
        color: Rgba,
        font: UiFont,
    },
    /// Avatar uzytkownika (`Renderer::set_avatar`) w kolku o boku `size`;
    /// nic nie rysuje, gdy avataru nie ma.
    Avatar {
        x: f32,
        y: f32,
        size: f32,
    },
    /// Od tego miejsca rysowanie jest przycinane do prostokata - do `Unclip`.
    Clip {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    Unclip,
}

/// Czcionki UI. Rozmiary sa w pikselach logicznych (96 DPI) i mnozone przez
/// skale DPI (`Renderer::set_ui_scale`) - tekst jest rysowany w docelowym
/// rozmiarze, bez skalowania rastra.
struct TextFormats {
    /// Consolas 14 (HUD, wartosci).
    mono: IDWriteTextFormat,
    /// Segoe UI 20, wysrodkowana (glify).
    big: IDWriteTextFormat,
    /// Segoe UI 15, wysrodkowana w obu osiach (przyciski).
    center: IDWriteTextFormat,
    /// Segoe UI 15, do lewej, wysrodkowana w pionie, bez zawijania (listy).
    ui: IDWriteTextFormat,
    /// Segoe UI 16,5, wysrodkowana, bez zawijania (tytul notatki, przyciski okna).
    title: IDWriteTextFormat,
}

impl TextFormats {
    unsafe fn new(dwrite: &IDWriteFactory, scale: f32) -> Result<Self> {
        let make = |family: &str, size: f32| -> Result<IDWriteTextFormat> {
            dwrite.CreateTextFormat(
                &HSTRING::from(family),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size * scale,
                &HSTRING::from("en-us"),
            )
        };
        let mono = make("Consolas", 14.0)?;
        let big = make("Segoe UI", 20.0)?;
        big.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        big.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        let center = make("Segoe UI", 15.0)?;
        center.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        center.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        // Listy i etykiety: do lewej, wysrodkowane w pionie, bez zawijania
        // (za dlugi tytul jest ucinany przez prostokat, nie lamany).
        let ui = make("Segoe UI", 15.0)?;
        ui.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        ui.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        let title = make("Segoe UI", 16.5)?;
        title.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        title.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        title.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        Ok(Self {
            mono,
            big,
            center,
            ui,
            title,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiFont {
    /// Consolas 14, do lewej, od gory (HUD, wartosci).
    Mono,
    /// Segoe UI 15, do lewej, wysrodkowana w pionie, bez zawijania (listy).
    Ui,
    /// Segoe UI 15, wysrodkowana w obu osiach (przyciski).
    Center,
    /// Segoe UI 20, wysrodkowana (glify).
    Big,
    /// Segoe UI 19, wysrodkowana - tytul notatki i przyciski okna w zakladkach.
    Title,
}

/// Czubek cudzej kreski w trakcie rysowania (warstwa live), we wlasnym kolorze.
#[derive(Debug, Clone, Default)]
pub struct WetTail {
    pub segs: Vec<Segment>,
    pub color: Rgba,
}

/// Co narysowac na wierzchu klatki, poza dokumentem.
#[derive(Default, Clone, Copy)]
pub struct Overlay<'a> {
    pub hud: Option<&'a str>,
    /// Okrag gumki: (x, y, promien) w pikselach ekranu.
    pub cursor: Option<(f32, f32, f32)>,
    /// Czubki kresek innych osob (live) - jak `tail`, ale kazdy w swoim kolorze.
    pub tails: &'a [WetTail],
    /// Rysiki innych osob: (x, y) w pikselach ekranu, mala kropka.
    pub marks: &'a [(f32, f32)],
    pub ui: &'a [UiPrim],
    /// Fale lokalnego przyciemnienia (Z7, po bezczynnosci), na samym wierzchu.
    pub dim: Option<&'a DimMask>,
}

/// Maska przyciemnienia (Z7): krycie czerni 0..255 w siatce co `DIM_STEP` px
/// ekranu, rozciagana na okno z interpolacja. Piksel maski `(i, j)` odpowiada
/// pikselowi ekranu `(i * DIM_STEP, j * DIM_STEP)`; siatka wychodzi o pol kroku
/// poza okno, zeby brzeg interpolacji nie byl widoczny.
#[derive(Debug, Clone, PartialEq)]
pub struct DimMask {
    pub w: u32,
    pub h: u32,
    pub alpha: Vec<u8>,
}

/// Krok siatki maski przyciemnienia w pikselach ekranu.
pub const DIM_STEP: u32 = 12;

impl DimMask {
    /// Maska pokrywajaca okno `view_w` x `view_h` (pusta - krycie 0).
    pub fn for_view(view_w: u32, view_h: u32) -> Self {
        let w = view_w.div_ceil(DIM_STEP) + 1;
        let h = view_h.div_ceil(DIM_STEP) + 1;
        Self {
            w,
            h,
            alpha: vec![0; (w * h) as usize],
        }
    }
}

pub struct Renderer {
    adapter_name: String,
    tearing_supported: bool,
    size: (u32, u32),

    device: ID3D11Device,
    swapchain: IDXGISwapChain1,
    factory2d: ID2D1Factory1,
    ctx: ID2D1DeviceContext,
    /// D2D 1.2 - realizacje geometrii (Windows 8.1+).
    ctx1: ID2D1DeviceContext1,

    backbuffer: Option<ID2D1Bitmap1>,
    /// Dwie bitmapy warstwy suchej: biezaca i zapasowa do przewijania przyrostowego.
    dry: [Option<ID2D1Bitmap1>; 2],
    dry_cur: usize,

    brushes: HashMap<u32, ID2D1Brush>,
    hud_fg: ID2D1Brush,
    hud_bg: ID2D1Brush,
    cursor_brush: ID2D1Brush,
    /// Bitmapa maski przyciemnienia (Z7) i jej rozmiar; bufor BGRA do przeslania.
    dim_bmp: Option<(ID2D1Bitmap1, u32, u32)>,
    dim_px: Vec<u8>,
    /// Avatar zalogowanego uzytkownika (naglowek menu): pedzel bitmapowy i rozmiar zrodla.
    avatar: Option<(ID2D1BitmapBrush1, u32, u32)>,
    round: ID2D1StrokeStyle1,
    dwrite: IDWriteFactory,
    /// Czcionki UI w fizycznych pikselach dla `ui_scale` (`set_ui_scale`).
    fonts: TextFormats,
    ui_scale: f32,

    /// Obrys per kreska zrealizowany do mesha (`geometry.rs`). Kreski sa
    /// niezmienne, wiec wpis dezaktualizuje sie tylko przy duzej zmianie zoomu
    /// (tolerancja splaszczania byla liczona dla innej skali).
    geo_cache: HashMap<StrokeId, GeoEntry>,
    geo_cache_verts: usize,
    /// Bufor teselacji wielokrotnego uzytku.
    seg_scratch: Vec<Segment>,
}

struct GeoEntry {
    real: ID2D1GeometryRealization,
    zoom: f32,
    verts: usize,
}

/// Suma wierzcholkow obrysow w cache; powyzej - czyszczenie w calosci.
/// Rzad 100-200 MB GPU przy pelnym budzecie.
const GEO_CACHE_BUDGET: usize = 6_000_000;
/// Zoom w tym zakresie wzgledem zbudowanego nie wymaga przebudowy obrysu.
const GEO_ZOOM_TOL: f32 = 1.5;

impl Renderer {
    pub fn new(hwnd: HWND, width: u32, height: u32) -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory6 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
            // Swiadomy wybor adaptera o najnizszym poborze mocy (docs/04): domyslna
            // heurystyka DXGI potrafi wybudzic dedykowana karte do rysowania notatek.
            let (adapter, adapter_name) = pick_low_power_adapter(&factory)?;

            let mut device: Option<ID3D11Device> = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )?;
            let device = device.expect("D3D11CreateDevice: sukces bez urzadzenia");

            let mut tearing: u32 = 0;
            let tearing_supported = factory
                .CheckFeatureSupport(
                    DXGI_FEATURE_PRESENT_ALLOW_TEARING,
                    &mut tearing as *mut _ as *mut _,
                    size_of::<u32>() as u32,
                )
                .is_ok()
                && tearing != 0;

            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width.max(1),
                Height: height.max(1),
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_NONE,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                Flags: swapchain_flags(tearing_supported).0 as u32,
                ..Default::default()
            };
            let swapchain = factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)?;
            let _ = factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
            // Domyslna kolejka to 3 klatki; przy 120 Hz to 25 ms samego kolejkowania.
            if let Ok(sc2) = swapchain.cast::<IDXGISwapChain2>() {
                let _ = sc2.SetMaximumFrameLatency(1);
            }

            let factory2d: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dxgi_device: IDXGIDevice = device.cast()?;
            let device2d = factory2d.CreateDevice(&dxgi_device)?;
            let ctx = device2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let ctx1: ID2D1DeviceContext1 = ctx.cast()?;
            ctx.SetDpi(96.0, 96.0); // jednostki D2D == piksele fizyczne
            ctx.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            // Skala szarosci, nigdy ClearType - uklad subpikseli OLED daje kolorowe obwodki.
            ctx.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

            let hud_fg = solid(&ctx, 0.45, 0.62, 0.55, 0.9)?;
            let hud_bg = solid(&ctx, 0.0, 0.0, 0.0, 0.55)?;
            let cursor_brush = solid(&ctx, 0.6, 0.6, 0.6, 0.8)?;

            let round = factory2d.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES1 {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 1.0,
                    ..Default::default()
                },
                None,
            )?;

            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let fonts = TextFormats::new(&dwrite, 1.0)?;

            let mut r = Self {
                adapter_name,
                tearing_supported,
                size: (width.max(1), height.max(1)),
                device,
                swapchain,
                factory2d,
                ctx,
                ctx1,
                backbuffer: None,
                dry: [None, None],
                dry_cur: 0,
                brushes: HashMap::new(),
                hud_fg,
                hud_bg,
                cursor_brush,
                dim_bmp: None,
                dim_px: Vec::new(),
                avatar: None,
                round,
                dwrite,
                fonts,
                ui_scale: 1.0,
                geo_cache: HashMap::new(),
                geo_cache_verts: 0,
                seg_scratch: Vec::with_capacity(4096),
            };
            r.create_size_dependent()?;
            Ok(r)
        }
    }

    /// Skala DPI dla UI (1.0 = 96 DPI): czcionki sa tworzone od nowa w
    /// docelowym rozmiarze w pikselach. Tanie i rzadkie (start, zmiana monitora).
    pub fn set_ui_scale(&mut self, scale: f32) {
        if (scale - self.ui_scale).abs() < 1e-3 || scale <= 0.0 {
            return;
        }
        if let Ok(fonts) = unsafe { TextFormats::new(&self.dwrite, scale) } {
            self.fonts = fonts;
            self.ui_scale = scale;
        }
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub fn tearing_supported(&self) -> bool {
        self.tearing_supported
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    fn create_size_dependent(&mut self) -> Result<()> {
        unsafe {
            let surface: IDXGISurface = self.swapchain.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            self.backbuffer = Some(
                self.ctx
                    .CreateBitmapFromDxgiSurface(&surface, Some(&props))?,
            );

            let dry_props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                ..Default::default()
            };
            let size = D2D_SIZE_U {
                width: self.size.0,
                height: self.size.1,
            };
            for slot in &mut self.dry {
                let bmp = self.ctx.CreateBitmap(size, None, 0, &dry_props)?;
                self.ctx.SetTarget(&bmp);
                self.ctx.BeginDraw();
                self.ctx.Clear(Some(&BG_COLOR));
                self.ctx.EndDraw(None, None)?;
                *slot = Some(bmp);
            }
            self.ctx.SetTarget(None);
            Ok(())
        }
    }

    /// Zwraca `true`, gdy rozmiar sie zmienil i warstwa sucha wymaga przebudowy.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<bool> {
        if width == 0 || height == 0 || (width, height) == self.size {
            return Ok(false);
        }
        unsafe {
            self.ctx.SetTarget(None);
            self.backbuffer = None;
            self.dry = [None, None];
            self.swapchain.ResizeBuffers(
                0,
                width,
                height,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                swapchain_flags(self.tearing_supported),
            )?;
        }
        self.size = (width, height);
        self.create_size_dependent()?;
        Ok(true)
    }

    /// Do wywolania przy zmianie dokumentu. `StrokeId` = (autor, licznik) jest
    /// unikalny tylko w obrebie notatki, wiec wpisy z poprzedniej notatki pasowalyby
    /// do id kresek nastepnej i pelna przebudowa rysowalaby cudza geometrie
    /// (objaw: biale poziome pasy z notatki testowej na innych notatkach po F11).
    pub fn clear_geometry(&mut self) {
        self.geo_cache.clear();
        self.geo_cache_verts = 0;
    }

    /// Avatar do `UiPrim::Avatar`: piksele BGRA premultiplied, `w * h * 4` bajtow.
    pub fn set_avatar(&mut self, w: u32, h: u32, bgra: &[u8]) -> Result<()> {
        if w == 0 || h == 0 || bgra.len() != (w * h * 4) as usize {
            self.avatar = None;
            return Ok(());
        }
        unsafe {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                ..Default::default()
            };
            let size = D2D_SIZE_U {
                width: w,
                height: h,
            };
            let bmp = self.ctx.CreateBitmap(size, None, 0, &props)?;
            bmp.CopyFromMemory(None, bgra.as_ptr() as *const _, w * 4)?;
            let bprops = D2D1_BITMAP_BRUSH_PROPERTIES1 {
                extendModeX: D2D1_EXTEND_MODE_CLAMP,
                extendModeY: D2D1_EXTEND_MODE_CLAMP,
                interpolationMode: D2D1_INTERPOLATION_MODE_LINEAR,
            };
            let brush = self.ctx.CreateBitmapBrush(&bmp, Some(&bprops), None)?;
            self.avatar = Some((brush, w, h));
        }
        Ok(())
    }

    pub fn clear_avatar(&mut self) {
        self.avatar = None;
    }

    pub fn has_avatar(&self) -> bool {
        self.avatar.is_some()
    }

    /// Okno schowane: oddajemy pamiec sterownika (Z2). Bitmapy zostaja, ale
    /// sterownik moze zwolnic swoje bufory posrednie.
    pub fn trim(&self) {
        if let Ok(d3) = self.device.cast::<IDXGIDevice3>() {
            unsafe { d3.Trim() };
        }
    }

    fn brush(&mut self, color: Rgba) -> Result<ID2D1Brush> {
        let key = u32::from_le_bytes([color.r, color.g, color.b, color.a]);
        if let Some(b) = self.brushes.get(&key) {
            return Ok(b.clone());
        }
        let b = unsafe {
            solid(
                &self.ctx,
                color.r as f32 / 255.0,
                color.g as f32 / 255.0,
                color.b as f32 / 255.0,
                color.a as f32 / 255.0,
            )?
        };
        self.brushes.insert(key, b.clone());
        Ok(b)
    }

    unsafe fn draw_segments(&self, segs: &[Segment], brush: &ID2D1Brush, cam: &Camera) {
        for s in segs {
            let (ax, ay) = cam.to_screen(s.a.x, s.a.y);
            let (bx, by) = cam.to_screen(s.b.x, s.b.y);
            self.ctx.DrawLine(
                Vector2 { X: ax, Y: ay },
                Vector2 { X: bx, Y: by },
                brush,
                (s.width * cam.zoom).max(0.4),
                &self.round,
            );
        }
    }

    /// Obrys kreski z cache albo zbudowany teraz. `verts` to koszt pamieciowy.
    unsafe fn stroke_realization(
        &mut self,
        id: StrokeId,
        data: &spectre_proto::StrokeData,
        ink: &InkConfig,
        zoom: f32,
    ) -> Result<ID2D1GeometryRealization> {
        if let Some(e) = self.geo_cache.get(&id) {
            let ratio = zoom / e.zoom;
            if ratio < GEO_ZOOM_TOL && ratio > 1.0 / GEO_ZOOM_TOL {
                return Ok(e.real.clone());
            }
            self.geo_cache_verts -= e.verts;
            self.geo_cache.remove(&id);
        }
        if self.geo_cache_verts > GEO_CACHE_BUDGET {
            self.geo_cache.clear();
            self.geo_cache_verts = 0;
        }
        let mut segs = std::mem::take(&mut self.seg_scratch);
        segs.clear();
        stroke_segments(data, ink, &mut segs);
        let geo = geometry::build(&self.factory2d, &segs, zoom)?;
        let verts = segs.len() * 2 + 32;
        self.seg_scratch = segs;
        // Tolerancja splaszczania w jednostkach canvasu: 1/4 piksela ekranu.
        let real = self
            .ctx1
            .CreateFilledGeometryRealization(&geo, 0.25 / zoom)?;
        self.geo_cache.insert(
            id,
            GeoEntry {
                real: real.clone(),
                zoom,
                verts,
            },
        );
        self.geo_cache_verts += verts;
        Ok(real)
    }

    /// Rysuje kreski dokumentu przecinajace `rect` (canvas) na biezacy target.
    /// Kazda kreska to jedno `DrawGeometryRealization` w transformacji kamery.
    unsafe fn draw_document(
        &mut self,
        doc: &Document,
        cam: &Camera,
        ink: &InkConfig,
        rect: Bbox,
    ) -> Result<()> {
        self.ctx.SetTransform(&Matrix3x2 {
            M11: cam.zoom,
            M12: 0.0,
            M21: 0.0,
            M22: cam.zoom,
            M31: cam.shift.0 - cam.scroll_x * cam.zoom,
            M32: cam.shift.1 - cam.scroll_y * cam.zoom,
        });
        let mut res = Ok(());
        for (id, data, _) in doc.visible_in(rect) {
            let real = match self.stroke_realization(id, data, ink, cam.zoom) {
                Ok(r) => r,
                Err(e) => {
                    res = Err(e);
                    break;
                }
            };
            let brush = match self.brush(data.color) {
                Ok(b) => b,
                Err(e) => {
                    res = Err(e);
                    break;
                }
            };
            self.ctx1.DrawGeometryRealization(&real, &brush);
        }
        self.ctx.SetTransform(&Matrix3x2::identity());
        res
    }

    /// Przebudowa fragmentu warstwy suchej (canvas). Po wymazaniu: czyscimy
    /// prostokat i rysujemy w nim tylko kreski, ktore go przecinaja - zamiast
    /// pelnej przebudowy, ktora przy zapisanej stronie kosztuje dziesiatki ms.
    pub fn repaint(
        &mut self,
        doc: &Document,
        cam: &Camera,
        ink: &InkConfig,
        rect: Bbox,
    ) -> Result<()> {
        let Some(dry) = self.dry[self.dry_cur].clone() else {
            return Ok(());
        };
        let (w, h) = self.size;
        let view = cam.visible(w as f32, h as f32);
        if !rect.intersects(&view) {
            return Ok(());
        }
        let (sx0, sy0) = cam.to_screen(rect.min_x, rect.min_y);
        let (sx1, sy1) = cam.to_screen(rect.max_x, rect.max_y);
        let clip = D2D_RECT_F {
            left: sx0.floor().max(0.0),
            top: sy0.floor().max(0.0),
            right: sx1.ceil().min(w as f32),
            bottom: sy1.ceil().min(h as f32),
        };
        // Prostokat w canvasie odpowiadajacy klipowi zaokraglonemu do pikseli.
        let (cx0, cy0) = cam.to_canvas(clip.left, clip.top);
        let (cx1, cy1) = cam.to_canvas(clip.right, clip.bottom);
        let region = Bbox {
            min_x: cx0,
            min_y: cy0,
            max_x: cx1,
            max_y: cy1,
        };
        unsafe {
            self.ctx.SetTarget(&dry);
            self.ctx.BeginDraw();
            self.ctx
                .PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.ctx.Clear(Some(&BG_COLOR));
            let res = self.draw_document(doc, cam, ink, region);
            self.ctx.PopAxisAlignedClip();
            self.ctx.EndDraw(None, None)?;
            self.ctx.SetTarget(None);
            res?;
        }
        Ok(())
    }

    /// Pelna przebudowa warstwy suchej.
    pub fn rebuild(&mut self, doc: &Document, cam: &Camera, ink: &InkConfig) -> Result<()> {
        let Some(dry) = self.dry[self.dry_cur].clone() else {
            return Ok(());
        };
        let (w, h) = self.size;
        unsafe {
            self.ctx.SetTarget(&dry);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&BG_COLOR));
            self.draw_document(doc, cam, ink, cam.visible(w as f32, h as f32))?;
            self.ctx.EndDraw(None, None)?;
            self.ctx.SetTarget(None);
        }
        Ok(())
    }

    /// Przewiniecie przyrostowe: przesuwa istniejaca warstwe sucha i dorysowuje
    /// tylko odsloniety pas. `old_scroll` to `scroll_y` sprzed zmiany.
    pub fn scroll(
        &mut self,
        doc: &Document,
        cam: &Camera,
        old_scroll: f32,
        ink: &InkConfig,
    ) -> Result<()> {
        let (w, h) = self.size;
        let exact = (cam.scroll_y - old_scroll) * cam.zoom;
        let dy = exact.round();
        // Kamera kwantuje przewiniecie do pikseli (Camera::scroll_to), wiec
        // przesuniecie jest calkowite. Gdyby nie bylo - przebudowa zamiast
        // dryfu, ktory kumulowalby sie z kazdym krokiem.
        if (exact - dy).abs() > 1e-3 {
            return self.rebuild(doc, cam, ink);
        }
        if dy == 0.0 {
            return Ok(());
        }
        if dy.abs() >= h as f32 {
            return self.rebuild(doc, cam, ink);
        }
        let src = self.dry[self.dry_cur].clone();
        let dst = self.dry[1 - self.dry_cur].clone();
        let (Some(src), Some(dst)) = (src, dst) else {
            return Ok(());
        };
        // Tresc idzie w gore, gdy scroll rosnie.
        let band = if dy > 0.0 {
            (h as f32 - dy, h as f32)
        } else {
            (0.0, -dy)
        };
        unsafe {
            self.ctx.SetTarget(&dst);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&BG_COLOR));
            let off = Vector2 { X: 0.0, Y: -dy };
            self.ctx.DrawImage(
                &src,
                Some(&off),
                None,
                Default::default(),
                Default::default(),
            );
            let clip = D2D_RECT_F {
                left: 0.0,
                top: band.0,
                right: w as f32,
                bottom: band.1,
            };
            self.ctx
                .PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let (_, y0) = cam.to_canvas(0.0, band.0);
            let (_, y1) = cam.to_canvas(0.0, band.1);
            let (x0, _) = cam.to_canvas(0.0, 0.0);
            let (x1, _) = cam.to_canvas(w as f32, 0.0);
            let rect = Bbox {
                min_x: x0,
                min_y: y0,
                max_x: x1,
                max_y: y1,
            };
            let res = self.draw_document(doc, cam, ink, rect);
            self.ctx.PopAxisAlignedClip();
            self.ctx.EndDraw(None, None)?;
            self.ctx.SetTarget(None);
            res?;
        }
        self.dry_cur = 1 - self.dry_cur;
        Ok(())
    }

    /// Wypala odcinki biezacej kreski do warstwy suchej.
    pub fn commit(&mut self, segs: &[Segment], color: Rgba, cam: &Camera) -> Result<()> {
        if segs.is_empty() {
            return Ok(());
        }
        let Some(dry) = self.dry[self.dry_cur].clone() else {
            return Ok(());
        };
        let brush = self.brush(color)?;
        unsafe {
            self.ctx.SetTarget(&dry);
            self.ctx.BeginDraw();
            self.draw_segments(segs, &brush, cam);
            self.ctx.EndDraw(None, None)?;
            self.ctx.SetTarget(None);
        }
        Ok(())
    }

    /// Sklada klatke: warstwa sucha + czubek kreski + overlay, i prezentuje.
    pub fn present(
        &mut self,
        tail: &[Segment],
        color: Rgba,
        cam: &Camera,
        overlay: Overlay,
        mode: PresentMode,
    ) -> Result<()> {
        let (Some(back), Some(dry)) = (self.backbuffer.clone(), self.dry[self.dry_cur].clone())
        else {
            return Ok(());
        };
        let brush = self.brush(color)?;
        let mut tail_brushes = Vec::with_capacity(overlay.tails.len());
        for t in overlay.tails {
            tail_brushes.push(self.brush(t.color)?);
        }
        unsafe {
            self.ctx.SetTarget(&back);
            self.ctx.BeginDraw();
            self.ctx
                .DrawImage(&dry, None, None, Default::default(), Default::default());
            for (t, b) in overlay.tails.iter().zip(&tail_brushes) {
                self.draw_segments(&t.segs, b, cam);
            }
            self.draw_segments(tail, &brush, cam);
            for &(x, y) in overlay.marks {
                let e = D2D1_ELLIPSE {
                    point: Vector2 { X: x, Y: y },
                    radiusX: 4.0,
                    radiusY: 4.0,
                };
                self.ctx.FillEllipse(&e, &self.cursor_brush);
            }
            if let Some((x, y, r)) = overlay.cursor {
                let e = D2D1_ELLIPSE {
                    point: Vector2 { X: x, Y: y },
                    radiusX: r,
                    radiusY: r,
                };
                self.ctx.DrawEllipse(&e, &self.cursor_brush, 1.0, None);
            }
            self.draw_ui(overlay.ui)?;
            if let Some(text) = overlay.hud {
                self.draw_hud(text);
            }
            // Fale przyciemnienia (Z7) - na wszystkim, lacznie z UI.
            if let Some(m) = overlay.dim {
                self.draw_dim(m)?;
            }
            self.ctx.EndDraw(None, None)?;
            self.ctx.SetTarget(None);

            let (interval, flags) = match mode {
                PresentMode::VSync => (1u32, DXGI_PRESENT(0)),
                PresentMode::Immediate if self.tearing_supported => {
                    (0u32, DXGI_PRESENT_ALLOW_TEARING)
                }
                PresentMode::Immediate | PresentMode::Latest => (0u32, DXGI_PRESENT(0)),
            };
            self.swapchain.Present(interval, flags).ok()?;
        }
        Ok(())
    }

    /// Maska przyciemnienia: przeslanie do bitmapy (BGRA premultiplied, czern
    /// z kryciem z maski) i rozciagniecie na okno z interpolacja kubiczna.
    unsafe fn draw_dim(&mut self, m: &DimMask) -> Result<()> {
        if m.w == 0 || m.h == 0 || m.alpha.len() != (m.w * m.h) as usize {
            return Ok(());
        }
        let need_new = !matches!(&self.dim_bmp, Some((_, w, h)) if *w == m.w && *h == m.h);
        if need_new {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                ..Default::default()
            };
            let size = D2D_SIZE_U {
                width: m.w,
                height: m.h,
            };
            let bmp = self.ctx.CreateBitmap(size, None, 0, &props)?;
            self.dim_bmp = Some((bmp, m.w, m.h));
        }
        let Some((bmp, _, _)) = &self.dim_bmp else {
            return Ok(());
        };
        self.dim_px.clear();
        self.dim_px.reserve(m.alpha.len() * 4);
        for &a in &m.alpha {
            self.dim_px.extend_from_slice(&[0, 0, 0, a]);
        }
        bmp.CopyFromMemory(None, self.dim_px.as_ptr() as *const _, m.w * 4)?;
        // Srodek piksela maski (i, j) laduje w (i * DIM_STEP, j * DIM_STEP) ekranu.
        let step = DIM_STEP as f32;
        let dest = D2D_RECT_F {
            left: -0.5 * step,
            top: -0.5 * step,
            right: (m.w as f32 - 0.5) * step,
            bottom: (m.h as f32 - 0.5) * step,
        };
        self.ctx.DrawBitmap(
            bmp,
            Some(&dest),
            1.0,
            D2D1_INTERPOLATION_MODE_CUBIC,
            None,
            None,
        );
        Ok(())
    }

    /// Prymitywy UI sa juz w fizycznych pikselach (uklad liczy je ze skali DPI);
    /// zadnej transformacji - kazdy prostokat laduje tam, gdzie go policzono.
    unsafe fn draw_ui(&mut self, prims: &[UiPrim]) -> Result<()> {
        let mut depth = 0u32;
        for p in prims {
            match p {
                UiPrim::Rect {
                    x,
                    y,
                    w,
                    h,
                    color,
                    r,
                } => {
                    let brush = self.brush(*color)?;
                    let rr = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: *x,
                            top: *y,
                            right: x + w,
                            bottom: y + h,
                        },
                        radiusX: *r,
                        radiusY: *r,
                    };
                    self.ctx.FillRoundedRectangle(&rr, &brush);
                }
                UiPrim::Outline {
                    x,
                    y,
                    w,
                    h,
                    color,
                    width,
                    r,
                } => {
                    let brush = self.brush(*color)?;
                    let rr = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: *x,
                            top: *y,
                            right: x + w,
                            bottom: y + h,
                        },
                        radiusX: *r,
                        radiusY: *r,
                    };
                    self.ctx.DrawRoundedRectangle(&rr, &brush, *width, None);
                }
                UiPrim::Circle {
                    x,
                    y,
                    radius,
                    color,
                } => {
                    let brush = self.brush(*color)?;
                    let e = D2D1_ELLIPSE {
                        point: Vector2 { X: *x, Y: *y },
                        radiusX: *radius,
                        radiusY: *radius,
                    };
                    self.ctx.FillEllipse(&e, &brush);
                }
                UiPrim::Text {
                    x,
                    y,
                    w,
                    h,
                    text,
                    color,
                    font,
                } => {
                    let brush = self.brush(*color)?;
                    let fmt = match font {
                        UiFont::Mono => &self.fonts.mono,
                        UiFont::Ui => &self.fonts.ui,
                        UiFont::Center => &self.fonts.center,
                        UiFont::Big => &self.fonts.big,
                        UiFont::Title => &self.fonts.title,
                    };
                    let wide: Vec<u16> = text.encode_utf16().collect();
                    let rect = D2D_RECT_F {
                        left: *x,
                        top: *y,
                        right: x + w,
                        bottom: y + h,
                    };
                    self.ctx.DrawText(
                        &wide,
                        fmt,
                        &rect,
                        &brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
                UiPrim::Avatar { x, y, size } => {
                    if let Some((brush, bw, bh)) = &self.avatar {
                        // Pedzel bitmapowy przeskalowany do kolka i wypelnienie elipsy.
                        let sx = size / *bw as f32;
                        let sy = size / *bh as f32;
                        brush.SetTransform(&Matrix3x2 {
                            M11: sx,
                            M12: 0.0,
                            M21: 0.0,
                            M22: sy,
                            M31: *x,
                            M32: *y,
                        });
                        let e = D2D1_ELLIPSE {
                            point: Vector2 {
                                X: x + size * 0.5,
                                Y: y + size * 0.5,
                            },
                            radiusX: size * 0.5,
                            radiusY: size * 0.5,
                        };
                        self.ctx.FillEllipse(&e, brush);
                    }
                }
                UiPrim::Clip { x, y, w, h } => {
                    let r = D2D_RECT_F {
                        left: *x,
                        top: *y,
                        right: x + w,
                        bottom: y + h,
                    };
                    self.ctx
                        .PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                    depth += 1;
                }
                UiPrim::Unclip => {
                    if depth > 0 {
                        self.ctx.PopAxisAlignedClip();
                        depth -= 1;
                    }
                }
            }
        }
        // Niedomkniete Clip nie moze zostawic kontekstu w zlym stanie.
        while depth > 0 {
            self.ctx.PopAxisAlignedClip();
            depth -= 1;
        }
        Ok(())
    }

    unsafe fn draw_hud(&self, text: &str) {
        let k = self.ui_scale;
        let lines = text.lines().count().max(1) as f32;
        let rect = D2D_RECT_F {
            left: 16.0 * k,
            top: 16.0 * k,
            right: (16.0 + 640.0) * k,
            bottom: (16.0 + 14.0) * k + lines * 18.0 * k,
        };
        self.ctx.FillRectangle(&rect, &self.hud_bg);
        let wide: Vec<u16> = text.encode_utf16().collect();
        let inner = D2D_RECT_F {
            left: rect.left + 10.0 * k,
            top: rect.top + 7.0 * k,
            right: rect.right - 10.0 * k,
            bottom: rect.bottom,
        };
        self.ctx.DrawText(
            &wide,
            &self.fonts.mono,
            &inner,
            &self.hud_fg,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
}

fn swapchain_flags(tearing: bool) -> DXGI_SWAP_CHAIN_FLAG {
    let mut f = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0;
    if tearing {
        f |= DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0;
    }
    DXGI_SWAP_CHAIN_FLAG(f)
}

unsafe fn solid(ctx: &ID2D1DeviceContext, r: f32, g: f32, b: f32, a: f32) -> Result<ID2D1Brush> {
    ctx.CreateSolidColorBrush(&D2D1_COLOR_F { r, g, b, a }, None)?
        .cast()
}

fn pick_low_power_adapter(factory: &IDXGIFactory6) -> Result<(IDXGIAdapter1, String)> {
    unsafe {
        let mut index = 0u32;
        loop {
            let adapter: IDXGIAdapter1 =
                factory.EnumAdapterByGpuPreference(index, DXGI_GPU_PREFERENCE_MINIMUM_POWER)?;
            let desc = adapter.GetDesc1()?;
            index += 1;
            if DXGI_ADAPTER_FLAG(desc.Flags as i32) == DXGI_ADAPTER_FLAG_SOFTWARE {
                continue;
            }
            let name = String::from_utf16_lossy(&desc.Description)
                .trim_end_matches('\0')
                .to_string();
            return Ok((adapter, name));
        }
    }
}
