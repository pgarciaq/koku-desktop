// Phase 2: Keycloak OIDC authorization_code + PKCE flow with token refresh.
// Will store CLIENT_SECRET and refresh tokens in the OS keychain via the keyring crate.

use super::{AuthProvider, UserInfo};
use reqwest::header::HeaderMap;

pub struct OidcAuthProvider {
    #[allow(dead_code)]
    client_id: String,
}

impl OidcAuthProvider {
    pub fn new(client_id: String) -> Self {
        Self { client_id }
    }
}

impl AuthProvider for OidcAuthProvider {
    fn request_headers(&self) -> HeaderMap {
        // TODO(Phase 2): return Authorization: Bearer <jwt> after OIDC login
        HeaderMap::new()
    }

    fn user_info(&self) -> UserInfo {
        UserInfo {
            username: "oidc-user".to_string(),
            email: "oidc@example.com".to_string(),
        }
    }
}
