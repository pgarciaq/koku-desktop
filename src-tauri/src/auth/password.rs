use super::{AuthProvider, UserInfo};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

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

pub struct PasswordAuthProvider {
    token_endpoint: String,
    client_id: String,
    client_secret: String,
    username: String,
    password: String,
    cached: Mutex<Option<CachedToken>>,
}

impl PasswordAuthProvider {
    pub fn new(
        token_endpoint: String,
        client_id: String,
        client_secret: String,
        username: String,
        password: String,
    ) -> Self {
        Self {
            token_endpoint,
            client_id,
            client_secret,
            username,
            password,
            cached: Mutex::new(None),
        }
    }

    fn refresh_token(&self) -> Option<String> {
        let url = self.token_endpoint.clone();
        let params: Vec<(String, String)> = vec![
            ("grant_type".into(), "password".into()),
            ("client_id".into(), self.client_id.clone()),
            ("client_secret".into(), self.client_secret.clone()),
            ("username".into(), self.username.clone()),
            ("password".into(), self.password.clone()),
        ];

        let result = std::thread::spawn(move || -> Option<(String, u64)> {
            let client = reqwest::blocking::Client::builder()
                .danger_accept_invalid_certs(true)
                .timeout(Duration::from_secs(15))
                .build()
                .ok()?;

            let resp = client.post(&url).form(&params).send().ok()?;

            if !resp.status().is_success() {
                log::error!(
                    "Password grant token request failed: {} {}",
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
            log::info!("Password grant token acquired (expires in {expires_in}s)");
            Some(token)
        } else {
            log::error!(
                "Failed to acquire token from {}",
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

impl AuthProvider for PasswordAuthProvider {
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
                            .or_else(|| payload.get("sub"))
                            .and_then(|v| v.as_str())
                            .unwrap_or(&self.username)
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
            username: self.username.clone(),
            email: String::new(),
        }
    }
}
