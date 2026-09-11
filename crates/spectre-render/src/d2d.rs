use std::collections::HashMap;
use std::mem::size_of;

use spectre_core::{Bbox, Camera, Document};
use spectre_ink::{InkConfig, Segment};
use spectre_proto::Rgba;
use windows::core::{Interface, Result, PCWSTR};
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1Brush, ID2D1DeviceContext, ID2D1Factory1,
    ID2D1StrokeStyle1, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_CAP_STYLE_ROUND,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_LINE_JOIN_ROUND, D2D1_STROKE_STYLE_PROPERTIES1,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_MEASURING_MODE_NATURAL,
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
use windows_numerics::Vector2;

use crate::tess::stroke_segments;

/// Czysta czern: na AMOLED piksel jest wtedy fizycznie zgaszony (Z7).
pub const BG_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

/// Co narysowac na wierzchu klatki, poza dokumentem.
#[derive(Default, Clone, Copy)]
pub struct Overlay<'a> {
    pub hud: Option<&'a str>,
    /// Okrag gumki: (x, y, promien) w pikselach ekranu.
    pub cursor: Option<(f32, f32, f32)>,
}

pub struct Renderer {
    adapter_name: String,
    tearing_supported: bool,
    size: (u32, u32),

    device: ID3D11Device,
    swapchain: IDXGISwapChain1,
    _factory2d: ID2D1Factory1,
    ctx: ID2D1DeviceContext,

    backbuffer: Option<ID2D1Bitmap1>,
    /// Dwie bitmapy warstwy suchej: biezaca i zapasowa do przewijania przyrostowego.
    dry: [Option<ID2D1Bitmap1>; 2],
    dry_cur: usize,

    brushes: HashMap<u32, ID2D1Brush>,
    hud_fg: ID2D1Brush,
    hud_bg: ID2D1Brush,
    cursor_brush: ID2D1Brush,
    round: ID2D1StrokeStyle1,
    text_fmt: IDWriteTextFormat,

    seg_buf: Vec<Segment>,
}

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
            let text_fmt = dwrite.CreateTextFormat(
                w("Consolas"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                14.0,
                w("en-us"),
            )?;

            let mut r = Self {
                adapter_name,
                tearing_supported,
                size: (width.max(1), height.max(1)),
                device,
                swapchain,
                _factory2d: factory2d,
                ctx,
                backbuffer: None,
                dry: [None, None],
                dry_cur: 0,
                brushes: HashMap::new(),
                hud_fg,
                hud_bg,
                cursor_brush,
                round,
                text_fmt,
                seg_buf: Vec::with_capacity(8192),
            };
            r.create_size_dependent()?;
            Ok(r)
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

    /// Rysuje kreski dokumentu przecinajace `rect` (canvas) na biezacy target.
    unsafe fn draw_document(
        &mut self,
        doc: &Document,
        cam: &Camera,
        ink: &InkConfig,
        rect: Bbox,
    ) -> Result<()> {
        // Zbieramy geometrie per kolor, zeby nie zmieniac pedzla na kazda kreske.
        let mut by_color: HashMap<u32, Vec<Segment>> = HashMap::new();
        for (_, data, _) in doc.visible_in(rect) {
            let key = u32::from_le_bytes([data.color.r, data.color.g, data.color.b, data.color.a]);
            let out = by_color.entry(key).or_default();
            stroke_segments(data, ink, out);
        }
        for (key, segs) in by_color {
            let [r, g, b, a] = key.to_le_bytes();
            let brush = self.brush(Rgba { r, g, b, a })?;
            self.draw_segments(&segs, &brush, cam);
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
        vsync: bool,
    ) -> Result<()> {
        let (Some(back), Some(dry)) = (self.backbuffer.clone(), self.dry[self.dry_cur].clone())
        else {
            return Ok(());
        };
        let brush = self.brush(color)?;
        unsafe {
            self.ctx.SetTarget(&back);
            self.ctx.BeginDraw();
            self.ctx
                .DrawImage(&dry, None, None, Default::default(), Default::default());
            self.draw_segments(tail, &brush, cam);
            if let Some((x, y, r)) = overlay.cursor {
                let e = D2D1_ELLIPSE {
                    point: Vector2 { X: x, Y: y },
                    radiusX: r,
                    radiusY: r,
                };
                self.ctx.DrawEllipse(&e, &self.cursor_brush, 1.0, None);
            }
            if let Some(text) = overlay.hud {
                self.draw_hud(text);
            }
            self.ctx.EndDraw(None, None)?;
            self.ctx.SetTarget(None);

            let (interval, flags) = if vsync {
                (1u32, DXGI_PRESENT(0))
            } else if self.tearing_supported {
                (0u32, DXGI_PRESENT_ALLOW_TEARING)
            } else {
                (0u32, DXGI_PRESENT(0))
            };
            self.swapchain.Present(interval, flags).ok()?;
        }
        let _ = std::mem::take(&mut self.seg_buf);
        Ok(())
    }

    unsafe fn draw_hud(&self, text: &str) {
        let lines = text.lines().count().max(1) as f32;
        let rect = D2D_RECT_F {
            left: 16.0,
            top: 16.0,
            right: 16.0 + 640.0,
            bottom: 16.0 + lines * 18.0 + 14.0,
        };
        self.ctx.FillRectangle(&rect, &self.hud_bg);
        let wide: Vec<u16> = text.encode_utf16().collect();
        let inner = D2D_RECT_F {
            left: rect.left + 10.0,
            top: rect.top + 7.0,
            right: rect.right - 10.0,
            bottom: rect.bottom,
        };
        self.ctx.DrawText(
            &wide,
            &self.text_fmt,
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

fn w(s: &str) -> PCWSTR {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    PCWSTR(Box::leak(v.into_boxed_slice()).as_ptr())
}
