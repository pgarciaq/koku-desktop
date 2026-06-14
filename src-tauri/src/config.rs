use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const SAAS_SERVER_URL: &str = "https://console.redhat.com";
pub const SAAS_TOKEN_ENDPOINT: &str =
    "https://sso.redhat.com/auth/realms/redhat-external/protocol/openid-connect/token";
pub const SAAS_PROFILE_ID: &str = "saas";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub connection_mode: ConnectionMode,
    #[serde(default)]
    pub server_url: String,
    pub auth_mode: AuthMode,
    #[serde(default)]
    pub service_account: ServiceAccountConfig,
    #[serde(default)]
    pub offline_token: String,
    #[serde(default)]
    pub kc_username: String,
    #[serde(default)]
    pub kc_password: String,
}

impl ConnectionProfile {
    pub fn default_saas() -> Self {
        Self {
            id: SAAS_PROFILE_ID.to_string(),
            name: "Red Hat Hybrid Cloud Console".to_string(),
            connection_mode: ConnectionMode::Saas,
            server_url: String::new(),
            auth_mode: AuthMode::OfflineToken,
            service_account: ServiceAccountConfig::default(),
            offline_token: String::new(),
            kc_username: String::new(),
            kc_password: String::new(),
        }
    }

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub active_profile: String,
    pub profiles: Vec<ConnectionProfile>,
    pub theme: Theme,
    pub modules: ModuleConfig,
    pub dev_identity: serde_json::Value,
}

impl AppConfig {
    pub fn active_profile(&self) -> &ConnectionProfile {
        self.profiles
            .iter()
            .find(|p| p.id == self.active_profile)
            .unwrap_or_else(|| self.profiles.first().expect("profiles must not be empty"))
    }

    pub fn ensure_saas_profile(&mut self) {
        if !self.profiles.iter().any(|p| p.id == SAAS_PROFILE_ID) {
            self.profiles.insert(0, ConnectionProfile::default_saas());
        }
    }
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
    Password,
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
            active_profile: SAAS_PROFILE_ID.to_string(),
            profiles: vec![ConnectionProfile::default_saas()],
            theme: Theme::System,
            modules: ModuleConfig {
                ros: true,
                sources: true,
            },
            dev_identity: default_dev_identity(),
        }
    }
}

/// Old flat config format (pre-profiles) used for migration.
#[derive(Deserialize)]
struct LegacyConfig {
    connection_mode: ConnectionMode,
    server_url: String,
    auth_mode: AuthMode,
    theme: Theme,
    modules: ModuleConfig,
    #[serde(default)]
    service_account: ServiceAccountConfig,
    #[serde(default)]
    offline_token: String,
    dev_identity: serde_json::Value,
}

fn migrate_legacy(legacy: LegacyConfig) -> AppConfig {
    let profile_name = match legacy.connection_mode {
        ConnectionMode::Saas => "Red Hat Hybrid Cloud Console".to_string(),
        ConnectionMode::Private => {
            if legacy.server_url.is_empty() {
                "My Instance".to_string()
            } else {
                let url = &legacy.server_url;
                url::Url::parse(url)
                    .ok()
                    .and_then(|u| u.host_str().map(|h| h.to_string()))
                    .unwrap_or_else(|| "My Instance".to_string())
            }
        }
    };

    let profile_id = match legacy.connection_mode {
        ConnectionMode::Saas => SAAS_PROFILE_ID.to_string(),
        ConnectionMode::Private => uuid::Uuid::new_v4().to_string(),
    };

    let migrated_profile = ConnectionProfile {
        id: profile_id.clone(),
        name: profile_name,
        connection_mode: legacy.connection_mode,
        server_url: legacy.server_url,
        auth_mode: legacy.auth_mode,
        service_account: legacy.service_account,
        offline_token: legacy.offline_token,
        kc_username: String::new(),
        kc_password: String::new(),
    };

    let mut config = AppConfig {
        active_profile: profile_id,
        profiles: vec![migrated_profile],
        theme: legacy.theme,
        modules: legacy.modules,
        dev_identity: legacy.dev_identity,
    };
    config.ensure_saas_profile();
    config
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
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("reading config from {}", path.display()))?;

        let raw: serde_json::Value =
            serde_json::from_str(&contents).context("parsing config JSON")?;

        if raw.get("profiles").is_some() {
            let mut config: AppConfig =
                serde_json::from_value(raw).context("parsing new-format config")?;
            config.ensure_saas_profile();
            Ok(config)
        } else {
            let legacy: LegacyConfig =
                serde_json::from_value(raw).context("parsing legacy config for migration")?;
            let config = migrate_legacy(legacy);
            config.save().ok();
            Ok(config)
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
