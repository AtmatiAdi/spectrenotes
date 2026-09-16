//! Aktualizacje z wydan GitHuba (docs/06-DYSTRYBUCJA). Wspolne dla
//! aplikacji (sprawdzanie w tle, pobieranie z postepem, podmiana binarki)
//! i instalatora (`SpectreNotes-Setup.exe`, ktory pobiera najnowsze wydanie).
//!
//! Wydanie to tag `vX.Y.Z` z zasobami o stalych nazwach (`ASSET_*`), zeby
//! `releases/latest/download/<nazwa>` byl trwalym adresem. Pobrany plik
//! sprawdzamy z `SHA256SUMS.txt` z tego samego wydania - bez sumy nie
//! instalujemy.
//!
//! Token jest opcjonalny: publiczne repozytorium odpowiada anonimowo,
//! prywatne wymaga tokenu z dostepem (w aplikacji - token logowania GitHub).
//! Z tokenem zasoby idą przez API (`Accept: application/octet-stream`),
//! bez niego przez `browser_download_url`; oba przekierowuja na CDN.

pub mod json;

use std::fmt;
use std::path::{Path, PathBuf};

use json::{json_bool, json_str, json_u64};
use spectre_shell_win::http::{self, Progress};

/// Wersja tej kompilacji (wspolna dla workspace'u).
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Zasoby wydania - nazwy stale miedzy wersjami.
pub const ASSET_EXE: &str = "spectrenotes.exe";
pub const ASSET_SETUP: &str = "SpectreNotes-Setup.exe";
pub const ASSET_SUMS: &str = "SHA256SUMS.txt";

const API: &str = "https://api.github.com";
const UA: &str = concat!("User-Agent: SpectreNotes/", env!("CARGO_PKG_VERSION"));
const ACCEPT_API: &str = "Accept: application/vnd.github+json";
const ACCEPT_BIN: &str = "Accept: application/octet-stream";

/// Publiczne repozytorium **wydan** - osobne od prywatnych zrodel (`repository`
/// w Cargo.toml), zeby instalator i aktualizacje dzialaly bez tokenu u kazdego.
/// Jedno zrodlo prawdy: `release.ps1` czyta te stala z tego pliku.
pub const RELEASES_REPO: &str = "AtmatiAdi/spectrenotes-releases";

/// `owner/repo`, z ktorego bierzemy wydania.
pub fn default_repo() -> &'static str {
    RELEASES_REPO
}

/// `https://github.com/owner/repo[.git][/]` -> `owner/repo`.
pub fn repo_from_url(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    (rest.matches('/').count() == 1).then_some(rest)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// `v1.2.3`, `1.2.3`, `1.2` (=1.2.0), `1.2.3-beta` (sufiks pomijany).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let core = s.split(['-', '+']).next()?;
        let mut it = core.split('.').map(|p| p.parse::<u32>());
        let a = it.next()?.ok()?;
        let b = it.next().unwrap_or(Ok(0)).ok()?;
        let c = it.next().unwrap_or(Ok(0)).ok()?;
        if it.next().is_some() {
            return None;
        }
        Some(Self(a, b, c))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    /// Adres w API (`/releases/assets/<id>`) - z tokenem, dla prywatnych repo.
    pub api_url: String,
    /// Adres publiczny (`/releases/download/<tag>/<nazwa>`).
    pub browser_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    pub name: String,
    /// Opis wydania (markdown) - pierwsze linie pokazujemy w menu.
    pub notes: String,
    pub html_url: String,
    pub assets: Vec<Asset>,
}

