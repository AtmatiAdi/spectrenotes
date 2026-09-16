//! Odczyt pojedynczych pol z plaskiego JSON-a GitHuba - bez parsera.
//! Odpowiedzi, ktore czytamy, maja kilka plaskich pol; serde bylby
//! najwiekszym crate'em w binarce.

/// Wartosc tekstowa pola `"key": "..."` (pierwsze wystapienie klucza).
/// Obsluguje `\"`, `\\`, `\/`, `\n`, `\t`, `\uXXXX`.
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
                            'r' => {}
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

/// Wartosc liczbowa pola `"key": 123` (pierwsze wystapienie klucza).
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

/// Wartosc logiczna pola `"key": true`.
pub fn json_bool(body: &str, key: &str) -> Option<bool> {
    let pat = format!("\"{key}\"");
    let i = body.find(&pat)?;
    let rest = body[i + pat.len()..]
        .trim_start()
        .strip_prefix(':')?
        .trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_plaski() {
        let body = r#"{"device_code":"abc\"d","user_code":"WDJB-MJHT","interval":5,"expires_in":899,"login":"AtmatiAdi","x":{"login":"inny"},"draft":false,"prerelease": true}"#;
        assert_eq!(json_str(body, "device_code").as_deref(), Some("abc\"d"));
        assert_eq!(json_str(body, "user_code").as_deref(), Some("WDJB-MJHT"));
        assert_eq!(json_u64(body, "interval"), Some(5));
        assert_eq!(json_u64(body, "expires_in"), Some(899));
        assert_eq!(json_str(body, "login").as_deref(), Some("AtmatiAdi"));
        assert_eq!(json_str(body, "brak"), None);
        assert_eq!(json_str(r#"{"a":"A\n"}"#, "a").as_deref(), Some("A\n"));
        assert_eq!(json_bool(body, "draft"), Some(false));
        assert_eq!(json_bool(body, "prerelease"), Some(true));
        assert_eq!(json_bool(body, "login"), None);
    }
}
