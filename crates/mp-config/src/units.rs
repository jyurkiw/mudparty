//! Durations (`250ms`, `30s`, `5m`, `1h`) and sizes (`512KB`, `1MB`, a plain number of
//! bytes) as the config files write them.

use std::time::Duration;

use serde::{Deserialize, Deserializer, Serializer, de};

/// `250ms`, `30s`, `5m` or `1h`.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: u64 = num
        .parse()
        .map_err(|_| format!("{s:?} isn't a duration like 250ms, 30s, 5m or 1h"))?;
    match unit {
        "ms" => Ok(Duration::from_millis(n)),
        "s" => Ok(Duration::from_secs(n)),
        "m" => Ok(Duration::from_secs(n * 60)),
        "h" => Ok(Duration::from_secs(n * 3600)),
        _ => Err(format!("{s:?} isn't a duration like 250ms, 30s, 5m or 1h")),
    }
}

/// `512KB`, `1MB` (powers of 1024) or a plain number of bytes.
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: u64 = num
        .parse()
        .map_err(|_| format!("{s:?} isn't a size like 512KB or 1MB"))?;
    match unit {
        "" | "B" => Ok(n),
        "KB" => Ok(n * 1024),
        "MB" => Ok(n * 1024 * 1024),
        _ => Err(format!("{s:?} isn't a size like 512KB or 1MB")),
    }
}

pub fn duration<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
    parse_duration(&String::deserialize(d)?).map_err(de::Error::custom)
}

/// A duration in the largest unit that writes it exactly: `1h`, `4m`, `5s`, `1500ms`.
pub fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if !ms.is_multiple_of(1000) {
        return format!("{ms}ms");
    }
    let s = d.as_secs();
    match s {
        0 => "0s".to_owned(),
        s if s.is_multiple_of(3600) => format!("{}h", s / 3600),
        s if s.is_multiple_of(60) => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// A duration as [`duration`] reads it.
pub fn ser_duration<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format_duration(*d))
}

pub fn opt_size<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Bytes(u64),
        Text(String),
    }
    match Option::<Raw>::deserialize(d)? {
        None => Ok(None),
        Some(Raw::Bytes(n)) => Ok(Some(n)),
        Some(Raw::Text(t)) => parse_size(&t).map(Some).map_err(de::Error::custom),
    }
}
