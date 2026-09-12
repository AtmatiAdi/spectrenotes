//! GitHub bez zewnetrznych narzedzi: logowanie Device Flow (OAuth) i REST API
//! do wykrycia/zalozenia prywatnego repozytorium space'u. HTTP przez WinHTTP
//! (`spectre_shell_win::http`), JSON parsowany recznie - odpowiedzi maja
//! kilka plaskich pol, serde bylby najwiekszym crate'em w binarce.
//!
//! Device Flow: aplikacja prosi GitHub o kod, pokazuje go uzytkownikowi,
//! otwiera `github.com/login/device`, a potem odpytuje, az uzytkownik
//! zatwierdzi. Wymaga `client_id` aplikacji OAuth zarejestrowanej na GitHubie
//! z wlaczonym Device Flow (`CLIENT_ID`, nadpisywalne w `config.txt`:
//! `github_client_id=`). Bez client_id zostaje wklejenie tokenu (PAT).

use std::time::{Duration, Instant};

use spectre_shell_win::http;
use spectre_sync::GitError;

/// OAuth App "SpectreNotes" - do uzupelnienia po rejestracji na GitHubie
/// (Settings -> Developer settings -> OAuth Apps, "Enable Device Flow").
pub const CLIENT_ID: &str = "";
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
        401 => Err(GitError::Auth(format!("{what}: token odrzucony (401)"))),
        403 | 429 => {
            let lower = r.body.to_ascii_lowercase();
            if r.status == 429 || lower.contains("rate limit") || lower.contains("abuse") {
                Err(GitError::RateLimited(format!(
                    "{what}: GitHub zglasza limit ({})",
                    r.status
                )))
            } else {
                Err(GitError::Auth(format!("{what}: brak uprawnien (403)")))
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
        .ok_or_else(|| GitError::Other("device/code: brak device_code".into()))?;
    let user_code = json_str(&r.body, "user_code")
        .ok_or_else(|| GitError::Other("device/code: brak user_code".into()))?;
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
            return Err(GitError::Other(
                "logowanie: kod wygasl, sprobuj ponownie".into(),
            ));
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
                return Err(GitError::Other("logowanie: kod wygasl".into()));
            }
            Some("access_denied") => {
                return Err(GitError::Other("logowanie: odmowa w przegladarce".into()));
            }
            Some(e) => return Err(GitError::Other(format!("logowanie: {e}"))),
            None => check_status(&r, "oauth/access_token")?,
        }
    }
}

fn auth_header(token: &str) -> String {
    format!("Authorization: Bearer {token}")
}

/// Login zalogowanego uzytkownika.
pub fn user_login(token: &str) -> Result<String> {
    let r = http::request(
        "GET",
        &format!("{API}/user"),
        &[UA, ACCEPT_API, &auth_header(token)],
        None,
    )
    .map_err(map_http)?;
    check_status(&r, "user")?;
    json_str(&r.body, "login").ok_or_else(|| GitError::Other("user: brak login".into()))
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
         \"description\":\"SpectreNotes space - notatki (op-log CRDT)\"}}"
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

/// Wartosc tekstowa pola `"key": "..."` z plaskiego JSON-a (bez zagniezdzen
/// o tej samej nazwie). Obsluguje `\"`, `\\`, `\/`, `\n`, `\uXXXX`.
pub fn json_str(body: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let mut from = 0;
    while let Some(i) = body[from..].find(&pat) {
        let after = &body[from + i + pat.len()..];
        let after = after.trim_start();
        if let Some(rest) = after.strip_prefix(':') {
            let rest = rest.trim_start();
            if let Some(s) = rest.strip_prefix('"') {
                let mut out = String::new();
                let mut chars = s.chars();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => return Some(out),
                        '\\' => match chars.next()? {
                            'n' => out.push('\n'),
                            't' => out.push('\t'),
                            'u' => {
                                let hex: String = chars.by_ref().take(4).collect();
                                if let Some(ch) =
                                    u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                                {
                                    out.push(ch);
                                }
                            }
                            other => out.push(other),
                        },
                        c => out.push(c),
                    }
                }
                return None;
            }
            return None;
        }
        from += i + pat.len();
    }
    None
}

pub fn json_u64(body: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\"");
    let i = body.find(&pat)?;
    let rest = body[i + pat.len()..]
        .trim_start()
        .strip_prefix(':')?
        .trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
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
    fn json_plaski() {
        let body = r#"{"device_code":"abc\"d","user_code":"WDJB-MJHT","interval":5,"expires_in":899,"login":"AtmatiAdi","x":{"login":"inny"}}"#;
        assert_eq!(json_str(body, "device_code").as_deref(), Some("abc\"d"));
        assert_eq!(json_str(body, "user_code").as_deref(), Some("WDJB-MJHT"));
        assert_eq!(json_u64(body, "interval"), Some(5));
        assert_eq!(json_u64(body, "expires_in"), Some(899));
        assert_eq!(json_str(body, "login").as_deref(), Some("AtmatiAdi"));
        assert_eq!(json_str(body, "brak"), None);
        assert_eq!(json_str(r#"{"a":"A\n"}"#, "a").as_deref(), Some("A\n"));
    }

    #[test]
    fn nazwa_repo() {
        assert_eq!(repo_name("default"), "spectrenotes-default");
        assert_eq!(repo_name("Moje Notatki!"), "spectrenotes-moje-notatki-");
        assert_eq!(repo_name(""), "spectrenotes-space");
    }
}
