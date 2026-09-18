//! Miniatury notatek rysowane **poza watkiem okna**.
//!
//! Miniatura to cala notatka narysowana raz (odczyt operacji, dokument,
//! geometria kazdej kreski, teselacja) - przy setkach notatek to sekundy pracy,
//! ktore nie moga isc z klatek okna. `ThumbRenderer` ma wlasne urzadzenie D3D11
//! (ten sam adapter o najnizszym poborze mocy, co glowny renderer) i wlasny
//! kontekst D2D, wiec zyje w osobnym watku i nie dotyka niczego, co nalezy do
//! okna. Wynik to piksele BGRA - watek okna tylko wrzuca je do bitmapy
//! (`Renderer::load_thumb`, ulamek milisekundy).
//!
//! Kadr jest **jak strona**: w poziomie zawsze cala kolumna (miniatury roznych
//! notatek maja te sama skale i da sie je porownac), w pionie od gory tresci -
//! notatka zaczynajaca sie nisko nie daje pustego kafelka.

use std::collections::HashMap;

use spectre_core::camera::COLUMN_W;
use spectre_core::{Bbox, Camera};
use spectre_ink::{InkConfig, Segment};
use spectre_proto::{Rgba, StrokeData};
use windows::core::{Interface, Result};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Brush, ID2D1DeviceContext, ID2D1Factory1,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_CPU_READ, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_MAP_OPTIONS_READ,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory2, IDXGIDevice, IDXGIFactory6, DXGI_CREATE_FACTORY_FLAGS};
use windows_numerics::Matrix3x2;

use crate::d2d::{pick_low_power_adapter, BG_COLOR};
use crate::geometry;
use crate::tess::stroke_segments;

/// Kadr zaczyna sie tyle pikseli miniatury **nad** trescia: kreska lezaca
/// dokladnie na gornej krawedzi wypadala pol piksela za kadrem i kafelek
/// wygladal na pusty.
const TOP_MARGIN_PX: f32 = 3.0;
/// Minimalna grubosc kreski w pikselach miniatury. Geometria jest budowana tak,
/// jakby zoom byl mniejszy - inaczej kreski zwezone ~20 razy bylyby niewidoczne.
const MIN_PX: f32 = 1.2;

pub struct ThumbRenderer {
    factory2d: ID2D1Factory1,
    ctx: ID2D1DeviceContext,
    brushes: HashMap<u32, ID2D1Brush>,
    segs: Vec<Segment>,
}

impl ThumbRenderer {
    pub fn new() -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory6 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
            let (adapter, _) = pick_low_power_adapter(&factory)?;
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
            let factory2d: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dxgi_device: IDXGIDevice = device.cast()?;
            let device2d = factory2d.CreateDevice(&dxgi_device)?;
            let ctx = device2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            ctx.SetDpi(96.0, 96.0);
            ctx.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            Ok(Self {
                factory2d,
                ctx,
                brushes: HashMap::new(),
                segs: Vec::new(),
            })
        }
    }

    /// Miniatura `w` x `h` z kresek notatki (w kolejnosci rysowania): BGRA,
    /// wiersz po wierszu, bez wyrownania.
    pub fn render(&mut self, strokes: &[StrokeData], ink: &InkConfig, w: u32, h: u32) -> Result<Vec<u8>> {
        let zoom = w as f32 / COLUMN_W;
        let content = strokes
            .iter()
            .map(Bbox::of)
            .reduce(|a, b| Bbox {
                min_x: a.min_x.min(b.min_x),
                min_y: a.min_y.min(b.min_y),
                max_x: a.max_x.max(b.max_x),
                max_y: a.max_y.max(b.max_y),
            });
        let margin = TOP_MARGIN_PX / zoom;
        let top = content.map_or(0.0, |b| (b.min_y - margin).max(0.0));
        let cam = Camera {
            scroll_x: 0.0,
            scroll_y: top,
            zoom,
            shift: (0.0, 0.0),
        };
        let region = cam.visible(w as f32, h as f32);
        unsafe {
            let size = D2D_SIZE_U {
                width: w.max(1),
                height: h.max(1),
            };
            let target = self.ctx.CreateBitmap(size, None, 0, &props(D2D1_BITMAP_OPTIONS_TARGET))?;
            self.ctx.SetTarget(&target);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&BG_COLOR));
            let res = self.draw(strokes, &cam, ink, region);
            self.ctx.EndDraw(None, None)?;
            self.ctx.SetTarget(None);
            res?;
            // Piksele: bitmapa docelowa siedzi na GPU, kopia przez `CPU_READ` i `Map`.
            let staging = self.ctx.CreateBitmap(
                size,
                None,
                0,
                &props(D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW),
            )?;
            staging.CopyFromBitmap(None, &target, None)?;
            let map = staging.Map(D2D1_MAP_OPTIONS_READ)?;
            let mut out = Vec::with_capacity((size.width * size.height * 4) as usize);
            for y in 0..size.height {
                let row = map.bits.add((y * map.pitch) as usize);
                out.extend_from_slice(std::slice::from_raw_parts(row, (size.width * 4) as usize));
            }
            staging.Unmap()?;
            Ok(out)
        }
    }

    unsafe fn draw(
        &mut self,
        strokes: &[StrokeData],
        cam: &Camera,
        ink: &InkConfig,
        rect: Bbox,
    ) -> Result<()> {
        self.ctx.SetTransform(&Matrix3x2 {
            M11: cam.zoom,
            M12: 0.0,
            M21: 0.0,
            M22: cam.zoom,
            M31: -cam.scroll_x * cam.zoom,
            M32: -cam.scroll_y * cam.zoom,
        });
        let geo_zoom = (cam.zoom * geometry::MIN_WIDTH_PX / MIN_PX).max(1e-4);
        let mut segs = std::mem::take(&mut self.segs);
        let mut res = Ok(());
        for data in strokes {
            if !Bbox::of(data).intersects(&rect) {
                continue;
            }
            segs.clear();
            stroke_segments(data, ink, &mut segs);
            match geometry::build(&self.factory2d, &segs, geo_zoom) {
                Ok(geo) => match self.brush(data.color) {
                    Ok(brush) => self.ctx.FillGeometry(&geo, &brush, None),
                    Err(e) => {
                        res = Err(e);
                        break;
                    }
                },
                Err(e) => {
                    res = Err(e);
                    break;
                }
            }
        }
        self.segs = segs;
        self.ctx.SetTransform(&Matrix3x2::identity());
        res
    }

    fn brush(&mut self, color: Rgba) -> Result<ID2D1Brush> {
        let key = u32::from_le_bytes([color.r, color.g, color.b, color.a]);
        if let Some(b) = self.brushes.get(&key) {
            return Ok(b.clone());
        }
        let b: ID2D1Brush = unsafe {
            self.ctx
                .CreateSolidColorBrush(
                    &D2D1_COLOR_F {
                        r: color.r as f32 / 255.0,
                        g: color.g as f32 / 255.0,
                        b: color.b as f32 / 255.0,
                        a: color.a as f32 / 255.0,
                    },
                    None,
                )?
                .cast()?
        };
        self.brushes.insert(key, b.clone());
        Ok(b)
    }
}

fn props(options: windows::Win32::Graphics::Direct2D::D2D1_BITMAP_OPTIONS) -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_IGNORE,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: options,
        ..Default::default()
    }
}
