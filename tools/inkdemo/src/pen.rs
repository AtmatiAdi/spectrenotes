//! Dekodowanie probek piora z `WM_POINTER`.
//!
//! Cala wartosc tego pliku sprowadza sie do dwoch rzeczy, ktore wiekszosc
//! aplikacji pomija (patrz `docs/02-PIORO-I-LATENCJA.md`):
//!
//! 1. `GetPointerPenInfoHistory` - pioro raportuje ~240 Hz, a komunikaty
//!    przychodza ~120 Hz. Bez odczytania historii gubimy co druga probke.
//! 2. `ptHimetricLocationRaw` zamiast `ptPixelLocation` - to drugie jest
//!    zaokraglone do calych pikseli ekranu i to ono odpowiada za "schodkowanie".

use std::collections::HashMap;

use spectre_ink::Sample;
use windows::Win32::Foundation::{HANDLE, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::Input::Pointer::{
    GetPointerDeviceRects, GetPointerInfo, GetPointerPenInfo, GetPointerPenInfoHistory,
    GetPointerType, POINTER_INFO, POINTER_PEN_INFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    PEN_FLAG_ERASER, PEN_FLAG_INVERTED, PEN_MASK_PRESSURE, PEN_MASK_TILT_X, PEN_MASK_TILT_Y,
    POINTER_INPUT_TYPE, PT_MOUSE, PT_PEN,
};

/// Zakres wspolrzednych digitizera i odpowiadajacy mu prostokat ekranu.
#[derive(Clone, Copy)]
struct DeviceRects {
    device: RECT,
    display: RECT,
}

pub struct PenDecoder {
    /// Cache per urzadzenie - `GetPointerDeviceRects` to wywolanie systemowe,
    /// a odpowiedz zmienia sie tylko przy zmianie konfiguracji ekranow.
    rects: HashMap<isize, DeviceRects>,
    qpc_freq: f64,
    /// Bufor na historie, zeby nie alokowac na kazdy komunikat.
    scratch: Vec<POINTER_PEN_INFO>,
}

/// Co powiedzialo pioro w jednym komunikacie.
pub struct PenBatch {
    pub samples: Vec<Sample>,
    pub eraser: bool,
    /// Ile probek faktycznie przyszlo z historii (do HUD-u).
    pub history_len: usize,
}

impl PenDecoder {
    pub fn new(qpc_freq: i64) -> Self {
        Self {
            rects: HashMap::new(),
            qpc_freq: qpc_freq as f64,
            scratch: Vec::with_capacity(64),
        }
    }

    pub fn is_pen(&self, pointer_id: u32) -> bool {
        let mut t = POINTER_INPUT_TYPE::default();
        unsafe { GetPointerType(pointer_id, &mut t).is_ok() && (t == PT_PEN || t == PT_MOUSE) }
    }

    /// Odczytuje wszystkie probki zwiazane z danym komunikatem.
    ///
    /// `with_history` jest `false` dla `WM_POINTERDOWN` (historia nie ma wtedy sensu)
    /// i `true` dla `WM_POINTERUPDATE`.
    pub fn decode(&mut self, hwnd: HWND, pointer_id: u32, with_history: bool) -> Option<PenBatch> {
        unsafe {
            let mut count: u32 = 0;
            let mut history_len = 0usize;

            if with_history
                && GetPointerPenInfoHistory(pointer_id, &mut count, None).is_ok()
                && count > 0
            {
                self.scratch.clear();
                self.scratch
                    .resize(count as usize, POINTER_PEN_INFO::default());
                if GetPointerPenInfoHistory(pointer_id, &mut count, Some(self.scratch.as_mut_ptr()))
                    .is_ok()
                {
                    self.scratch.truncate(count as usize);
                    // API oddaje od najnowszej do najstarszej - chcemy odwrotnie.
                    self.scratch.reverse();
                    history_len = self.scratch.len();
                } else {
                    self.scratch.clear();
                }
            }

            if self.scratch.is_empty() {
                let mut info = POINTER_PEN_INFO::default();
                if GetPointerPenInfo(pointer_id, &mut info).is_ok() {
                    self.scratch.push(info);
                } else {
                    // Mysz zglaszana przez `EnableMouseInPointer` - nie ma nacisku
                    // ani tiltu, ale jest uzytecznym punktem odniesienia dla oceny
                    // odczucia piora. `penMask == 0` sprawi, ze `to_sample` przyjmie
                    // stala wartosc nacisku.
                    let mut pi = POINTER_INFO::default();
                    GetPointerInfo(pointer_id, &mut pi).ok()?;
                    self.scratch.push(POINTER_PEN_INFO {
                        pointerInfo: pi,
                        ..Default::default()
                    });
                }
            }

            let origin = client_origin(hwnd);
            let mut samples = Vec::with_capacity(self.scratch.len());
            let mut eraser = false;

            // `scratch` jest pozyczany niemutowalnie w petli, a `self.rects` mutowalnie
            // w `to_sample`, wiec zdejmujemy bufor z `self` na czas konwersji.
            let batch = std::mem::take(&mut self.scratch);
            for info in &batch {
                let flags = info.penFlags;
                if (flags & (PEN_FLAG_INVERTED | PEN_FLAG_ERASER)) != 0 {
                    eraser = true;
                }
                if let Some(s) = self.make_sample(info, origin) {
                    samples.push(s);
                }
            }
            self.scratch = batch;
            self.scratch.clear();

            if samples.is_empty() {
                return None;
            }
            Some(PenBatch {
                samples,
                eraser,
                history_len,
            })
        }
    }

    fn make_sample(&mut self, info: &POINTER_PEN_INFO, origin: (f32, f32)) -> Option<Sample> {
        let pi = &info.pointerInfo;

        let (x, y) = match self.device_rects(pi.sourceDevice) {
            // Sciezka wlasciwa: subpikselowe wspolrzedne z zakresu digitizera.
            Some(r) => {
                let dw = (r.device.right - r.device.left) as f32;
                let dh = (r.device.bottom - r.device.top) as f32;
                if dw <= 0.0 || dh <= 0.0 {
                    return None;
                }
                let fx = (pi.ptHimetricLocationRaw.x - r.device.left) as f32 / dw;
                let fy = (pi.ptHimetricLocationRaw.y - r.device.top) as f32 / dh;
                let sx = r.display.left as f32 + fx * (r.display.right - r.display.left) as f32;
                let sy = r.display.top as f32 + fy * (r.display.bottom - r.display.top) as f32;
                (sx, sy)
            }
            // Awaryjnie (np. mysz uzyta jako punkt odniesienia): cale piksele.
            None => (
                pi.ptPixelLocationRaw.x as f32,
                pi.ptPixelLocationRaw.y as f32,
            ),
        };

        let pressure = if (info.penMask & PEN_MASK_PRESSURE) != 0 {
            (info.pressure as f32 / 1024.0).clamp(0.0, 1.0)
        } else {
            // Mysz i piora bez czujnika nacisku - stala wartosc, zeby kreska istniala.
            0.5
        };
        let tilt_x = if (info.penMask & PEN_MASK_TILT_X) != 0 {
            info.tiltX as f32
        } else {
            0.0
        };
        let tilt_y = if (info.penMask & PEN_MASK_TILT_Y) != 0 {
            info.tiltY as f32
        } else {
            0.0
        };

        // PerformanceCount to znacznik QPC nadany przez stos wejscia, wiec mierzy
        // moment probkowania, a nie moment odebrania komunikatu. To jedyny sposob,
        // zeby policzyc prawdziwa czestotliwosc piora.
        let t_us = if pi.PerformanceCount > 0 && self.qpc_freq > 0.0 {
            ((pi.PerformanceCount as f64 / self.qpc_freq) * 1_000_000.0) as u64
        } else {
            pi.dwTime as u64 * 1000
        };

        Some(Sample {
            x: x - origin.0,
            y: y - origin.1,
            pressure,
            tilt_x,
            tilt_y,
            t_us,
        })
    }

    fn device_rects(&mut self, device: HANDLE) -> Option<DeviceRects> {
        if device.is_invalid() {
            return None;
        }
        let key = device.0 as isize;
        if let Some(r) = self.rects.get(&key) {
            return Some(*r);
        }
        let mut device_rect = RECT::default();
        let mut display_rect = RECT::default();
        unsafe {
            GetPointerDeviceRects(device, &mut device_rect, &mut display_rect).ok()?;
        }
        let r = DeviceRects {
            device: device_rect,
            display: display_rect,
        };
        self.rects.insert(key, r);
        Some(r)
    }

    /// Wolane po `WM_DISPLAYCHANGE` - odwzorowanie digitizer -> ekran przestaje
    /// byc wtedy aktualne.
    pub fn invalidate(&mut self) {
        self.rects.clear();
    }
}

/// Punkt (0,0) obszaru klienta wyrazony we wspolrzednych ekranu.
fn client_origin(hwnd: HWND) -> (f32, f32) {
    let mut p = POINT { x: 0, y: 0 };
    unsafe {
        let _ = ClientToScreen(hwnd, &mut p);
    }
    (p.x as f32, p.y as f32)
}
