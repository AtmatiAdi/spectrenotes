//! GitHub bez zewnetrznych narzedzi: logowanie Device Flow (OAuth) i REST API
//! do wykrycia/zalozenia prywatnego repozytorium space'u. HTTP przez WinHTTP
//! (`spectre_shell_win::http`), JSON parsowany recznie - odpowiedzi maja
//! kilka plaskich pol, serde bylby najwiekszym crate'em w binarce.
//!
//! Logowanie, od najwygodniejszego:
//! 1. **Przegladarka z powrotem do aplikacji** (OAuth web flow): nasluch na
//!    loopbacku, `Authorize` na GitHubie, przegladarka wraca z kodem, kod
//!    wymieniany na token. Wymaga `CLIENT_ID` i `CLIENT_SECRET`.
//! 2. **Device Flow**: kod (w schowku) do wpisania na `github.com/login/device`,
//!    aplikacja odpytuje, az uzytkownik zatwierdzi. Wymaga tylko `CLIENT_ID`.
//! 3. **Wklejony token (PAT)** - zawsze dostepne.

use std::time::{Duration, Instant};

use spectre_shell_win::http;
use spectre_sync::GitError;
pub use spectre_update::json::{json_str, json_u64};

/// OAuth App "SpectreNotes" (konto AtmatiAdi; Settings -> Developer settings ->
/// OAuth Apps: adres zwrotny `http://127.0.0.1/callback`, Device Flow wlaczony,
/// tokeny bez wygasania). Client ID jest publiczny. Nadpisywalne w `config.txt`:
/// `github_client_id=`.
pub const CLIENT_ID: &str = "Ov23liM9ywOxPBWJ4gWU";
/// Sekret aplikacji - potrzebny tylko do wymiany kodu z przegladarki na token
/// (GitHub nie robi tu PKCE bez sekretu). **Nie w repozytorium**: `build.rs`
/// wkleja go z pliku `%USERPROFILE%\.spectrenotes-oauth-secret` (albo ze
/// zmiennej `SPECTRENOTES_OAUTH_SECRET`) przy budowaniu; bez niego zostaje
/// Device Flow (kod w schowku). Nadpisywalne: `github_client_secret=`.
pub const CLIENT_SECRET: &str = match option_env!("SPECTRENOTES_OAUTH_SECRET") {
    Some(s) => s,
    None => "",
};
const API: &str = "https://api.github.com";
const UA: &str = "User-Agent: SpectreNotes";
const ACCEPT_JSON: &str = "Accept: application/json";
const ACCEPT_API: &str = "Accept: application/vnd.github+json";

pub type Result<T> = std::result::Result<T, GitError>;

/// Rozpoczete logowanie: kod do wpisania i dane do odpytywania.
#[derive(Debug, Clone)]
pub struct Device {
    pub user_code: String,
    pub verification_uri: String,
    device_code: String,
    interval: Duration,
    expires_at: Instant,
}

fn map_http(e: windows::core::Error) -> GitError {
    GitError::Other(format!("HTTP: {e}"))
}

fn check_status(r: &http::Response, what: &str) -> Result<()> {
    match r.status {
        200..=299 => Ok(()),
        401 => Err(GitError::Auth(format!("{what}: token rejected (401)"))),
        403 | 429 => {
            let lower = r.body.to_ascii_lowercase();
            if r.status == 429 || lower.contains("rate limit") || lower.contains("abuse") {
                Err(GitError::RateLimited(format!(
                    "{what}: GitHub reports a rate limit ({})",
                    r.status
                )))
            } else {
                Err(GitError::Auth(format!("{what}: no permission (403)")))
            }
        }
        s => Err(GitError::Other(format!(
            "{what}: HTTP {s}: {}",
            json_str(&r.body, "message").unwrap_or_else(|| one_line(&r.body))
        ))),
    }
}