impl Release {
    /// Z odpowiedzi `GET /repos/{repo}/releases/latest`.
    pub fn parse(body: &str) -> Result<Self> {
        let tag = json_str(body, "tag_name").ok_or_else(|| Error::Parse("tag_name".into()))?;
        let version = Version::parse(&tag)
            .ok_or_else(|| Error::Parse(format!("tag `{tag}` is not vX.Y.Z")))?;
        let assets = parse_assets(body);
        Ok(Self {
            version,
            name: json_str(body, "name").unwrap_or_else(|| tag.clone()),
            tag,
            notes: json_str(body, "body").unwrap_or_default(),
            html_url: json_str(body, "html_url").unwrap_or_default(),
            assets,
        })
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// Czy to wydanie jest nowsze niz `current` (np. `CURRENT`).
    pub fn is_newer_than(&self, current: &str) -> bool {
        match Version::parse(current) {
            Some(c) => self.version > c,
            None => true,
        }
    }

    /// Pierwsza niepusta linia opisu, bez markdownowych ozdobnikow.
    pub fn headline(&self) -> String {
        self.notes
            .lines()
            .map(|l| l.trim().trim_start_matches(['#', '-', '*']).trim())
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .to_string()
    }
}

/// Zasoby z tablicy `"assets": [...]`. Kazdy obiekt zaczyna sie od `"url"`
/// i konczy na `"browser_download_url"`; `"name"` jest przed zagniezdzonym
/// `uploader` (ktory ma `login`, nie `name`), `size` po nim.
fn parse_assets(body: &str) -> Vec<Asset> {
    let Some(start) = body.find("\"assets\"") else {
        return Vec::new();
    };
    let end = body[start..]
        .find("\"tarball_url\"")
        .map(|i| start + i)
        .unwrap_or(body.len());
    let section = &body[start..end];
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(i) = section[pos..].find("\"name\"") {
        let name_at = pos + i;
        let rest = &section[name_at..];
        let Some(name) = json_str(rest, "name") else {
            break;
        };
        let Some(bdu_rel) = rest.find("\"browser_download_url\"") else {
            break;
        };
        let api_url = section[..name_at]
            .rfind("\"url\"")
            .and_then(|u| json_str(&section[u..], "url"))
            .unwrap_or_default();
        out.push(Asset {
            size: json_u64(&rest[..bdu_rel], "size").unwrap_or(0),
            browser_url: json_str(&rest[bdu_rel..], "browser_download_url").unwrap_or_default(),
            api_url,
            name,
        });
        pos = name_at + bdu_rel;
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Brak wydan albo brak dostepu (prywatne repo bez tokenu daje 404).
    NotFound,
    Auth,
    RateLimited,
    Http(String),
    Cancelled,
    /// Suma z `SHA256SUMS.txt` nie zgadza sie z pobranym plikiem (albo jej brak).
    Checksum(String),
    Io(String),
    Parse(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound => write!(f, "no release found or repository private"),
            Error::Auth => write!(f, "GitHub rejected the token"),
            Error::RateLimited => write!(f, "GitHub rate limit - try again later"),
            Error::Http(s) => write!(f, "{s}"),
            Error::Cancelled => write!(f, "cancelled"),
            Error::Checksum(s) => write!(f, "checksum: {s}"),
            Error::Io(s) => write!(f, "{s}"),
            Error::Parse(s) => write!(f, "unexpected response: {s}"),
        }
    }
}

impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        if http::is_cancelled(&e) {
            return Error::Cancelled;
        }
        match http::http_status(&e) {
            Some(404) => Error::NotFound,
            Some(401) => Error::Auth,
            Some(403) | Some(429) => Error::RateLimited,
            Some(s) => Error::Http(format!("HTTP {s}")),
            None => Error::Http(format!("network: {}", e.message())),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Dostep do wydan jednego repozytorium.
pub struct Client<'a> {
    pub repo: &'a str,
    pub token: Option<&'a str>,
}

impl<'a> Client<'a> {
    pub fn new(repo: &'a str, token: Option<&'a str>) -> Self {
        Self { repo, token }
    }

    fn auth(&self) -> Option<String> {
        self.token
            .filter(|t| !t.is_empty())
            .map(|t| format!("Authorization: Bearer {t}"))
    }

    /// Najnowsze wydanie (bez szkicow i przedpremier - tak dziala `latest`).
    pub fn latest(&self) -> Result<Release> {
        let auth = self.auth();
        let mut headers = vec![UA, ACCEPT_API];
        if let Some(a) = &auth {
            headers.push(a);
        }
        let url = format!("{API}/repos/{}/releases/latest", self.repo);
        let r = http::request("GET", &url, &headers, None)
            .map_err(|e| Error::Http(format!("network: {}", e.message())))?;
        match r.status {
            200 => {}
            404 => return Err(Error::NotFound),
            401 => return Err(Error::Auth),
            403 | 429 => return Err(Error::RateLimited),
            s => {
                return Err(Error::Http(format!(
                    "HTTP {s}: {}",
                    json_str(&r.body, "message").unwrap_or_default()
                )))
            }
        }
        if json_bool(&r.body, "draft") == Some(true) {
            return Err(Error::NotFound);
        }
        Release::parse(&r.body)
    }

