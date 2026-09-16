//! Minimalny klient HTTPS na WinHTTP - do GitHub API (device flow, repo,
//! wydania). Systemowy TLS i proxy, zero zaleznosci; kilka zadan na sesje,
//! wiec bez puli polaczen i bez asynchronicznosci. Wolac tylko z watku
//! roboczego (sync, update), nigdy z watku okna.

use std::ffi::c_void;
use std::io::Write;
use std::path::Path;

use windows::core::{Error, Result, HRESULT, PCWSTR};
use windows::Win32::Foundation::{ERROR_CANCELLED, E_INVALIDARG};
use windows::Win32::Networking::WinHttp::*;

use crate::window::wide;

pub struct Response {
    pub status: u32,
    pub body: String,
}

/// Zadanie HTTPS z odpowiedzia tekstowa (JSON GitHuba).
pub fn request(method: &str, url: &str, headers: &[&str], body: Option<&str>) -> Result<Response> {
    let (status, bytes) = request_bytes(method, url, headers, body)?;
    Ok(Response {
        status,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// Zadanie HTTPS. `url` musi byc `https://host/sciezka`. Naglowki jako
/// gotowe linie `Nazwa: wartosc`. Limit ciala odpowiedzi 4 MB. Zwraca
/// (status, surowe bajty) - do obrazkow (avatar). Przekierowania sledzi
/// WinHTTP sam, jak dotad.
pub fn request_bytes(
    method: &str,
    url: &str,
    headers: &[&str],
    body: Option<&str>,
) -> Result<(u32, Vec<u8>)> {
    let req = Request::send(method, url, headers, body, true)?;
    let mut out = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    while let Some(n) = req.read(&mut buf)? {
        out.extend_from_slice(&buf[..n]);
        if out.len() > 4 * 1024 * 1024 {
            break;
        }
    }
    Ok((req.status, out))
}

/// Postep pobierania: (bajty odebrane, rozmiar calkowity, jesli serwer go
/// podal). Zwrot `false` przerywa pobieranie (`is_cancelled` na bledzie).
pub type Progress<'a> = &'a mut dyn FnMut(u64, Option<u64>) -> bool;

/// Ile przekierowan sledzimy przy pobieraniu pliku.
const MAX_REDIRECTS: usize = 5;

/// Pobiera plik pod `dest` (nadpisuje). Przekierowania obslugujemy sami:
/// GitHub odsyla zasoby wydan na inny host z podpisem w adresie, a WinHTTP
/// powtorzylby tam nasz naglowek `Authorization` - serwer odrzuca zadanie
/// z dwoma mechanizmami uwierzytelnienia. Przy zmianie hosta naglowki
/// `Authorization` odpadaja. Zwraca liczbe pobranych bajtow.
pub fn download(url: &str, headers: &[&str], dest: &Path, progress: Progress) -> Result<u64> {
    let mut url = url.to_string();
    let mut headers: Vec<&str> = headers.to_vec();
    for _ in 0..=MAX_REDIRECTS {
        let req = Request::send("GET", &url, &headers, None, false)?;
        match req.status {
            301 | 302 | 303 | 307 | 308 => {
                let next = req
                    .header(WINHTTP_QUERY_LOCATION)
                    .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
                let next = if next.starts_with("https://") {
                    next
                } else {
                    // Wzgledny `Location`: ten sam host.
                    format!("https://{}{}", host_of(&url)?, next)
                };
                if host_of(&next)? != host_of(&url)? {
                    headers.retain(|h| !h.to_ascii_lowercase().starts_with("authorization:"));
                }
                url = next;
            }
            200..=299 => {
                let total = req
                    .header(WINHTTP_QUERY_CONTENT_LENGTH)
                    .and_then(|s| s.trim().parse::<u64>().ok());
                let mut file = std::fs::File::create(dest).map_err(io_err)?;
                let mut buf = vec![0u8; 256 * 1024];
                let mut done = 0u64;
                if !progress(0, total) {
                    return Err(cancelled());
                }
                while let Some(n) = req.read(&mut buf)? {
                    file.write_all(&buf[..n]).map_err(io_err)?;
                    done += n as u64;
                    if !progress(done, total) {
                        drop(file);
                        let _ = std::fs::remove_file(dest);
                        return Err(cancelled());
                    }
                }
                file.flush().map_err(io_err)?;
                return Ok(done);
            }
            s => return Err(Error::new(status_hresult(s), format!("HTTP {s}"))),
        }
    }
    Err(Error::new(E_INVALIDARG, "too many redirects"))
}

/// Czy blad z `download` to przerwanie przez `Progress`.
pub fn is_cancelled(e: &Error) -> bool {
    e.code() == HRESULT::from_win32(ERROR_CANCELLED.0)
}

/// Status HTTP z bledu `download` (odpowiedz spoza 2xx), jesli to on.
pub fn http_status(e: &Error) -> Option<u32> {
    let s = (e.code().0 as u32).wrapping_sub(STATUS_BASE);
    (100..600).contains(&s).then_some(s)
}

/// Statusy HTTP jako HRESULT: FACILITY_ITF (kody wlasne aplikacji, od 0x200
/// w gore), przesuniete tak, by nie zderzyc sie z bledami Win32 i I/O.
const STATUS_BASE: u32 = 0x8004_0800;

fn status_hresult(status: u32) -> HRESULT {
    HRESULT((STATUS_BASE + status) as i32)
}

fn cancelled() -> Error {
    Error::from_hresult(HRESULT::from_win32(ERROR_CANCELLED.0))
}

fn io_err(e: std::io::Error) -> Error {
    Error::new(
        HRESULT::from_win32(e.raw_os_error().unwrap_or(0) as u32),
        e.to_string(),
    )
}

fn host_of(url: &str) -> Result<&str> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
    Ok(rest.split('/').next().unwrap_or(rest))
}

/// Wyslane zadanie z odebranym naglowkiem odpowiedzi; cialo czyta sie
/// `read`. Uchwyty zamykane w kolejnosci odwrotnej do otwarcia (pola
/// od ostatniego).
struct Request {
    status: u32,
    req: Handle,
    _connect: Handle,
    _session: Handle,
}

impl Request {
    fn send(
        method: &str,
        url: &str,
        headers: &[&str],
        body: Option<&str>,
        follow_redirects: bool,
    ) -> Result<Self> {
        let rest = url
            .strip_prefix("https://")
            .ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
        let (host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let agent = wide(concat!("SpectreNotes/", env!("CARGO_PKG_VERSION")));
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
            let session = Handle(session);
            let connect = WinHttpConnect(session.0, PCWSTR(host_w.as_ptr()), 443, 0);
            if connect.is_null() {
                return Err(Error::from_thread());
            }
            let connect = Handle(connect);
            let req = WinHttpOpenRequest(
                connect.0,
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
            let req = Handle(req);
            if !follow_redirects {
                let policy: u32 = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
                WinHttpSetOption(
                    Some(req.0),
                    WINHTTP_OPTION_REDIRECT_POLICY,
                    Some(&policy.to_ne_bytes()),
                )?;
            }
            for h in headers {
                let line = wide(&format!("{h}\r\n"));
                // Bez zera koncowego: dlugosc jawna.
                WinHttpAddRequestHeaders(req.0, &line[..line.len() - 1], WINHTTP_ADDREQ_FLAG_ADD)?;
            }
            let body_bytes = body.map(str::as_bytes).unwrap_or(&[]);
            WinHttpSendRequest(
                req.0,
                None,
                Some(body_bytes.as_ptr() as *const c_void),
                body_bytes.len() as u32,
                body_bytes.len() as u32,
                0,
            )?;
            WinHttpReceiveResponse(req.0, std::ptr::null_mut())?;

            let mut status: u32 = 0;
            let mut len = std::mem::size_of::<u32>() as u32;
            WinHttpQueryHeaders(
                req.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some(&mut status as *mut u32 as *mut c_void),
                &mut len,
                std::ptr::null_mut(),
            )?;
            Ok(Self {
                status,
                req,
                _connect: connect,
                _session: session,
            })
        }
    }

    /// Naglowek odpowiedzi jako tekst (`None` = brak).
    fn header(&self, which: u32) -> Option<String> {
        let mut buf = [0u16; 2048];
        let mut len = (buf.len() * 2) as u32;
        let ok = unsafe {
            WinHttpQueryHeaders(
                self.req.0,
                which,
                PCWSTR::null(),
                Some(buf.as_mut_ptr() as *mut c_void),
                &mut len,
                std::ptr::null_mut(),
            )
        };
        ok.ok()?;
        Some(String::from_utf16_lossy(&buf[..(len as usize / 2).min(buf.len())]))
    }

    /// Kolejny kawalek ciala do `buf`; `None` = koniec.
    fn read(&self, buf: &mut [u8]) -> Result<Option<usize>> {
        unsafe {
            let mut avail: u32 = 0;
            WinHttpQueryDataAvailable(self.req.0, &mut avail)?;
            if avail == 0 {
                return Ok(None);
            }
            let want = (avail as usize).min(buf.len()) as u32;
            let mut read: u32 = 0;
            WinHttpReadData(self.req.0, buf.as_mut_ptr() as *mut c_void, want, &mut read)?;
            Ok(Some(read as usize))
        }
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

    #[test]
    fn host_z_adresu() {
        assert_eq!(
            host_of("https://api.github.com/repos/x").unwrap(),
            "api.github.com"
        );
        assert_eq!(
            host_of("https://objects.githubusercontent.com").unwrap(),
            "objects.githubusercontent.com"
        );
        assert!(host_of("http://x").is_err());
    }

    #[test]
    fn status_z_bledu() {
        let e = Error::new(status_hresult(404), "HTTP 404");
        assert_eq!(http_status(&e), Some(404));
        assert!(!is_cancelled(&e));
        assert!(is_cancelled(&cancelled()));
        assert_eq!(http_status(&cancelled()), None);
    }
}