/// Krok 1 Device Flow: kod dla uzytkownika.
pub fn device_start(client_id: &str) -> Result<Device> {
    let body = format!("client_id={client_id}&scope=repo");
    let r = http::request(
        "POST",
        "https://github.com/login/device/code",
        &[
            UA,
            ACCEPT_JSON,
            "Content-Type: application/x-www-form-urlencoded",
        ],
        Some(&body),
    )
    .map_err(map_http)?;
    check_status(&r, "device/code")?;
    let device_code = json_str(&r.body, "device_code")
        .ok_or_else(|| GitError::Other("device/code: no device_code".into()))?;
    let user_code = json_str(&r.body, "user_code")
        .ok_or_else(|| GitError::Other("device/code: no user_code".into()))?;
    let verification_uri = json_str(&r.body, "verification_uri")
        .unwrap_or_else(|| "https://github.com/login/device".to_string());
    let interval = json_u64(&r.body, "interval").unwrap_or(5).max(1);
    let expires_in = json_u64(&r.body, "expires_in").unwrap_or(900);
    Ok(Device {
        user_code,
        verification_uri,
        device_code,
        interval: Duration::from_secs(interval),
        expires_at: Instant::now() + Duration::from_secs(expires_in),
    })
}

/// Krok 2: odpytywanie do skutku. Blokuje (do `expires_in`, zwykle 15 min) -
/// tylko z osobnego watku. Zwraca token.
pub fn device_wait(client_id: &str, d: &Device) -> Result<String> {
    let mut interval = d.interval;
    loop {
        if Instant::now() >= d.expires_at {
            return Err(GitError::Other("sign-in: code expired, try again".into()));
        }
        std::thread::sleep(interval);
        let body = format!(
            "client_id={client_id}&device_code={}&grant_type=urn:ietf:params:oauth:grant-type:device_code",
            d.device_code
        );
        let r = http::request(
            "POST",
            "https://github.com/login/oauth/access_token",
            &[
                UA,
                ACCEPT_JSON,
                "Content-Type: application/x-www-form-urlencoded",
            ],
            Some(&body),
        )
        .map_err(map_http)?;
        if let Some(token) = json_str(&r.body, "access_token") {
            return Ok(token);
        }
        match json_str(&r.body, "error").as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += Duration::from_secs(5),
            Some("expired_token") => {
                return Err(GitError::Other("sign-in: code expired".into()));
            }
            Some("access_denied") => {
                return Err(GitError::Other("sign-in: denied in the browser".into()));
            }
            Some(e) => return Err(GitError::Other(format!("sign-in: {e}"))),
            None => check_status(&r, "oauth/access_token")?,
        }
    }
}

/// Logowanie przez przegladarke z powrotem do aplikacji (OAuth web flow):
/// nasluch na `127.0.0.1:<port>`, `github.com/login/oauth/authorize`,
/// przegladarka wraca z `code` na `/callback`, kod wymieniamy na token.
/// GitHub przy adresie zwrotnym loopback przyjmuje dowolny port, wiec w
/// aplikacji OAuth wystarczy zarejestrowany `http://127.0.0.1/callback`.
pub struct BrowserLogin {
    pub url: String,
    listener: std::net::TcpListener,
    redirect_uri: String,
    state: String,
}

/// Krok 1: port, adres do otwarcia w przegladarce.
pub fn browser_login_start(client_id: &str) -> Result<BrowserLogin> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| GitError::Other(format!("sign-in: local port: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| GitError::Other(format!("sign-in: local port: {e}")))?
        .port();
    listener
        .set_nonblocking(true)
        .map_err(|e| GitError::Other(format!("sign-in: listener: {e}")))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let state = format!("{:x}{:x}", nanos, std::process::id());
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={client_id}&redirect_uri={}&scope=repo&state={state}",
        url_encode(&redirect_uri)
    );
    Ok(BrowserLogin {
        url,
        listener,
        redirect_uri,
        state,
    })
}

