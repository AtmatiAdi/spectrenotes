//! Instalacja per uzytkownik i podmiana binarki przy aktualizacji.
//!
//! Aplikacja mieszka w `%LOCALAPPDATA%\SpectreNotes\app\spectrenotes.exe`
//! (bez uprawnien administratora), ma skrot w menu Start i wpis w
//! "Zainstalowane aplikacje" (`HKCU\...\Uninstall`). Dane zostaja tam,
//! gdzie byly: `%APPDATA%\SpectreNotes` - instalacja i odinstalowanie
//! ich nie dotykaja.
//!
//! Podmiana dzialajacej binarki: Windows nie pozwala jej nadpisac ani
//! skasowac, ale pozwala **przemianowac** - obraz w pamieci trzyma sie
//! pliku, nie nazwy. Wiec: `spectrenotes.exe` -> `spectrenotes.old.exe`,
//! nowy plik na miejsce starego, restart, a nowa instancja po starcie
//! sprzata `.old.exe` (`cleanup_old`).

use std::path::{Path, PathBuf};

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetWindowThreadProcessId, PostMessageW, WM_APP,
};

use crate::window::wide;

/// Nazwa pliku binarki - ta sama w wydaniu (zasob release'u) i na dysku.
pub const EXE_NAME: &str = "spectrenotes.exe";
/// Klasa okna aplikacji (`create_window`) - po niej szukamy dzialajacej instancji.
pub const WINDOW_CLASS: &str = "SpectreNotes";
/// Prosba o zakonczenie procesu (nie schowanie do traya): instalator
/// wysyla ja dzialajacej instancji przed podmiana pliku.
pub const WM_QUIT_APP: u32 = WM_APP + 5;

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\SpectreNotes";

/// `%LOCALAPPDATA%\SpectreNotes\app`.
pub fn app_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("SpectreNotes").join("app")
}

/// `%LOCALAPPDATA%\SpectreNotes\updates` - pobrane, jeszcze niezainstalowane wydania.
pub fn updates_dir() -> PathBuf {
    app_dir().with_file_name("updates")
}

/// Skrot w menu Start biezacego uzytkownika.
pub fn shortcut_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(r"Microsoft\Windows\Start Menu\Programs\SpectreNotes.lnk")
}

