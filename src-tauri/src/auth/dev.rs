use super::{AuthProvider, UserInfo};
use anyhow::{Context, Result};
use base64::Engine;
use reqwest::header::{HeaderMap, HeaderValue};

pub struct DevAuthProvider {
    encoded: String,
    user_info: UserInfo,
}

impl DevAuthProvider {
    pub fn new(dev_identity: serde_json::Value) -> Result<Self> {
        let json =
            serde_json::to_string(&dev_identity).context("serializing dev identity to JSON")?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());

        let username = dev_identity
            .pointer("/identity/user/username")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let email = dev_identity
            .pointer("/identity/user/email")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Ok(Self {
            encoded,
            user_info: UserInfo { username, email },
        })
    }
}

impl AuthProvider for DevAuthProvider {
    fn request_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Ok(value) = HeaderValue::from_str(&self.encoded) {
            headers.insert("x-rh-identity", value);
        }
        headers
    }

    fn user_info(&self) -> UserInfo {
        self.user_info.clone()
    }
}
