mod dev;
mod oidc;

pub use dev::DevAuthProvider;
pub use oidc::OidcAuthProvider;

use reqwest::header::HeaderMap;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct UserInfo {
    pub username: String,
    pub email: String,
}

pub trait AuthProvider: Send + Sync {
    fn request_headers(&self) -> HeaderMap;
    fn user_info(&self) -> UserInfo;
}
