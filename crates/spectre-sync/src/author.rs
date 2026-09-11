//! Autor = (uzytkownik, urzadzenie). Nazwa jest katalogiem w `ops/`,
//! skrot jest `AuthorId` w kazdej operacji.

use spectre_proto::AuthorId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorName {
    pub user: String,
    pub device: String,
}

impl AuthorName {
    pub fn new(user: &str, device: &str) -> Self {
        Self {
            user: sanitize(user),
            device: sanitize(device),
        }
    }

    /// Z env: `USERNAME@COMPUTERNAME` na Windows, `USER@HOSTNAME` gdzie indziej.
    pub fn from_env() -> Self {
        let user = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "user".into());
        let device = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "device".into());
        Self::new(&user, &device)
    }

    /// Nazwa katalogu: `user@device`.
    pub fn dir_name(&self) -> String {
        format!("{}@{}", self.user, self.device)
    }

    pub fn id(&self) -> AuthorId {
        AuthorId::from_name(&self.dir_name())
    }
}

/// Tylko znaki bezpieczne w nazwie katalogu na kazdym systemie plikow.
fn sanitize(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();
    if out.is_empty() {
        out.push('x');
    }
    out
}
