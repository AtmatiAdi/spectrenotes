//! Minimalny klient HTTPS na WinHTTP - do GitHub API (device flow, repo).
//! Systemowy TLS i proxy, zero zaleznosci; kilka zadan na sesje, wiec
//! bez puli polaczen i bez asynchronicznosci. Wolac tylko z watku sync.

use std::ffi::c_void;

use windows::core::{Error, Result, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

use crate::window::wide;

pub struct Response {
    pub status: u32,
    pub body: String,
}

/// Zadanie HTTPS. `url` musi byc `https://host/sciezka`. Naglowki jako
/// gotowe linie `Nazwa: wartosc`. Limit ciala odpowiedzi 4 MB.
pub fn request(method: &str, url: &str, headers: &[&str], body: Option<&str>) -> Result<Response> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG))?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let agent = wide("SpectreNotes/0.1");
    let host_w = wide(host);
    let path_w = wide(path);
    let method_w = wide(method);
    unsafe {
        let session = WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        );
        if session.is_null() {
            return Err(Error::from_thread());
        }
        let _s = Handle(session);
        let connect = WinHttpConnect(session, PCWSTR(host_w.as_ptr()), 443, 0);
        if connect.is_null() {
            return Err(Error::from_thread());
        }
        let _c = Handle(connect);
        let req = WinHttpOpenRequest(
            connect,
            PCWSTR(method_w.as_ptr()),
            PCWSTR(path_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        );
        if req.is_null() {
            return Err(Error::from_thread());
        }
        let _r = Handle(req);
        for h in headers {
            let line = wide(&format!("{h}\r\n"));
            // Bez zera koncowego: dlugosc jawna.
            WinHttpAddRequestHeaders(req, &line[..line.len() - 1], WINHTTP_ADDREQ_FLAG_ADD)?;
        }
        let body_bytes = body.map(str::as_bytes).unwrap_or(&[]);
        WinHttpSendRequest(
            req,
            None,
            Some(body_bytes.as_ptr() as *const c_void),
            body_bytes.len() as u32,
            body_bytes.len() as u32,
            0,
        )?;
        WinHttpReceiveResponse(req, std::ptr::null_mut())?;

        let mut status: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            req,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut len,
            std::ptr::null_mut(),
        )?;

        let mut out = Vec::new();
        loop {
            let mut avail: u32 = 0;
            WinHttpQueryDataAvailable(req, &mut avail)?;
            if avail == 0 {
                break;
            }
            let start = out.len();
            out.resize(start + avail as usize, 0);
            let mut read: u32 = 0;
            WinHttpReadData(
                req,
                out[start..].as_mut_ptr() as *mut c_void,
                avail,
                &mut read,
            )?;
            out.truncate(start + read as usize);
            if out.len() > 4 * 1024 * 1024 {
                break;
            }
        }
        Ok(Response {
            status,
            body: String::from_utf8_lossy(&out).into_owned(),
        })
    }
}

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wymaga sieci - uruchamiac recznie: `cargo test -p spectre-shell-win -- --ignored`.
    #[test]
    #[ignore]
    fn github_api_odpowiada() {
        let r = request(
            "GET",
            "https://api.github.com/zen",
            &[
                "User-Agent: SpectreNotes",
                "Accept: application/vnd.github+json",
            ],
            None,
        )
        .unwrap();
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(!r.body.is_empty());
    }
}
