use regex::Regex;
use std::sync::LazyLock;
static KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z0-9]{5}(?:-[A-Z0-9]{5}){2,4}\b").unwrap());
static MAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").unwrap());
static HOME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:/Users/|/home/|[A-Z]:\\Users\\)[^/\\\s]+").unwrap());
static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[A-Za-z0-9_%=-]{32,}\b").unwrap());
pub fn safe_text(text: &str) -> String {
    let text = KEY.replace_all(text, "[product key redacted]");
    let text = MAIL.replace_all(&text, "[email redacted]");
    let text = HOME.replace_all(&text, "[home]");
    TOKEN
        .replace_all(&text, "[token redacted]")
        .chars()
        .take(500)
        .collect()
}
pub fn valid_key(text: &str) -> bool {
    KEY.find(text).is_some_and(|m| m.as_str() == text)
}
pub fn csv_safe(text: &str) -> String {
    if text
        .trim_start()
        .starts_with(['=', '+', '-', '@', '\t', '\r'])
    {
        format!("'{text}")
    } else {
        text.into()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redact_keys_paths_mail() {
        let s = safe_text("AAAAA-BBBBB-CCCCC /Users/example/private example@example.invalid");
        assert!(!s.contains("AAAAA"));
        assert!(!s.contains("/Users/example"));
        assert!(!s.contains("example@"));
    }
    #[test]
    fn formula_safe_export() {
        assert!(csv_safe("=cmd()").starts_with('\''));
        assert_eq!(csv_safe("Example game"), "Example game");
    }
}
