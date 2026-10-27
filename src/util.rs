// src/util.rs
pub fn now_str() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let ms = dur.as_millis();
    format!("{ms}")
}

// bool-Parser falls noch nicht vorhanden
pub fn parse_bool(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}