/// Sciezka biezacej binarki.
pub fn current_exe() -> Option<PathBuf> {
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(None, &mut buf) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

/// Czy biezaca binarka to ta zainstalowana (a nie np. `target\release`).
pub fn is_installed_copy() -> bool {
    match current_exe() {
        Some(p) => same_path(&p, &app_dir().join(EXE_NAME)),
        None => false,
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

/// Instalacja tej binarki: zamyka dzialajaca instancje, kopiuje plik do
/// `app_dir`, robi skrot i wpis odinstalowania. Zwraca sciezke
/// zainstalowanej binarki (do uruchomienia).
pub fn install_self(version: &str) -> std::io::Result<PathBuf> {
    let src = current_exe().ok_or_else(|| std::io::Error::other("executable path"))?;
    let dir = app_dir();
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(EXE_NAME);
    quit_other_instances();
    if !same_path(&src, &dest) {
        // Zainstalowana kopia mogla byc jeszcze w uzyciu (instancja konczy sie
        // chwile po naszym komunikacie) - wtedy najpierw ja przemianowujemy.
        if dest.exists() && std::fs::copy(&src, &dest).is_err() {
            let old = dir.join("spectrenotes.old.exe");
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&dest, &old)?;
            std::fs::copy(&src, &dest)?;
        } else if !dest.exists() {
            std::fs::copy(&src, &dest)?;
        }
    }
    create_shortcut(&shortcut_path(), &dest, "", "SpectreNotes - pen notes")?;
    let size_kb = std::fs::metadata(&dest).map(|m| m.len() / 1024).unwrap_or(0) as u32;
    register_uninstall(version, &dest, size_kb)?;
    Ok(dest)
}

/// Odinstalowanie: skrot, wpis, autostart; katalog aplikacji kasuje
/// odlozone polecenie, bo wlasnego pliku proces nie usunie.
pub fn uninstall_self() -> std::io::Result<()> {
    quit_other_instances();
    let _ = std::fs::remove_file(shortcut_path());
    let _ = crate::autostart::set_enabled(false);
    unregister_uninstall();
    let dir = app_dir();
    let _ = std::fs::remove_dir_all(updates_dir());
    // `ping` jako opoznienie (cmd nie ma `sleep`): proces musi sie skonczyc,
    // zanim `rmdir` zdejmie jego plik.
    // `raw_arg`: `Command` cytowalby cala linie, a cmd gubi sie przy wiecej niz
    // dwu cudzyslowach w `/c`.
    use std::os::windows::process::CommandExt;
    std::process::Command::new("cmd")
        .raw_arg(format!(
            "/c ping 127.0.0.1 -n 3 >nul & rmdir /s /q \"{}\"",
            dir.display()
        ))
        .spawn()?;
    Ok(())
}

/// Konczy inne instancje aplikacji (po klasie okna) i czeka, az ich procesy
/// znikna - do 10 s kazdy. Wlasny proces pomija.
pub fn quit_other_instances() {
    let class = wide(WINDOW_CLASS);
    let me = std::process::id();
    let mut handles = Vec::new();
    let mut prev: Option<HWND> = None;
    unsafe {
        while let Ok(hwnd) = FindWindowExW(None, prev, PCWSTR(class.as_ptr()), PCWSTR::null()) {
            if hwnd.is_invalid() {
                break;
            }
            prev = Some(hwnd);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 || pid == me {
                continue;
            }
            if let Ok(h) = OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            ) {
                let _ = PostMessageW(Some(hwnd), WM_QUIT_APP, WPARAM(0), LPARAM(0));
                handles.push(h);
            }
        }
        for h in handles {
            let _ = WaitForSingleObject(h, 10_000);
            let _ = CloseHandle(h);
        }
    }
}

/// Podmiana biezacej binarki na `new` (rename + move). Po sukcesie nalezy
/// uruchomic nowy plik i zakonczyc ten proces. Przy bledzie na drugim
/// kroku stara nazwa wraca.
pub fn replace_current_exe(new: &Path) -> std::io::Result<PathBuf> {
    let cur = current_exe().ok_or_else(|| std::io::Error::other("executable path"))?;
    let old = old_exe_path(&cur);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&cur, &old)?;
    if let Err(e) = std::fs::rename(new, &cur).or_else(|_| std::fs::copy(new, &cur).map(|_| ())) {
        let _ = std::fs::rename(&old, &cur);
        return Err(e);
    }
    Ok(cur)
}

fn old_exe_path(cur: &Path) -> PathBuf {
    cur.with_extension("old.exe")
}

/// Sprzata `spectrenotes.old.exe` po poprzedniej aktualizacji. Wolac na
/// starcie; jesli stary proces jeszcze zyje, plik zostanie do nastepnego razu.
pub fn cleanup_old() {
    if let Some(cur) = current_exe() {
        let old = old_exe_path(&cur);
        for _ in 0..10 {
            if !old.exists() || std::fs::remove_file(&old).is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}

/// Uruchamia `exe` z argumentami jako osobny proces (nie czeka).
pub fn spawn(exe: &Path, args: &[String]) -> std::io::Result<()> {
    std::process::Command::new(exe)
        .args(args)
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .spawn()
        .map(|_| ())
}

/// Uruchamia `exe` i czeka na koniec; zwraca kod wyjscia.
pub fn run_and_wait(exe: &Path, args: &[String]) -> std::io::Result<i32> {
    let st = std::process::Command::new(exe).args(args).status()?;
    Ok(st.code().unwrap_or(-1))
}

/// Skrot `.lnk` (IShellLink). Katalog roboczy = katalog binarki.
pub fn create_shortcut(lnk: &Path, target: &Path, args: &str, desc: &str) -> std::io::Result<()> {
    let com = |e: windows::core::Error| std::io::Error::other(format!("shortcut: {e}"));
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(com)?;
        let target_w = wide(&target.to_string_lossy());
        let dir_w = wide(
            &target
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        let args_w = wide(args);
        let desc_w = wide(desc);
        link.SetPath(PCWSTR(target_w.as_ptr())).map_err(com)?;
        link.SetWorkingDirectory(PCWSTR(dir_w.as_ptr())).map_err(com)?;
        link.SetArguments(PCWSTR(args_w.as_ptr())).map_err(com)?;
        link.SetDescription(PCWSTR(desc_w.as_ptr())).map_err(com)?;
        let file: IPersistFile = link.cast().map_err(com)?;
        if let Some(parent) = lnk.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lnk_w = wide(&lnk.to_string_lossy());
        file.Save(PCWSTR(lnk_w.as_ptr()), true).map_err(com)?;
    }
    Ok(())
}

fn set_sz(h: HKEY, name: &str, value: &str) -> std::io::Result<()> {
    let name_w = wide(name);
    let data = wide(value);
    let bytes: Vec<u8> = data.iter().flat_map(|c| c.to_le_bytes()).collect();
    let rc = unsafe { RegSetValueExW(h, PCWSTR(name_w.as_ptr()), None, REG_SZ, Some(&bytes)) };
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("registry {name}: code {}", rc.0)))
    }
}