    /// Pobiera zasob do `dest` z postepem. Z tokenem - przez API (dziala dla
    /// prywatnych repo), bez - adresem publicznym.
    pub fn download(&self, asset: &Asset, dest: &Path, progress: Progress) -> Result<u64> {
        let auth = self.auth();
        let (url, mut headers) = match &auth {
            Some(a) if !asset.api_url.is_empty() => {
                (asset.api_url.as_str(), vec![UA, ACCEPT_BIN, a.as_str()])
            }
            _ => (asset.browser_url.as_str(), vec![UA]),
        };
        if url.is_empty() {
            return Err(Error::Parse(format!("asset {} has no url", asset.name)));
        }
        headers.push("Accept-Encoding: identity");
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(http::download(url, &headers, dest, progress)?)
    }

    /// Tresc `SHA256SUMS.txt` z wydania.
    pub fn fetch_sums(&self, release: &Release) -> Result<String> {
        let sums = release
            .asset(ASSET_SUMS)
            .ok_or_else(|| Error::Checksum(format!("release has no {ASSET_SUMS}")))?;
        let tmp =
            std::env::temp_dir().join(format!("spectrenotes-sums-{}.txt", std::process::id()));
        self.download(sums, &tmp, &mut |_, _| true)?;
        let text = std::fs::read_to_string(&tmp)?;
        let _ = std::fs::remove_file(&tmp);
        Ok(text)
    }

    /// Pobiera zasob `name` do `dest` i sprawdza sume z `SHA256SUMS.txt`.
    /// Przy niezgodnosci plik jest kasowany.
    pub fn fetch_verified(
        &self,
        release: &Release,
        name: &str,
        dest: &Path,
        progress: Progress,
    ) -> Result<PathBuf> {
        let asset = release
            .asset(name)
            .ok_or_else(|| Error::Parse(format!("release {} has no {name}", release.tag)))?;
        let sums = self.fetch_sums(release)?;
        let want = sum_for(&sums, name)
            .ok_or_else(|| Error::Checksum(format!("{ASSET_SUMS} has no entry for {name}")))?;
        self.download(asset, dest, progress)?;
        let got = spectre_shell_win::hash::sha256_file(dest)?;
        if !got.eq_ignore_ascii_case(&want) {
            let _ = std::fs::remove_file(dest);
            return Err(Error::Checksum(format!(
                "{name}: expected {want}, got {got}"
            )));
        }
        Ok(dest.to_path_buf())
    }
}

/// Suma dla `name` z tekstu w formacie `sha256sum` (`hex  nazwa` / `hex *nazwa`).
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let hex = it.next()?;
        let file = it.next()?.trim_start_matches('*');
        (file.eq_ignore_ascii_case(name) && hex.len() == 64).then(|| hex.to_ascii_lowercase())
    })
}

