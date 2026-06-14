use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const SAAS_SERVER_URL: &str = "https://console.redhat.com";
const SAAS_TOKEN_ENDPOINT: &str =
    "https://sso.redhat.com/auth/realms/redhat-external/protocol/openid-connect/token";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub connection_mode: ConnectionMode,
    pub server_url: String,
    pub auth_mode: AuthMode,
    pub theme: Theme,
    pub modules: ModuleConfig,
    #[serde(default)]
    pub service_account: ServiceAccountConfig,
    #[serde(default)]
    pub offline_token: String,
    pub dev_identity: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionMode {
    Saas,
    Private,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    OfflineToken,
    ServiceAccount,
    Dev,
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
pub struct ServiceAccountConfig {
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub token_endpoint: String,
    #[serde(default)]
    pub display_name: String,
}

impl Default for ServiceAccountConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            token_endpoint: String::new(),
            display_name: String::new(),
        }
    }
}

impl AppConfig {
    pub fn effective_server_url(&self) -> &str {
        match self.connection_mode {
            ConnectionMode::Saas => SAAS_SERVER_URL,
            ConnectionMode::Private => &self.server_url,
        }
    }

    pub fn effective_token_endpoint(&self) -> &str {
        match self.connection_mode {
            ConnectionMode::Saas => SAAS_TOKEN_ENDPOINT,
            ConnectionMode::Private => &self.service_account.token_endpoint,
        }
    }

    pub fn is_saas(&self) -> bool {
        self.connection_mode == ConnectionMode::Saas
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
            connection_mode: ConnectionMode::Private,
            server_url: "http://localhost:8000".to_string(),
            auth_mode: AuthMode::Dev,
            theme: Theme::System,
            modules: ModuleConfig {
                ros: true,
                sources: true,
            },
            service_account: ServiceAccountConfig::default(),
            offline_token: String::new(),
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
