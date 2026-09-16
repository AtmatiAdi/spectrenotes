//! SpectreNotes-Setup.exe - instalator, ktory sam nic nie zawiera: pobiera
//! najnowsze wydanie z GitHuba (`spectre_update`), sprawdza sume SHA-256
//! i uruchamia pobrana binarke z `--install`. Dzieki temu nie starzeje sie:
//! ten sam plik zawsze instaluje biezaca wersje.
//!
//! Jedno male okno: tekst, pasek postepu, przycisk Anuluj/Zamknij. Cala
//! robota w watku roboczym, okno tylko pokazuje postep.
//!
//! `--token <t>` (albo `GITHUB_TOKEN`) - dostep do prywatnego repozytorium
//! wydan; `--repo owner/repo` - inne repozytorium niz wbudowane.

#![windows_subsystem = "windows"]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use spectre_shell_win::install;
use spectre_shell_win::window::wide;
use spectre_update::{human_bytes, Client, ASSET_EXE};
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, GetStockObject, CLEARTYPE_QUALITY, DEFAULT_CHARSET, FF_DONTCARE, FW_NORMAL,
    HBRUSH, HFONT, OUT_DEFAULT_PRECIS, WHITE_BRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_PROGRESS_CLASS, INITCOMMONCONTROLSEX, PBM_SETPOS, PBM_SETRANGE32,
    PROGRESS_CLASSW,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Watek -> okno: zmienil sie tekst/postep (odczyt z `Shared`).
const WM_PROGRESS: u32 = WM_APP + 1;
/// Watek -> okno: koniec; `wparam` 0 = sukces, 1 = blad.
const WM_DONE: u32 = WM_APP + 2;

const ID_BUTTON: isize = 1;
const ID_LABEL: isize = 2;
const ID_BAR: isize = 3;

struct Shared {
    text: String,
    /// Postep 0..1000 (`None` = nieznany).
    pos: Option<u32>,
}

struct Setup {
    label: HWND,
    bar: HWND,
    button: HWND,
    shared: Arc<Mutex<Shared>>,
    cancel: Arc<AtomicBool>,
    done: bool,
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_PROGRESS_CLASS,
        };
        let _ = InitCommonControlsEx(&icc);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let token = arg_value(&args, "--token").or_else(|| std::env::var("GITHUB_TOKEN").ok());
    let repo =
        arg_value(&args, "--repo").unwrap_or_else(|| spectre_update::default_repo().to_string());

    let hwnd = create_window()?;
    let shared = Arc::new(Mutex::new(Shared {
        text: "Looking up the latest release...".into(),
        pos: None,
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let setup = Box::new(Setup {
        label: child(hwnd, ID_LABEL),
        bar: child(hwnd, ID_BAR),
        button: child(hwnd, ID_BUTTON),
        shared: shared.clone(),
        cancel: cancel.clone(),
        done: false,
    });
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(setup) as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    let hwnd_raw = hwnd.0 as isize;
    std::thread::spawn(move || {
        let r = run(&repo, token.as_deref(), &shared, &cancel, hwnd_raw);
        let (code, text) = match r {
            Ok(()) => (0, "Installed. SpectreNotes is starting.".to_string()),
            Err(e) => (1, format!("Failed: {e}")),
        };
        shared.lock().unwrap().text = text;
        unsafe {
            let _ = PostMessageW(
                Some(HWND(hwnd_raw as *mut _)),
                WM_DONE,
                WPARAM(code),
                LPARAM(0),
            );
        }
    });
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn child(hwnd: HWND, id: isize) -> HWND {
    unsafe { GetDlgItem(Some(hwnd), id as i32).unwrap_or_default() }
}

/// Cala robota: wydanie -> pobranie z weryfikacja -> `--install`.
fn run(
    repo: &str,
    token: Option<&str>,
    shared: &Arc<Mutex<Shared>>,
    cancel: &Arc<AtomicBool>,
    hwnd_raw: isize,
) -> std::result::Result<(), String> {
    let post = || unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd_raw as *mut _)),
            WM_PROGRESS,
            WPARAM(0),
            LPARAM(0),
        );
    };
    let set = |text: String, pos: Option<u32>| {
        let mut s = shared.lock().unwrap();
        s.text = text;
        s.pos = pos;
        drop(s);
        post();
    };
    let client = Client::new(repo, token);
    let release = client.latest().map_err(|e| match e {
        spectre_update::Error::NotFound if token.is_none() => {
            format!("{e}\nIf the repository is private, run with --token <GitHub token>.")
        }
        e => e.to_string(),
    })?;
    let asset = release
        .asset(ASSET_EXE)
        .ok_or_else(|| format!("release {} has no {ASSET_EXE}", release.tag))?;
    let size = asset.size;
    set(
        format!(
            "Downloading SpectreNotes {} ({})...",
            release.version,
            human_bytes(size)
        ),
        Some(0),
    );
    let dir = std::env::temp_dir().join("SpectreNotes-Setup");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(ASSET_EXE);
    let version = release.version.to_string();
    let mut progress = |done: u64, total: Option<u64>| -> bool {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let total = total.unwrap_or(size).max(1);
        set(
            format!(
                "Downloading SpectreNotes {version}: {} / {}",
                human_bytes(done),
                human_bytes(total)
            ),
            Some(((done * 1000) / total) as u32),
        );
        true
    };
    client
        .fetch_verified(&release, ASSET_EXE, &dest, &mut progress)
        .map_err(|e| e.to_string())?;
    set("Installing...".into(), Some(1000));
    let code =
        install::run_and_wait(&dest, &["--install".to_string()]).map_err(|e| e.to_string())?;
    if code != 0 {
        return Err(format!("installer exited with code {code}"));
    }
    let _ = std::fs::remove_file(&dest);
    Ok(())
}

