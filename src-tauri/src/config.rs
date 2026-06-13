use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server_url: String,
    pub auth_mode: AuthMode,
    pub theme: Theme,
    pub modules: ModuleConfig,
    #[serde(default)]
    pub oidc: OidcConfig,
    pub dev_identity: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    Dev,
    Oidc,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleConfig {
    pub ros: bool,
    pub sources: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    #[serde(default = "default_keycloak_url")]
    pub keycloak_url: String,
    #[serde(default = "default_realm")]
    pub realm: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

fn default_keycloak_url() -> String {
    "https://keycloak.example.com".to_string()
}

fn default_realm() -> String {
    "cost-management".to_string()
}

impl Default for OidcConfig {
    fn default() -> Self {
        Self {
            keycloak_url: default_keycloak_url(),
            realm: default_realm(),
            client_id: "cost-management-ui".to_string(),
            client_secret: String::new(),
            username: String::new(),
            password: String::new(),
        }
    }
}

impl OidcConfig {
    pub fn token_url(&self) -> String {
        let base = self.keycloak_url.trim_end_matches('/');
        format!("{base}/realms/{}/protocol/openid-connect/token", self.realm)
    }
}

fn default_dev_identity() -> serde_json::Value {
    serde_json::json!({
        "identity": {
            "account_number": "10001",
            "org_id": "1234567",
            "type": "User",
            "user": {
                "username": "admin",
                "email": "admin@example.com",
                "is_org_admin": true
            }
        },
        "entitlements": {
            "cost_management": { "is_entitled": true }
        }
    })
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server_url: "http://localhost:8000".to_string(),
            auth_mode: AuthMode::Dev,
            theme: Theme::System,
            modules: ModuleConfig {
                ros: true,
                sources: true,
            },
            oidc: OidcConfig::default(),
            dev_identity: default_dev_identity(),
        }
    }
}

impl AppConfig {
    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("koku-desktop")
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.json")
    }

    pub fn is_first_launch() -> bool {
        !Self::config_path().exists()
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if path.exists() {
            let contents = fs::read_to_string(&path)
                .with_context(|| format!("reading config from {}", path.display()))?;
            serde_json::from_str(&contents).context("parsing config JSON")
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir();
        fs::create_dir_all(&dir)
            .with_context(|| format!("creating config directory {}", dir.display()))?;
        let contents =
            serde_json::to_string_pretty(self).context("serializing config to JSON")?;
        fs::write(Self::config_path(), contents).context("writing config file")
    }
}
