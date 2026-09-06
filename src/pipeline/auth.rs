use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

pub struct ApiKeyState {
    pub keys: HashSet<String>,
}

impl ApiKeyState {
    pub fn from_keys(keys: Vec<String>) -> Self {
        Self {
            keys: keys.into_iter().filter(|k| !k.is_empty()).collect(),
        }
    }
}

pub async fn require_api_key(
    State(state): State<Arc<ApiKeyState>>,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if state.keys.is_empty() {
        return Ok(next.run(request).await);
    }

    let Some(key) = extract_api_key(request.headers()) else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    if state.keys.contains(&key) {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn extract_api_key(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers.get("x-api-key")
        && let Ok(key) = value.to_str() {
            let key = key.trim();
            if !key.is_empty() {
                return Some(key.to_string());
            }
        }

    if let Some(value) = headers.get(axum::http::header::AUTHORIZATION)
        && let Ok(auth) = value.to_str()
            && let Some(key) = auth.strip_prefix("Bearer ") {
                let key = key.trim();
                if !key.is_empty() {
                    return Some(key.to_string());
                }
            }

    None
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
        assert_eq!(extract_api_key(&headers).as_deref(), Some("secret-key"));
    }

    #[test]
    fn x_api_key_header_is_extracted() {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("dev-key"));
        assert_eq!(extract_api_key(&headers).as_deref(), Some("dev-key"));
    }

    #[test]
    fn missing_key_returns_none() {
        assert!(extract_api_key(&HeaderMap::new()).is_none());
    }
}
