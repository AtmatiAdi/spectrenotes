//! Wiele space'ow w jednej instancji (Etap 6 3/4). Space domyslny (`default`,
//! zawsze prywatny) plus space'y wspoldzielone: kazdy to osobny katalog
//! `<dane>\spaces\<nazwa>` z wlasnym repozytorium `spectrenotes-<nazwa>`
//! na koncie zalozyciela i wspolpracownikami GitHub.
//!
//! Rejestr `<dane>\spaces.txt` (lokalny dla maszyny, jedna linia na space:
//! `nazwa=wlasciciel`; pusty wlasciciel = nasze konto) mowi, ktore katalogi
//! sa space'ami i czyje jest repo. Znajomi (`friends.txt`, jeden login GitHub
//! na linie) leza w space'ie domyslnym, wiec synchronizuja sie miedzy
//! wlasnymi urzadzeniami jak notatki.

use std::io;
use std::path::{Path, PathBuf};

/// Space znany aplikacji: katalog, nazwa i czyje jest repozytorium.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceInfo {
    pub name: String,
    pub root: PathBuf,
    /// Login wlasciciela repo dla space'u cudzego; `None` = wlasne konto.
    pub owner: Option<String>,
}

impl SpaceInfo {
    /// Nazwa repozytorium na GitHubie.
    pub fn repo_name(&self) -> String {
        crate::github::repo_name(&self.name)
    }
}

pub fn registry_path(data_dir: &Path) -> PathBuf {
    data_dir.join("spaces.txt")
}

fn spaces_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("spaces")
}

/// Space'y z rejestru, w kolejnosci zapisu; katalogi, ktorych juz nie ma,
/// sa pomijane. Space domyslny nie jest w rejestrze - aplikacja daje go sama.
pub fn load(data_dir: &Path, default_root: &Path) -> Vec<SpaceInfo> {
    let Ok(text) = std::fs::read_to_string(registry_path(data_dir)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, owner) = line.split_once('=').unwrap_or((line, ""));
        let name = name.trim();
        if !valid_name(name) {
            continue;
        }
        let root = spaces_dir(data_dir).join(name);
        if root == default_root || !root.is_dir() || out.iter().any(|s: &SpaceInfo| s.name == name)
        {
            continue;
        }
        out.push(SpaceInfo {
            name: name.to_string(),
            root,
            owner: Some(owner.trim().to_string()).filter(|o| !o.is_empty()),
        });
    }
    out
}

fn save(data_dir: &Path, spaces: &[SpaceInfo]) -> io::Result<()> {
    let mut text = String::from("# space'y wspoldzielone: nazwa=wlasciciel (pusty = moje konto)\n");
    for s in spaces {
        text.push_str(&s.name);
        text.push('=');
        text.push_str(s.owner.as_deref().unwrap_or(""));
        text.push('\n');
    }
    std::fs::write(registry_path(data_dir), text)
}

/// Nowy space (wlasny albo cudzy, do ktorego nas zaproszono): katalog i wpis
/// w rejestrze. Repozytorium zaklada/podpina watek sync przy pierwszym cyklu.
pub fn create(
    data_dir: &Path,
    spaces: &mut Vec<SpaceInfo>,
    name: &str,
    owner: Option<&str>,
) -> io::Result<SpaceInfo> {
    let name = normalize_name(name);
    if !valid_name(&name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "space name"));
    }
    if name == "default" || spaces.iter().any(|s| s.name == name) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a space with this name already exists",
        ));
    }
    let root = spaces_dir(data_dir).join(&name);
    std::fs::create_dir_all(root.join("notes"))?;
    let info = SpaceInfo {
        name,
        root,
        owner: owner.map(str::to_string).filter(|o| !o.is_empty()),
    };
    spaces.push(info.clone());
    save(data_dir, spaces)?;
    Ok(info)
}

/// Opuszczenie space'u: tylko wpis w rejestrze - katalog z notatkami zostaje
/// na dysku (nic nie kasujemy za uzytkownika).
pub fn forget(data_dir: &Path, spaces: &mut Vec<SpaceInfo>, name: &str) -> io::Result<()> {
    spaces.retain(|s| s.name != name);
    save(data_dir, spaces)
}

/// Nazwa space'u = nazwa katalogu i czlon nazwy repo: male litery, cyfry, `-`.
pub fn normalize_name(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if (c == ' ' || c == '-' || c == '_') && !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(40).collect()
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.starts_with('-')
}

/// Nazwa space'u z nazwy repozytorium (`spectrenotes-projekt` -> `projekt`).
pub fn name_from_repo(repo: &str) -> Option<String> {
    let name = repo.strip_prefix("spectrenotes-")?;
    valid_name(name).then(|| name.to_string())
}

// ----- znajomi -------------------------------------------------------------

pub fn friends_path(default_root: &Path) -> PathBuf {
    default_root.join("friends.txt")
}

/// Loginy GitHub znajomych, w kolejnosci dodania.
pub fn load_friends(default_root: &Path) -> Vec<String> {
    std::fs::read_to_string(friends_path(default_root))
        .map(|t| {
            t.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn save_friends(default_root: &Path, friends: &[String]) -> io::Result<()> {
    let mut text = String::from("# znajomi: login GitHub na linie\n");
    for f in friends {
        text.push_str(f);
        text.push('\n');
    }
    std::fs::write(friends_path(default_root), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nazwy() {
        assert_eq!(normalize_name("Projekt z Kuba!"), "projekt-z-kuba");
        assert_eq!(normalize_name("  --a__b  "), "a-b");
        assert_eq!(normalize_name("!!!"), "");
        assert_eq!(
            name_from_repo("spectrenotes-projekt"),
            Some("projekt".into())
        );
        assert_eq!(name_from_repo("inne-repo"), None);
    }

    #[test]
    fn rejestr_i_znajomi() {
        let dir = std::env::temp_dir().join(format!("spectre-spaces-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let default_root = dir.join("spaces").join("default");
        std::fs::create_dir_all(&default_root).unwrap();
        let mut spaces = load(&dir, &default_root);
        assert!(spaces.is_empty());
        let own = create(&dir, &mut spaces, "Moj Projekt", None).unwrap();
        assert_eq!(own.name, "moj-projekt");
        assert_eq!(own.repo_name(), "spectrenotes-moj-projekt");
        let theirs = create(&dir, &mut spaces, "kuba", Some("kuba")).unwrap();
        assert_eq!(theirs.owner.as_deref(), Some("kuba"));
        assert!(create(&dir, &mut spaces, "kuba", None).is_err());
        assert!(create(&dir, &mut spaces, "default", None).is_err());
        let again = load(&dir, &default_root);
        assert_eq!(again, spaces);
        forget(&dir, &mut spaces, "kuba").unwrap();
        assert_eq!(load(&dir, &default_root).len(), 1);

        assert!(load_friends(&default_root).is_empty());
        save_friends(&default_root, &["kuba".into(), "ola".into()]).unwrap();
        assert_eq!(load_friends(&default_root), vec!["kuba", "ola"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
