use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

/// Subprotocol marker. The browser sends `Sec-WebSocket-Protocol: index-key, <key>`.
pub(crate) const WS_KEY_PROTOCOL: &str = "index-key";

pub struct ApiKeyState {
    pub keys: HashSet<String>,
}

impl ApiKeyState {
    pub fn from_keys(keys: Vec<String>) -> Self {
        Self {
            keys: keys.into_iter().filter(|k| !k.is_empty()).collect(),
        }
    }

    /// No configured keys means open access; the default bind is loopback only.
    pub fn allows(&self, presented: Option<&str>) -> bool {
        if self.keys.is_empty() {
            return true;
        }
        let Some(presented) = presented else {
            return false;
        };
        // Check every key with no early exit, so the time does not depend on which key matches.
        self.keys.iter().fold(false, |ok, key| {
            ct_eq(key.as_bytes(), presented.as_bytes()) | ok
        })
    }
}

/// The time depends only on the presented length, not on the key length or content.
fn ct_eq(key: &[u8], presented: &[u8]) -> bool {
    let len_differs = u8::from(key.len() != presented.len());
    presented.iter().enumerate().fold(len_differs, |acc, (i, byte)| {
        acc | (byte ^ key.get(i).copied().unwrap_or(0))
    }) == 0
}

pub async fn require_api_key(
    State(state): State<Arc<ApiKeyState>>,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let headers = request.headers();
    let presented = extract_api_key(headers).or_else(|| {
        matches!(request.uri().path(), "/stream" | "/v1/stream")
            .then(|| subprotocol_key(headers))
            .flatten()
    });

    if state.allows(presented) {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

pub(crate) fn extract_api_key(headers: &HeaderMap) -> Option<&str> {
    if let Some(value) = headers.get("x-api-key")
        && let Ok(key) = value.to_str() {
            let key = key.trim();
            if !key.is_empty() {
                return Some(key);
            }
        }

    if let Some(value) = headers.get(axum::http::header::AUTHORIZATION)
        && let Ok(auth) = value.to_str()
            && let Some(key) = auth.strip_prefix("Bearer ") {
                let key = key.trim();
                if !key.is_empty() {
                    return Some(key);
                }
            }

    None
}

/// Browsers cannot set headers on a WebSocket, but they can send subprotocols.
/// A query string would put the key in proxy logs and browser history.
fn subprotocol_key(headers: &HeaderMap) -> Option<&str> {
    let mut parts = headers
        .get_all(header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim);
    parts.find(|part| *part == WS_KEY_PROTOCOL)?;
    parts.next().filter(|key| !key.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn bearer_token_is_extracted() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer secret-key"),
        );
        assert_eq!(extract_api_key(&headers), Some("secret-key"));
    }

    #[test]
    fn x_api_key_header_is_extracted() {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("dev-key"));
        assert_eq!(extract_api_key(&headers), Some("dev-key"));
    }

    #[test]
    fn missing_key_returns_none() {
        assert!(extract_api_key(&HeaderMap::new()).is_none());
    }

    #[test]
    fn allows_checks_configured_keys() {
        let open = ApiKeyState::from_keys(vec![]);
        assert!(open.allows(None));

        let state = ApiKeyState::from_keys(vec!["k1".into(), "key-two".into()]);
        assert!(state.allows(Some("key-two")));
        assert!(state.allows(Some("k1")));
        assert!(!state.allows(Some("key-twO")));
        assert!(!state.allows(Some("k")));
        assert!(!state.allows(Some("k1\0")));
        assert!(!state.allows(Some("")));
        assert!(!state.allows(None));
    }

    #[test]
    fn subprotocol_key_follows_marker() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static("other, index-key , k-1"),
        );
        assert_eq!(subprotocol_key(&headers), Some("k-1"));

        headers.insert(header::SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static("k-1, index-key"));
        assert!(subprotocol_key(&headers).is_none());
        assert!(subprotocol_key(&HeaderMap::new()).is_none());
    }
}
