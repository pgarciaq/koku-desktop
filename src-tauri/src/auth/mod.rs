mod dev;
mod offline_token;
mod service_account;

pub use dev::DevAuthProvider;
pub use offline_token::OfflineTokenAuthProvider;
pub use service_account::ServiceAccountAuthProvider;

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
