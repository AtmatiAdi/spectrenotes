//! D3D11 + DXGI (flip model) + Direct2D.
//!
//! Demo celowo NIE uzywa docelowego renderera na wgpu. Etap 0 sprawdza wylacznie
//! sciezke **wejscia i prezentacji**, a te sa identyczne niezaleznie od tego, skad
//! biora sie trojkaty. Direct2D daje tu pelna kontrole nad swapchainem, buduje sie
//! w sekundy zamiast minut i dorzuca DirectWrite na HUD za darmo.
//! Uzasadnienie wymiennosci renderera: `docs/adr/0001-stack.md`.

use std::mem::size_of;

use spectre_ink::Segment;
use windows::core::{Interface, Result, PCWSTR};
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1Brush, ID2D1DeviceContext, ID2D1Factory1,
    ID2D1StrokeStyle1, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_CAP_STYLE_ROUND,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE,
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
    CreateDXGIFactory2, IDXGIAdapter1, IDXGIDevice, IDXGIFactory6, IDXGISurface, IDXGISwapChain1,
    IDXGISwapChain2, DXGI_ADAPTER_FLAG, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_CREATE_FACTORY_FLAGS,
    DXGI_FEATURE_PRESENT_ALLOW_TEARING, DXGI_GPU_PREFERENCE_MINIMUM_POWER, DXGI_MWA_NO_ALT_ENTER,
    DXGI_PRESENT, DXGI_PRESENT_ALLOW_TEARING, DXGI_SCALING_NONE, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows_numerics::Vector2;

pub const INK_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    // #D8D8D8, nie biel - patrz docs/04-ENERGIA-I-AMOLED.md
    r: 0.847,
    g: 0.847,
    b: 0.847,
    a: 1.0,
};
pub const BG_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    // Czysta czern: na AMOLED piksel jest wtedy fizycznie zgaszony.
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

pub struct Gfx {
    pub adapter_name: String,
    pub tearing_supported: bool,
    pub size: (u32, u32),

    _device: ID3D11Device,
    swapchain: IDXGISwapChain1,
    _factory2d: ID2D1Factory1,
    ctx: ID2D1DeviceContext,

    backbuffer: Option<ID2D1Bitmap1>,
    /// Warstwa sucha: wypalony atrament. Nigdy nie jest przerysowywana w calosci.
    canvas: Option<ID2D1Bitmap1>,

    ink: ID2D1Brush,
    eraser: ID2D1Brush,
    hud_fg: ID2D1Brush,
    hud_bg: ID2D1Brush,
    round: ID2D1StrokeStyle1,
    text_fmt: IDWriteTextFormat,
}

impl Gfx {
    pub fn new(hwnd: HWND, width: u32, height: u32) -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory6 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;

            // Swiadomy wybor adaptera o najnizszym poborze mocy. Domyslna heurystyka
            // DXGI potrafi wybudzic dedykowana karte do rysowania notatek.
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
            let device = device.expect("D3D11CreateDevice zwrocilo sukces bez urzadzenia");

            let mut tearing: u32 = 0;
            let tearing_supported = factory
                .CheckFeatureSupport(
                    DXGI_FEATURE_PRESENT_ALLOW_TEARING,
                    &mut tearing as *mut _ as *mut _,
                    size_of::<u32>() as u32,
                )
                .is_ok()
                && tearing != 0;

