//! Systemowe okno wyboru pliku (zalacznik do feedbacku). Modalne wobec
//! naszego okna - pompuje komunikaty samo, wiec wolac tylko z watku okna
//! i poza obsluga rysika.

use std::mem::size_of;
use std::path::PathBuf;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST,
    OPENFILENAMEW,
};

use crate::window::wide;

/// Sciezka wybranego pliku albo `None`, gdy uzytkownik zrezygnowal.
pub fn pick_file(hwnd: HWND, title: &str) -> Option<PathBuf> {
    let mut buf = vec![0u16; 32 * 1024];
    let title = wide(title);
    let filter: Vec<u16> = "All files\0*.*\0Images\0*.png;*.jpg;*.jpeg;*.gif;*.webp\0\0"
        .encode_utf16()
        .collect();
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    let ok = unsafe { GetOpenFileNameW(&mut ofn) }.as_bool();
    if !ok {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    if len == 0 {
        return None;
    }
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}