/// `1 234 567` -> `1.2 MB` (do paska postepu).
pub fn human_bytes(b: u64) -> String {
    const K: f64 = 1024.0;
    let b = b as f64;
    if b >= K * K {
        format!("{:.1} MB", b / K / K)
    } else if b >= K {
        format!("{:.0} KB", b / K)
    } else {
        format!("{b:.0} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: &str = r###"{
  "url": "https://api.github.com/repos/AtmatiAdi/spectrenotes/releases/1",
  "assets_url": "https://api.github.com/repos/AtmatiAdi/spectrenotes/releases/1/assets",
  "html_url": "https://github.com/AtmatiAdi/spectrenotes/releases/tag/v0.2.0",
  "id": 1,
  "author": { "login": "AtmatiAdi", "html_url": "https://github.com/AtmatiAdi", "type": "User" },
  "tag_name": "v0.2.0",
  "target_commitish": "main",
  "name": "SpectreNotes 0.2.0",
  "draft": false,
  "prerelease": false,
  "assets": [
    {
      "url": "https://api.github.com/repos/AtmatiAdi/spectrenotes/releases/assets/11",
      "id": 11,
      "name": "SHA256SUMS.txt",
      "label": "",
      "uploader": { "login": "AtmatiAdi", "url": "https://api.github.com/users/AtmatiAdi" },
      "content_type": "text/plain",
      "state": "uploaded",
      "size": 210,
      "download_count": 0,
      "browser_download_url": "https://github.com/AtmatiAdi/spectrenotes/releases/download/v0.2.0/SHA256SUMS.txt"
    },
    {
      "url": "https://api.github.com/repos/AtmatiAdi/spectrenotes/releases/assets/12",
      "id": 12,
      "name": "spectrenotes.exe",
      "label": null,
      "uploader": { "login": "AtmatiAdi", "url": "https://api.github.com/users/AtmatiAdi" },
      "content_type": "application/x-msdownload",
      "state": "uploaded",
      "size": 2386944,
      "download_count": 3,
      "browser_download_url": "https://github.com/AtmatiAdi/spectrenotes/releases/download/v0.2.0/spectrenotes.exe"
    }
  ],
  "tarball_url": "https://api.github.com/repos/AtmatiAdi/spectrenotes/tarball/v0.2.0",
  "zipball_url": "https://api.github.com/repos/AtmatiAdi/spectrenotes/zipball/v0.2.0",
  "body": "## Zmiany\r\n\r\n- Pelny ekran melduje sie powloce\r\n- Panel menu omija pasek"
}"###;

    #[test]
    fn wersje() {
        assert_eq!(Version::parse("v1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("0.1"), Some(Version(0, 1, 0)));
        assert_eq!(Version::parse("1.2.3-beta+5"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("latest"), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert!(Version(0, 2, 0) > Version(0, 1, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        assert_eq!(Version(0, 2, 0).to_string(), "0.2.0");
    }

    #[test]
    fn repo_z_adresu() {
        assert_eq!(
            repo_from_url("https://github.com/AtmatiAdi/spectrenotes"),
            Some("AtmatiAdi/spectrenotes")
        );
        assert_eq!(repo_from_url("https://github.com/A/b.git/"), Some("A/b"));
        assert_eq!(repo_from_url("https://gitlab.com/a/b"), None);
        assert_eq!(default_repo(), "AtmatiAdi/spectrenotes-releases");
        assert!(repo_from_url(&format!("https://github.com/{RELEASES_REPO}")).is_some());
    }

    #[test]
    fn wydanie_z_json() {
        let r = Release::parse(RELEASE).unwrap();
        assert_eq!(r.version, Version(0, 2, 0));
        assert_eq!(r.tag, "v0.2.0");
        assert_eq!(r.name, "SpectreNotes 0.2.0");
        assert_eq!(
            r.html_url,
            "https://github.com/AtmatiAdi/spectrenotes/releases/tag/v0.2.0"
        );
        assert_eq!(r.assets.len(), 2);
        let exe = r.asset("spectrenotes.exe").unwrap();
        assert_eq!(exe.size, 2_386_944);
        assert_eq!(
            exe.api_url,
            "https://api.github.com/repos/AtmatiAdi/spectrenotes/releases/assets/12"
        );
        assert_eq!(
            exe.browser_url,
            "https://github.com/AtmatiAdi/spectrenotes/releases/download/v0.2.0/spectrenotes.exe"
        );
        let sums = r.asset("sha256sums.txt").unwrap();
        assert_eq!(sums.size, 210);
        assert!(sums.api_url.ends_with("/assets/11"));
        assert!(r.is_newer_than("0.1.0"));
        assert!(!r.is_newer_than("0.2.0"));
        assert!(!r.is_newer_than("0.3.0"));
        assert_eq!(r.headline(), "Zmiany");
    }

    #[test]
    fn bez_zasobow_i_bez_tagu() {
        let r = Release::parse(r#"{"tag_name":"v1.0.0","assets":[],"tarball_url":"x"}"#).unwrap();
        assert!(r.assets.is_empty());
        assert_eq!(r.name, "v1.0.0");
        assert!(matches!(
            Release::parse(r#"{"name":"x"}"#),
            Err(Error::Parse(_))
        ));
        assert!(matches!(
            Release::parse(r#"{"tag_name":"nightly"}"#),
            Err(Error::Parse(_))
        ));
    }

    #[test]
    fn sumy() {
        let text = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  spectrenotes.exe\r\nBA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD *SpectreNotes-Setup.exe\n";
        assert_eq!(
            sum_for(text, "spectrenotes.exe").as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
        assert_eq!(
            sum_for(text, "spectrenotes-setup.exe").as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert_eq!(sum_for(text, "inny.exe"), None);
    }

    #[test]
    fn bajty_dla_ludzi() {
        assert_eq!(human_bytes(500), "500 B");
        assert_eq!(human_bytes(20 * 1024), "20 KB");
        assert_eq!(human_bytes(2_386_944), "2.3 MB");
    }
}