            let mut flags = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0;
            if tearing_supported {
                flags |= DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0;
            }

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
                Flags: flags as u32,
                ..Default::default()
            };

            let swapchain = factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)?;
            // Bez tego Alt+Enter wchodzi w tryb pelnoekranowy DXGI, ktorego nie chcemy -
            // pelny ekran robimy sami jako borderless (patrz main.rs).
            let _ = factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);

            // Domyslna kolejka to 3 klatki. Przy 120 Hz to 25 ms samego kolejkowania.
            if let Ok(sc2) = swapchain.cast::<IDXGISwapChain2>() {
                let _ = sc2.SetMaximumFrameLatency(1);
            }

            let factory2d: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dxgi_device: IDXGIDevice = device.cast()?;
            let device2d = factory2d.CreateDevice(&dxgi_device)?;
            let ctx = device2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            // 96 DPI => jednostki D2D sa dokladnie pikselami. Przy PER_MONITOR_AWARE_V2
            // i tak pracujemy w pikselach fizycznych.
            ctx.SetDpi(96.0, 96.0);
            ctx.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            // Skala szarosci, nigdy ClearType - uklad subpikseli OLED daje kolorowe obwodki.
            ctx.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

            let ink: ID2D1Brush = ctx.CreateSolidColorBrush(&INK_COLOR, None)?.cast()?;
            let eraser: ID2D1Brush = ctx.CreateSolidColorBrush(&BG_COLOR, None)?.cast()?;
            let hud_fg: ID2D1Brush = ctx
                .CreateSolidColorBrush(
                    &D2D1_COLOR_F {
                        r: 0.45,
                        g: 0.62,
                        b: 0.55,
                        a: 0.9,
                    },
                    None,
                )?
                .cast()?;
            let hud_bg: ID2D1Brush = ctx
                .CreateSolidColorBrush(
                    &D2D1_COLOR_F {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.55,
                    },
                    None,
                )?
                .cast()?;

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

            let mut gfx = Self {
                adapter_name,
                tearing_supported,
                size: (width.max(1), height.max(1)),
                _device: device,
                swapchain,
                _factory2d: factory2d,
                ctx,
                backbuffer: None,
                canvas: None,
                ink,
                eraser,
                hud_fg,
                hud_bg,
                round,
                text_fmt,
            };
            gfx.create_size_dependent()?;
            Ok(gfx)
        }
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

            let canvas_props = D2D1_BITMAP_PROPERTIES1 {
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
            let new_canvas = self.ctx.CreateBitmap(size, None, 0, &canvas_props)?;

            // Przeniesienie dotychczasowego atramentu na nowa warstwe, zeby zmiana
            // rozmiaru okna nie kasowala rysunku.
            self.ctx.SetTarget(&new_canvas);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&BG_COLOR));
            if let Some(old) = self.canvas.take() {
                self.ctx
                    .DrawImage(&old, None, None, Default::default(), Default::default());
            }
            self.ctx.EndDraw(None, None)?;
            self.ctx.SetTarget(None);
            self.canvas = Some(new_canvas);
            Ok(())
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 || (width, height) == self.size {
            return Ok(());
        }
        unsafe {
            self.ctx.SetTarget(None);
            self.backbuffer = None;
            let mut flags = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0;
            if self.tearing_supported {
                flags |= DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0;
            }
            self.swapchain.ResizeBuffers(
                0,
                width,
                height,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                windows::Win32::Graphics::Dxgi::DXGI_SWAP_CHAIN_FLAG(flags),
            )?;
        }
        self.size = (width, height);
        self.create_size_dependent()
    }

    /// Wypala odcinki do warstwy suchej. Koszt zalezy tylko od liczby NOWYCH
    /// odcinkow, nigdy od tego, ile atramentu jest juz na canvasie.
    pub fn commit(&mut self, segs: &[Segment], erase: bool) -> Result<()> {
        if segs.is_empty() {
            return Ok(());
        }
        let Some(canvas) = self.canvas.clone() else {
            return Ok(());
        };
        unsafe {
            self.ctx.SetTarget(&canvas);
            self.ctx.BeginDraw();
            self.draw_segments(segs, erase);
            self.ctx.EndDraw(None, None)?;
            self.ctx.SetTarget(None);
        }
        Ok(())
    }

    pub fn clear_canvas(&mut self) -> Result<()> {
        let Some(canvas) = self.canvas.clone() else {
            return Ok(());
        };
        unsafe {
            self.ctx.SetTarget(&canvas);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&BG_COLOR));
            self.ctx.EndDraw(None, None)?;
            self.ctx.SetTarget(None);
        }
        Ok(())
    }

    unsafe fn draw_segments(&self, segs: &[Segment], erase: bool) {
        let brush = if erase { &self.eraser } else { &self.ink };
        for s in segs {
            self.ctx.DrawLine(
                Vector2 { X: s.a.x, Y: s.a.y },
                Vector2 { X: s.b.x, Y: s.b.y },
                brush,
                s.width.max(0.4),
                &self.round,
            );
        }
    }

    /// Sklada klatke: warstwa sucha + czubek kreski + HUD, i prezentuje.
    ///
    /// Zwraca czas samego `Present` w milisekundach.
    pub fn present(
        &mut self,
        tail: &[Segment],
        erase: bool,
        hud: Option<&str>,
        vsync: bool,
    ) -> Result<()> {
        let (Some(back), Some(canvas)) = (self.backbuffer.clone(), self.canvas.clone()) else {
            return Ok(());
        };
        unsafe {
            self.ctx.SetTarget(&back);
            self.ctx.BeginDraw();
            self.ctx
                .DrawImage(&canvas, None, None, Default::default(), Default::default());
            // Czubek rysujemy tylko na backbufferze - nastepna klatka narysuje go
            // od nowa, juz z poprawna krzywizna.
            self.draw_segments(tail, erase);
            if let Some(text) = hud {
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
        Ok(())
    }

    unsafe fn draw_hud(&self, text: &str) {
        let lines = text.lines().count().max(1) as f32;
        let rect = D2D_RECT_F {
            left: 16.0,
            top: 16.0,
            right: 16.0 + 470.0,
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

fn pick_low_power_adapter(factory: &IDXGIFactory6) -> Result<(IDXGIAdapter1, String)> {
    unsafe {
        let mut index = 0u32;
        loop {
            let adapter: IDXGIAdapter1 =
                factory.EnumAdapterByGpuPreference(index, DXGI_GPU_PREFERENCE_MINIMUM_POWER)?;
            let desc = adapter.GetDesc1()?;
            index += 1;
            if DXGI_ADAPTER_FLAG(desc.Flags as i32) == DXGI_ADAPTER_FLAG_SOFTWARE {
                continue; // WARP - tylko gdy nie ma nic innego
            }
            let name = String::from_utf16_lossy(&desc.Description)
                .trim_end_matches('\0')
                .to_string();
            return Ok((adapter, name));
        }
    }
}

/// Pomocnik do literalow PCWSTR bez makra `w!` (potrzebuje statycznego bufora).
fn w(s: &str) -> PCWSTR {
    // Celowo przeciekamy - wywolywane raz przy starcie, dla stalych nazw.
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    PCWSTR(Box::leak(v.into_boxed_slice()).as_ptr())
}