/// Krok 2: czeka na powrot przegladarki (do 5 min), odpowiada jej strona
/// "mozesz zamknac karte" i wymienia kod na token. Blokuje - z osobnego watku.
pub fn browser_login_wait(
    client_id: &str,
    client_secret: &str,
    l: &BrowserLogin,
) -> Result<String> {
    use std::io::{Read, Write};
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut stream = loop {
        match l.listener.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(GitError::Other(
                        "sign-in: no answer from the browser in 5 minutes".into(),
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(GitError::Other(format!("sign-in: listener: {e}"))),
        }
    };
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    let request = String::from_utf8_lossy(&buf);
    let target = request
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("");
    let result = callback_code(target, &l.state);
    let (title, text) = match &result {
        Ok(_) => (
            "Signed in",
            "SpectreNotes is signed in to GitHub. You can close this tab.".to_string(),
        ),
        Err(e) => ("Sign-in failed", e.to_string()),
    };
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>SpectreNotes</title></head>\
         <body style=\"background:#121212;color:#bebebe;font:16px 'Segoe UI',sans-serif;\
         display:flex;align-items:center;justify-content:center;height:100vh;margin:0\">\
         <div style=\"text-align:center\"><div style=\"font-size:28px;color:#73a08c;margin-bottom:12px\">{title}</div>{text}</div>\
         </body></html>"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.flush();
    drop(stream);
    let code = result?;
    let body = format!(
        "client_id={client_id}&client_secret={client_secret}&code={}&redirect_uri={}",
        url_encode(&code),
        url_encode(&l.redirect_uri)
    );
    let r = http::request(
        "POST",
        "https://github.com/login/oauth/access_token",
        &[
            UA,
            ACCEPT_JSON,
            "Content-Type: application/x-www-form-urlencoded",
        ],
        Some(&body),
    )
    .map_err(map_http)?;
    if let Some(token) = json_str(&r.body, "access_token") {
        return Ok(token);
    }
    match json_str(&r.body, "error_description").or_else(|| json_str(&r.body, "error")) {
        Some(e) => Err(GitError::Other(format!("sign-in: {e}"))),
        None => {
            check_status(&r, "oauth/access_token")?;
            Err(GitError::Other(
                "sign-in: no access_token in the answer".into(),
            ))
        }
    }
}

/// Kod z adresu, na ktory wrocila przegladarka (`/callback?code=..&state=..`);
/// `state` musi byc nasz - inaczej to nie jest odpowiedz na nasze pytanie.
fn callback_code(target: &str, state: &str) -> Result<String> {
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let param = |name: &str| {
        query
            .split('&')
            .find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == name))
            .map(|(_, v)| url_decode(v))
    };
    match (param("code"), param("state"), param("error")) {
        (Some(code), Some(s), _) if s == state => Ok(code),
        (_, _, Some(e)) => Err(GitError::Other(format!(
            "sign-in: {}",
            param("error_description").unwrap_or(e)
        ))),
        (Some(_), _, _) => Err(GitError::Other("sign-in: state mismatch".into())),
        _ => Err(GitError::Other("sign-in: no code in the callback".into())),
    }
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(v) => {
                    out.push(v);
                    i += 3;
                }
                Err(_) => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn auth_header(token: &str) -> String {
    format!("Authorization: Bearer {token}")
}

/// Zalogowany uzytkownik: login i adres avataru.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub login: String,
    pub avatar_url: String,
}

/// Kto jest zalogowany (i skad wziac avatar).
pub fn user_info(token: &str) -> Result<User> {
    let r = http::request(
        "GET",
        &format!("{API}/user"),
        &[UA, ACCEPT_API, &auth_header(token)],
        None,
    )
    .map_err(map_http)?;
    check_status(&r, "user")?;
    let login =
        json_str(&r.body, "login").ok_or_else(|| GitError::Other("user: no login".into()))?;
    Ok(User {
        login,
        avatar_url: json_str(&r.body, "avatar_url").unwrap_or_default(),
    })
}

