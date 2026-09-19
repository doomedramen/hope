//! Cursor pagination (spec §14.1) shared by every M1 list endpoint.
//! Cursor = base64 of `"<rfc3339 created_at>|<uuid>"`, opaque to clients.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct ListParams {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

pub const DEFAULT_LIMIT: i64 = 50;
pub const MAX_LIMIT: i64 = 200;

pub fn effective_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

pub fn decode_cursor(cursor: &Option<String>) -> Option<(String, Uuid)> {
    let raw = cursor.as_ref()?;
    let bytes = URL_SAFE_NO_PAD.decode(raw).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let (created_at, id) = text.split_once('|')?;
    let id = Uuid::parse_str(id).ok()?;
    Some((created_at.to_string(), id))
}

pub fn encode_cursor(created_at: &str, id: Uuid) -> String {
    URL_SAFE_NO_PAD.encode(format!("{created_at}|{id}"))
}
