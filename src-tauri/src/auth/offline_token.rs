use super::{AuthProvider, UserInfo};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const RHSM_CLIENT_ID: &str = "rhsm-api";

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
}

struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

pub struct OfflineTokenAuthProvider {
    token_endpoint: String,
    offline_token: String,
    is_saas: bool,
    cached: Mutex<Option<CachedToken>>,
}

impl OfflineTokenAuthProvider {
    pub fn new(token_endpoint: String, offline_token: String, is_saas: bool) -> Self {
        Self {
            token_endpoint,
            offline_token,
            is_saas,
            cached: Mutex::new(None),
        }
    }

    fn refresh_token(&self) -> Option<String> {
        let url = self.token_endpoint.clone();
        let mut params: Vec<(String, String)> = vec![
            ("grant_type".into(), "refresh_token".into()),
            ("client_id".into(), RHSM_CLIENT_ID.into()),
            ("refresh_token".into(), self.offline_token.clone()),
        ];
        if self.is_saas {
            params.push(("scope".into(), "api.console".into()));
        }

        let result = std::thread::spawn(move || -> Option<(String, u64)> {
            let client = reqwest::blocking::Client::builder()
                .danger_accept_invalid_certs(true)
                .timeout(Duration::from_secs(15))
                .build()
                .ok()?;

            let resp = client.post(&url).form(&params).send().ok()?;

            if !resp.status().is_success() {
                log::error!(
                    "Offline token refresh failed: {} {}",
                    resp.status(),
                    resp.text().unwrap_or_default()
                );
                return None;
            }

            let token_resp: TokenResponse = resp.json().ok()?;
            Some((token_resp.access_token, token_resp.expires_in))
        })
        .join()
        .ok()
        .flatten();

        if let Some((access_token, expires_in)) = result {
            let expires_at = Instant::now() + Duration::from_secs(expires_in.saturating_sub(30));
            let token = access_token.clone();
            if let Ok(mut cached) = self.cached.lock() {
                *cached = Some(CachedToken {
                    access_token,
                    expires_at,
                });
            }
            log::info!("Offline token refreshed (expires in {expires_in}s)");
            Some(token)
        } else {
            log::error!(
                "Failed to refresh offline token via {}",
                self.token_endpoint
            );
            None
        }
    }

    fn get_valid_token(&self) -> Option<String> {
        if let Ok(cached) = self.cached.lock() {
            if let Some(ref t) = *cached {
                if t.expires_at > Instant::now() {
                    return Some(t.access_token.clone());
                }
            }
        }
        self.refresh_token()
    }

    fn decode_jwt_payload(token: &str) -> Option<serde_json::Value> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() < 2 {
            return None;
        }
        let decoded = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD_NO_PAD,
            parts[1],
        )
        .or_else(|_| {
            base64::Engine::decode(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                parts[1],
            )
        })
        .ok()?;
        serde_json::from_slice(&decoded).ok()
    }
}

impl AuthProvider for OfflineTokenAuthProvider {
    fn request_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(token) = self.get_valid_token() {
            if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
                headers.insert("authorization", value);
            }
        }
        headers
    }

    fn user_info(&self) -> UserInfo {
        if let Ok(cached) = self.cached.lock() {
            if let Some(ref t) = *cached {
                if let Some(payload) = Self::decode_jwt_payload(&t.access_token) {
                    return UserInfo {
                        username: payload
                            .get("preferred_username")
                            .or_else(|| payload.get("username"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("user")
                            .to_string(),
                        email: payload
                            .get("email")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    };
                }
            }
        }
        UserInfo {
            username: "user".into(),
            email: String::new(),
        }
    }
}