/// Login zalogowanego uzytkownika.
pub fn user_login(token: &str) -> Result<String> {
    user_info(token).map(|u| u.login)
}

/// Bok avataru pobieranego z GitHuba (`?s=`); w menu ma 44 px.
pub const AVATAR_PX: u32 = 96;

/// Surowe bajty avataru (PNG/JPEG) w rozmiarze `AVATAR_PX`. Zwraca tez ile
/// bajtow przyszlo - do budzetu ruchu.
pub fn fetch_avatar(avatar_url: &str) -> Result<Vec<u8>> {
    if avatar_url.is_empty() {
        return Err(GitError::Other("avatar: no url".into()));
    }
    let sep = if avatar_url.contains('?') { '&' } else { '?' };
    let url = format!("{avatar_url}{sep}s={AVATAR_PX}");
    let (status, bytes) = http::request_bytes("GET", &url, &[UA], None).map_err(map_http)?;
    if !(200..=299).contains(&status) {
        return Err(GitError::Other(format!("avatar: HTTP {status}")));
    }
    Ok(bytes)
}

/// Prywatne repo `login/name`: istniejace albo zalozone. Zwraca URL do klonowania.
pub fn ensure_repo(token: &str, login: &str, name: &str) -> Result<String> {
    let headers = [UA, ACCEPT_API, &auth_header(token)];
    let r = http::request(
        "GET",
        &format!("{API}/repos/{login}/{name}"),
        &headers,
        None,
    )
    .map_err(map_http)?;
    if r.status == 200 {
        return Ok(json_str(&r.body, "clone_url")
            .unwrap_or_else(|| format!("https://github.com/{login}/{name}.git")));
    }
    if r.status != 404 {
        check_status(&r, "repos")?;
    }
    let body = format!(
        "{{\"name\":\"{name}\",\"private\":true,\"auto_init\":false,\
         \"description\":\"SpectreNotes space - notes (op-log CRDT)\"}}"
    );
    let r = http::request(
        "POST",
        &format!("{API}/user/repos"),
        &[
            UA,
            ACCEPT_API,
            &auth_header(token),
            "Content-Type: application/json",
        ],
        Some(&body),
    )
    .map_err(map_http)?;
    check_status(&r, "user/repos")?;
    Ok(json_str(&r.body, "clone_url")
        .unwrap_or_else(|| format!("https://github.com/{login}/{name}.git")))
}

/// Nazwa repozytorium dla space'u: unikalna w obrebie konta, czytelna.
pub fn repo_name(space_dir_name: &str) -> String {
    let slug: String = space_dir_name
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!(
        "spectrenotes-{}",
        if slug.is_empty() { "space" } else { &slug }
    )
}

fn one_line(s: &str) -> String {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .chars()
        .take(120)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kod_z_powrotu_przegladarki() {
        assert_eq!(
            callback_code("/callback?code=abc%2F1&state=s1", "s1").unwrap(),
            "abc/1"
        );
        assert!(callback_code("/callback?code=abc&state=zle", "s1")
            .unwrap_err()
            .to_string()
            .contains("state"));
        assert!(callback_code(
            "/callback?error=access_denied&error_description=The+user+denied&state=s1",
            "s1"
        )
        .unwrap_err()
        .to_string()
        .contains("The user denied"));
        assert!(callback_code("/favicon.ico", "s1").is_err());
        assert_eq!(
            url_encode("http://127.0.0.1:5/callback"),
            "http%3A%2F%2F127.0.0.1%3A5%2Fcallback"
        );
    }

    #[test]
    fn nazwa_repo() {
        assert_eq!(repo_name("default"), "spectrenotes-default");
        assert_eq!(repo_name("Moje Notatki!"), "spectrenotes-moje-notatki-");
        assert_eq!(repo_name(""), "spectrenotes-space");
    }
}