fn set_dword(h: HKEY, name: &str, value: u32) -> std::io::Result<()> {
    let name_w = wide(name);
    let rc = unsafe {
        RegSetValueExW(
            h,
            PCWSTR(name_w.as_ptr()),
            None,
            REG_DWORD,
            Some(&value.to_le_bytes()),
        )
    };
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("registry {name}: code {}", rc.0)))
    }
}

/// Wpis w "Zainstalowane aplikacje" (Ustawienia -> Aplikacje).
pub fn register_uninstall(version: &str, exe: &Path, size_kb: u32) -> std::io::Result<()> {
    let key = wide(UNINSTALL_KEY);
    let mut h = HKEY::default();
    let rc = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut h,
            None,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(std::io::Error::other(format!("uninstall key: code {}", rc.0)));
    }
    let exe_s = exe.to_string_lossy();
    let dir_s = exe
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let r = (|| {
        set_sz(h, "DisplayName", "SpectreNotes")?;
        set_sz(h, "DisplayVersion", version)?;
        set_sz(h, "Publisher", "SpectreNotes")?;
        set_sz(h, "InstallLocation", &dir_s)?;
        set_sz(h, "DisplayIcon", &exe_s)?;
        set_sz(h, "UninstallString", &format!("\"{exe_s}\" --uninstall"))?;
        set_sz(h, "URLInfoAbout", env!("CARGO_PKG_REPOSITORY"))?;
        set_dword(h, "NoModify", 1)?;
        set_dword(h, "NoRepair", 1)?;
        set_dword(h, "EstimatedSize", size_kb)
    })();
    unsafe {
        let _ = RegCloseKey(h);
    }
    r
}

pub fn unregister_uninstall() {
    let key = wide(UNINSTALL_KEY);
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sciezki_instalacji() {
        assert!(app_dir().ends_with(r"SpectreNotes\app"));
        assert!(updates_dir().ends_with(r"SpectreNotes\updates"));
        assert!(shortcut_path().ends_with("SpectreNotes.lnk"));
        assert_eq!(
            old_exe_path(Path::new(r"C:\x\spectrenotes.exe")),
            PathBuf::from(r"C:\x\spectrenotes.old.exe")
        );
    }

    #[test]
    fn podmiana_pliku_przez_rename() {
        // Symulacja na zwyklych plikach: rename + move i sprzatanie.
        let dir = std::env::temp_dir().join(format!("sn-inst-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cur = dir.join("spectrenotes.exe");
        let new = dir.join("new.exe");
        std::fs::write(&cur, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        let old = old_exe_path(&cur);
        std::fs::rename(&cur, &old).unwrap();
        std::fs::rename(&new, &cur).unwrap();
        assert_eq!(std::fs::read(&cur).unwrap(), b"new");
        assert_eq!(std::fs::read(&old).unwrap(), b"old");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
