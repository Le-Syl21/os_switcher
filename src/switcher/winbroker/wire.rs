//! The pipe protocol and the privileged operations it maps to.
//!
//! The wire format is a single line of JSON in each direction. Requests are a
//! **closed vocabulary** — `get_state`, `set`, `clear_next` — deserialized with
//! `deny_unknown_fields` and a bounded length, because this runs in the SYSTEM
//! service against input a standard user controls (G3/G4). Each operation reuses
//! the ordinary [`Switcher`], which re-reads the machine's real entries, so a
//! `set` for an entry that no longer exists is refused rather than blindly
//! written.

use serde::{Deserialize, Serialize};

use super::BrokerEntry;
use crate::switcher::{OsKind, Scope, Switcher};

/// Largest request we will read before parsing (G4). A request is three short
/// fields; a few hundred bytes is plenty, so this is generous headroom.
pub const MAX_REQUEST_BYTES: usize = 4096;
/// Longest selector key we accept.
const MAX_KEY_LEN: usize = 128;

/// One-shot vs. permanent, on the wire.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WireScope {
    Default,
    Once,
}

impl From<WireScope> for Scope {
    fn from(w: WireScope) -> Self {
        match w {
            WireScope::Default => Scope::Default,
            WireScope::Once => Scope::Once,
        }
    }
}

impl From<Scope> for WireScope {
    fn from(s: Scope) -> Self {
        match s {
            Scope::Default => WireScope::Default,
            Scope::Once => WireScope::Once,
        }
    }
}

/// The closed set of verbs. Anything else fails to deserialize.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Action {
    GetState,
    Set,
    ClearNext,
}

/// A request. `deny_unknown_fields` rejects anything with stray keys.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    action: Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<WireScope>,
}

/// One entry on the wire.
#[derive(Serialize, Deserialize)]
struct WireEntry {
    key: String,
    label: String,
    kind: String,
    is_default: bool,
    is_next: bool,
}

/// The service's reply.
#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
enum Response {
    State { entries: Vec<WireEntry> },
    Ok,
    Error { message: String },
}

fn kind_str(k: OsKind) -> &'static str {
    match k {
        OsKind::Windows => "windows",
        OsKind::Linux => "linux",
        OsKind::MacOs => "macos",
        OsKind::Other => "other",
    }
}

fn kind_from(s: &str) -> OsKind {
    match s {
        "windows" => OsKind::Windows,
        "linux" => OsKind::Linux,
        "macos" => OsKind::MacOs,
        _ => OsKind::Other,
    }
}

// ---- Server side (runs as SYSTEM inside the service) --------------------------

/// Handles one request line and returns the response line. Never panics: any
/// error, including a malformed request, comes back as an `error` response.
pub fn handle(request: &str) -> String {
    let response = match dispatch(request) {
        Ok(r) => r,
        Err(message) => Response::Error { message },
    };
    serde_json::to_string(&response)
        .unwrap_or_else(|_| r#"{"result":"error","message":"failed to serialize response"}"#.into())
}

fn dispatch(request: &str) -> Result<Response, String> {
    let req: Request =
        serde_json::from_str(request).map_err(|e| format!("malformed request: {e}"))?;
    if let Some(key) = &req.key {
        if key.len() > MAX_KEY_LEN {
            return Err("selector key too long".into());
        }
    }

    match req.action {
        Action::GetState => {
            let switcher = Switcher::detect();
            let entries = switcher
                .entries()
                .into_iter()
                .map(|e| WireEntry {
                    key: e.key,
                    label: e.label,
                    kind: kind_str(e.kind).to_string(),
                    is_default: e.is_default,
                    is_next: e.is_next,
                })
                .collect();
            Ok(Response::State { entries })
        }
        Action::Set => {
            let key = req.key.ok_or("`set` requires a `key`")?;
            let scope = req.scope.ok_or("`set` requires a `scope`")?;
            // `Switcher::set` re-reads the real entries and refuses an unknown
            // selector — this is the whitelist check (G3).
            let mut switcher = Switcher::detect();
            switcher
                .set(&key, scope.into())
                .map_err(|e| e.to_string())?;
            Ok(Response::Ok)
        }
        Action::ClearNext => {
            let mut switcher = Switcher::detect();
            switcher.clear_next().map_err(|e| e.to_string())?;
            Ok(Response::Ok)
        }
    }
}

// ---- Client side (runs unprivileged in the app) ------------------------------

/// The three request lines the client can send.
pub fn request_get_state() -> String {
    encode(Request {
        action: Action::GetState,
        key: None,
        scope: None,
    })
}

pub fn request_set(key: &str, scope: Scope) -> String {
    encode(Request {
        action: Action::Set,
        key: Some(key.to_string()),
        scope: Some(scope.into()),
    })
}

pub fn request_clear_next() -> String {
    encode(Request {
        action: Action::ClearNext,
        key: None,
        scope: None,
    })
}

fn encode(req: Request) -> String {
    // The fields are all plain data, so serialization cannot fail in practice.
    serde_json::to_string(&req).unwrap_or_default()
}

/// Parses a `get_state` reply into the display entries.
pub fn parse_state(json: &str) -> Result<Vec<BrokerEntry>, String> {
    match decode(json)? {
        Response::State { entries } => Ok(entries
            .into_iter()
            .map(|e| BrokerEntry {
                key: e.key,
                label: e.label,
                kind: kind_from(&e.kind),
                is_default: e.is_default,
                is_next: e.is_next,
            })
            .collect()),
        Response::Ok => Err("unexpected empty reply to get_state".into()),
        Response::Error { message } => Err(message),
    }
}

/// Parses an `ok`/`error` reply for a mutating request.
pub fn parse_ok(json: &str) -> Result<(), String> {
    match decode(json)? {
        Response::Ok => Ok(()),
        Response::State { .. } => Err("unexpected state reply to a mutating request".into()),
        Response::Error { message } => Err(message),
    }
}

fn decode(json: &str) -> Result<Response, String> {
    serde_json::from_str(json).map_err(|e| format!("malformed response: {e}"))
}