fn create_window() -> Result<HWND> {
    unsafe {
        let class = wide("SpectreNotesSetup");
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
            lpszClassName: PCWSTR(class.as_ptr()),
            hIcon: spectre_shell_win::window::app_icon(0).unwrap_or(LoadIconW(None, IDI_APPLICATION)?),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let k = GetDpiForSystem() as f32 / 96.0;
        let px = |v: f32| (v * k).round() as i32;
        let (w, h) = (px(460.0), px(150.0));
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        let title = wide("SpectreNotes Setup");
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            (sw - w) / 2,
            (sh - h) / 2,
            w,
            h,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;
        let font = ui_font(k);
        let make =
            |cls: &str, text: &str, style: u32, x: f32, y: f32, cw: f32, ch: f32, id: isize| {
                let cls_w = wide(cls);
                let text_w = wide(text);
                let h = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    PCWSTR(cls_w.as_ptr()),
                    PCWSTR(text_w.as_ptr()),
                    WINDOW_STYLE(style) | WS_CHILD | WS_VISIBLE,
                    px(x),
                    px(y),
                    px(cw),
                    px(ch),
                    Some(hwnd),
                    Some(HMENU(id as *mut _)),
                    Some(hinst.into()),
                    None,
                )
                .unwrap_or_default();
                SendMessageW(
                    h,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
                h
            };
        make(
            "STATIC",
            "Starting...",
            0x0080, /* SS_NOPREFIX */
            16.0,
            14.0,
            428.0,
            40.0,
            ID_LABEL,
        );
        let bar = make(
            &String::from_utf16_lossy(PROGRESS_CLASSW.as_wide()),
            "",
            0,
            16.0,
            60.0,
            428.0,
            14.0,
            ID_BAR,
        );
        SendMessageW(bar, PBM_SETRANGE32, Some(WPARAM(0)), Some(LPARAM(1000)));
        make(
            "BUTTON",
            "Cancel",
            BS_PUSHBUTTON as u32,
            348.0,
            86.0,
            96.0,
            28.0,
            ID_BUTTON,
        );
        Ok(hwnd)
    }
}

fn ui_font(k: f32) -> HFONT {
    let face = wide("Segoe UI");
    unsafe {
        CreateFontW(
            -(12.0 * k).round() as i32,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            windows::Win32::Graphics::Gdi::CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            FF_DONTCARE.0 as u32,
            PCWSTR(face.as_ptr()),
        )
    }
}

fn set_text(hwnd: HWND, text: &str) {
    let w = wide(text);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(w.as_ptr()));
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Setup;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let s = &mut *ptr;
    match msg {
        WM_PROGRESS => {
            let (text, pos) = {
                let sh = s.shared.lock().unwrap();
                (sh.text.clone(), sh.pos)
            };
            set_text(s.label, &text);
            if let Some(p) = pos {
                SendMessageW(s.bar, PBM_SETPOS, Some(WPARAM(p as usize)), Some(LPARAM(0)));
            }
            LRESULT(0)
        }
        WM_DONE => {
            let text = s.shared.lock().unwrap().text.clone();
            set_text(s.label, &text);
            s.done = true;
            set_text(s.button, "Close");
            if wparam.0 == 0 {
                SendMessageW(s.bar, PBM_SETPOS, Some(WPARAM(1000)), Some(LPARAM(0)));
                // Zainstalowana aplikacja juz startuje - okno instalatora znika samo.
                SetTimer(Some(hwnd), 1, 1500, None);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            if (wparam.0 & 0xffff) as isize == ID_BUTTON {
                if s.done {
                    let _ = DestroyWindow(hwnd);
                } else {
                    s.cancel.store(true, Ordering::Relaxed);
                    set_text(s.label, "Cancelling...");
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            s.cancel.store(true, Ordering::Relaxed);
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(ptr));
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
