use crate::config::{AppConfig, AuthMode, ConnectionMode, ConnectionProfile, ServiceAccountConfig, Theme};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::process;

#[derive(Parser)]
#[command(
    name = "koku-desktop",
    about = "Red Hat Lightspeed Cost Management Desktop",
    version,
    long_about = "Desktop client for Red Hat Lightspeed Cost Management.\n\n\
                   Run without arguments to launch the GUI.\n\
                   Use subcommands to manage configuration from the command line."
)]
pub struct Cli {
    /// Launch with a specific connection profile (by name)
    #[arg(long = "profile", global = true)]
    pub profile_override: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manage connection profiles
    Profile(ProfileArgs),
    /// Manage application configuration
    Config(ConfigArgs),
}

// ---------------------------------------------------------------------------
// profile subcommands
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct ProfileArgs {
    #[command(subcommand)]
    pub action: ProfileAction,
}

#[derive(Subcommand)]
pub enum ProfileAction {
    /// List all connection profiles
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show details of a profile (active profile if name omitted)
    Show {
        /// Profile name
        name: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Create a new connection profile
    Create {
        /// Profile name
        name: String,
        #[command(flatten)]
        opts: ProfileOptions,
        /// Set as the active profile after creation
        #[arg(long)]
        activate: bool,
    },
    /// Update an existing connection profile
    Update {
        /// Profile name
        name: String,
        #[command(flatten)]
        opts: ProfileOptions,
        /// Set as the active profile after update
        #[arg(long)]
        activate: bool,
    },
    /// Delete a connection profile
    Delete {
        /// Profile name
        name: String,
    },
    /// Set a profile as the active profile
    Activate {
        /// Profile name
        name: String,
    },
}

#[derive(Args)]
pub struct ProfileOptions {
    /// Connection mode
    #[arg(long = "mode", value_enum)]
    pub connection_mode: Option<CliConnectionMode>,

    /// Server URL (for private instances)
    #[arg(long)]
    pub server_url: Option<String>,

    /// Authentication mode
    #[arg(long = "auth", value_enum)]
    pub auth_mode: Option<CliAuthMode>,

    /// Offline token (for offline-token auth)
    #[arg(long)]
    pub offline_token: Option<String>,

    /// Client ID (for service-account or password auth)
    #[arg(long)]
    pub client_id: Option<String>,

    /// Client secret (for service-account or password auth)
    #[arg(long)]
    pub client_secret: Option<String>,

    /// Token endpoint URL (for private instances)
    #[arg(long)]
    pub token_endpoint: Option<String>,

    /// Display name for service accounts
    #[arg(long)]
    pub display_name: Option<String>,

    /// Username (for password auth)
    #[arg(long)]
    pub username: Option<String>,

    /// Password (for password auth)
    #[arg(long)]
    pub password: Option<String>,
}

#[derive(Clone, ValueEnum)]
pub enum CliConnectionMode {
    Saas,
    Private,
}

#[derive(Clone, ValueEnum)]
pub enum CliAuthMode {
    OfflineToken,
    ServiceAccount,
    Password,
    Dev,
}

// ---------------------------------------------------------------------------
// config subcommands
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Print the full configuration as JSON
    Show,
    /// Set the UI theme
    SetTheme {
        /// Theme to set
        #[arg(value_enum)]
        theme: CliTheme,
    },
    /// Toggle developer mode (no-authentication identity)
    SetDev {
        /// on/off (or true/false, yes/no, enable/disable)
        #[arg(value_parser = parse_on_off)]
        state: OnOff,
    },
    /// Import configuration from a JSON file
    Import {
        /// Path to the JSON config file
        file: String,
    },
    /// Export configuration as JSON
    Export {
        /// Write to file instead of stdout
        file: Option<String>,
    },
    /// Print the configuration file path
    Path,
}

#[derive(Clone, ValueEnum)]
pub enum CliTheme {
    Light,
    Dark,
    System,
}

#[derive(Clone)]
pub struct OnOff(pub bool);

fn parse_on_off(s: &str) -> Result<OnOff, String> {
    match s.to_lowercase().as_str() {
        "on" | "true" | "1" | "yes" | "enable" | "enabled" => Ok(OnOff(true)),
        "off" | "false" | "0" | "no" | "disable" | "disabled" => Ok(OnOff(false)),
        _ => Err(format!("expected on/off, got '{s}'")),
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Parse CLI args and handle subcommands. Returns `Some(cli)` if the GUI
/// should launch, or `None` if a subcommand already ran and the process
/// should exit.
pub fn run() -> Option<Cli> {
    let cli = Cli::parse();

    if cli.command.is_none() {
        return Some(cli);
    }

    // Re-attach console on Windows so output is visible
    #[cfg(target_os = "windows")]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }

    match cli.command.as_ref().unwrap() {
        Commands::Profile(args) => handle_profile(&args.action),
        Commands::Config(args) => handle_config(&args.action),
    }

    None
}

// ---------------------------------------------------------------------------
// Profile handlers
// ---------------------------------------------------------------------------

fn handle_profile(action: &ProfileAction) {
    let mut config = load_or_exit();

    match action {
        ProfileAction::List { json } => {
            if *json {
                let items: Vec<serde_json::Value> = config
                    .profiles
                    .iter()
                    .map(|p| {
                        serde_json::json!({
                            "id": p.id,
                            "name": p.name,
                            "connection_mode": p.connection_mode,
                            "auth_mode": p.auth_mode,
                            "active": p.id == config.active_profile,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&items).unwrap());
            } else {
                println!("{:<30} {:<10} {:<16} {}", "NAME", "MODE", "AUTH", "ACTIVE");
                println!("{}", "-".repeat(70));
                for p in &config.profiles {
                    let active = if p.id == config.active_profile { "*" } else { "" };
                    println!(
                        "{:<30} {:<10} {:<16} {}",
                        p.name,
                        format!("{:?}", p.connection_mode).to_lowercase(),
                        format!("{:?}", p.auth_mode),
                        active,
                    );
                }
            }
        }

        ProfileAction::Show { name, json } => {
            let profile = match name {
                Some(n) => find_profile(&config, n),
                None => Some(config.active_profile().clone()),
            };
            match profile {
                Some(p) => {
                    if *json {
                        println!("{}", serde_json::to_string_pretty(&p).unwrap());
                    } else {
                        print_profile(&p, p.id == config.active_profile);
                    }
                }
                None => {
                    eprintln!("Error: profile '{}' not found", name.as_deref().unwrap_or(""));
                    process::exit(1);
                }
            }
        }

        ProfileAction::Create { name, opts, activate } => {
            if config.profiles.iter().any(|p| p.name == *name) {
                eprintln!("Error: profile '{name}' already exists. Use 'profile update' instead.");
                process::exit(1);
            }

            let mut profile = ConnectionProfile {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.clone(),
                connection_mode: ConnectionMode::Private,
                server_url: String::new(),
                auth_mode: AuthMode::OfflineToken,
                service_account: ServiceAccountConfig::default(),
                offline_token: String::new(),
                kc_username: String::new(),
                kc_password: String::new(),
            };
            apply_options(&mut profile, opts);
            let profile_id = profile.id.clone();
            config.profiles.push(profile);

            if *activate {
                config.active_profile = profile_id;
            }

            save_or_exit(&config);
            println!("Created profile '{name}'");
        }

        ProfileAction::Update { name, opts, activate } => {
            let idx = match config.profiles.iter().position(|p| p.name == *name) {
                Some(i) => i,
                None => {
                    eprintln!("Error: profile '{name}' not found");
                    process::exit(1);
                }
            };

            apply_options(&mut config.profiles[idx], opts);

            if *activate {
                config.active_profile = config.profiles[idx].id.clone();
            }

            save_or_exit(&config);
            println!("Updated profile '{name}'");
        }

        ProfileAction::Delete { name } => {
            let idx = match config.profiles.iter().position(|p| p.name == *name) {
                Some(i) => i,
                None => {
                    eprintln!("Error: profile '{name}' not found");
                    process::exit(1);
                }
            };

            if config.profiles.len() <= 1 {
                eprintln!("Error: cannot delete the only remaining profile");
                process::exit(1);
            }

            let deleted_id = config.profiles[idx].id.clone();
            config.profiles.remove(idx);

            if config.active_profile == deleted_id {
                config.active_profile = config.profiles[0].id.clone();
                eprintln!(
                    "Active profile was deleted. Switched to '{}'",
                    config.profiles[0].name
                );
            }

            save_or_exit(&config);
            println!("Deleted profile '{name}'");
        }

        ProfileAction::Activate { name } => {
            match config.profiles.iter().find(|p| p.name == *name) {
                Some(p) => {
                    config.active_profile = p.id.clone();
                    save_or_exit(&config);
                    println!("Activated profile '{name}'");
                }
                None => {
                    eprintln!("Error: profile '{name}' not found");
                    process::exit(1);
                }
            }
        }
    }
}

fn apply_options(profile: &mut ConnectionProfile, opts: &ProfileOptions) {
    if let Some(mode) = &opts.connection_mode {
        profile.connection_mode = match mode {
            CliConnectionMode::Saas => ConnectionMode::Saas,
            CliConnectionMode::Private => ConnectionMode::Private,
        };
    }
    if let Some(url) = &opts.server_url {
        profile.server_url = url.clone();
    }
    if let Some(auth) = &opts.auth_mode {
        profile.auth_mode = match auth {
            CliAuthMode::OfflineToken => AuthMode::OfflineToken,
            CliAuthMode::ServiceAccount => AuthMode::ServiceAccount,
            CliAuthMode::Password => AuthMode::Password,
            CliAuthMode::Dev => AuthMode::Dev,
        };
    }
    if let Some(token) = &opts.offline_token {
        profile.offline_token = token.clone();
    }
    if let Some(id) = &opts.client_id {
        profile.service_account.client_id = id.clone();
    }
    if let Some(secret) = &opts.client_secret {
        profile.service_account.client_secret = secret.clone();
    }
    if let Some(ep) = &opts.token_endpoint {
        profile.service_account.token_endpoint = ep.clone();
    }
    if let Some(dn) = &opts.display_name {
        profile.service_account.display_name = dn.clone();
    }
    if let Some(u) = &opts.username {
        profile.kc_username = u.clone();
    }
    if let Some(p) = &opts.password {
        profile.kc_password = p.clone();
    }
}

fn find_profile(config: &AppConfig, name: &str) -> Option<ConnectionProfile> {
    config
        .profiles
        .iter()
        .find(|p| p.name == name)
        .cloned()
}

fn print_profile(p: &ConnectionProfile, is_active: bool) {
    println!("Name:            {}", p.name);
    println!("ID:              {}", p.id);
    println!("Active:          {}", if is_active { "yes" } else { "no" });
    println!(
        "Connection mode: {}",
        format!("{:?}", p.connection_mode).to_lowercase()
    );
    if !p.server_url.is_empty() {
        println!("Server URL:      {}", p.server_url);
    }
    println!("Auth mode:       {:?}", p.auth_mode);
    match p.auth_mode {
        AuthMode::OfflineToken => {
            if !p.offline_token.is_empty() {
                let masked = mask_secret(&p.offline_token);
                println!("Offline token:   {masked}");
            }
        }
        AuthMode::ServiceAccount => {
            if !p.service_account.client_id.is_empty() {
                println!("Client ID:       {}", p.service_account.client_id);
            }
            if !p.service_account.client_secret.is_empty() {
                println!("Client secret:   {}", mask_secret(&p.service_account.client_secret));
            }
            if !p.service_account.display_name.is_empty() {
                println!("Display name:    {}", p.service_account.display_name);
            }
            if !p.service_account.token_endpoint.is_empty() {
                println!("Token endpoint:  {}", p.service_account.token_endpoint);
            }
        }
        AuthMode::Password => {
            if !p.kc_username.is_empty() {
                println!("Username:        {}", p.kc_username);
            }
            if !p.kc_password.is_empty() {
                println!("Password:        {}", mask_secret(&p.kc_password));
            }
            if !p.service_account.client_id.is_empty() {
                println!("Client ID:       {}", p.service_account.client_id);
            }
            if !p.service_account.client_secret.is_empty() {
                println!("Client secret:   {}", mask_secret(&p.service_account.client_secret));
            }
            if !p.service_account.token_endpoint.is_empty() {
                println!("Token endpoint:  {}", p.service_account.token_endpoint);
            }
        }
        AuthMode::Dev => {}
    }
}

fn mask_secret(s: &str) -> String {
    if s.len() <= 8 {
        return "****".to_string();
    }
    let visible = &s[..4];
    format!("{visible}...{}", "*".repeat(4))
}

// ---------------------------------------------------------------------------
// Config handlers
// ---------------------------------------------------------------------------

fn handle_config(action: &ConfigAction) {
    let mut config = load_or_exit();

    match action {
        ConfigAction::Show => {
            println!("{}", serde_json::to_string_pretty(&config).unwrap());
        }

        ConfigAction::SetTheme { theme } => {
            config.theme = match theme {
                CliTheme::Light => Theme::Light,
                CliTheme::Dark => Theme::Dark,
                CliTheme::System => Theme::System,
            };
            save_or_exit(&config);
            println!("Theme set to {:?}", config.theme);
        }

        ConfigAction::SetDev { state } => {
            if state.0 {
                let active_idx = config
                    .profiles
                    .iter()
                    .position(|p| p.id == config.active_profile);
                if let Some(idx) = active_idx {
                    config.profiles[idx].auth_mode = AuthMode::Dev;
                }
                save_or_exit(&config);
                println!("Developer mode enabled on active profile");
            } else {
                let active_idx = config
                    .profiles
                    .iter()
                    .position(|p| p.id == config.active_profile);
                if let Some(idx) = active_idx {
                    if config.profiles[idx].auth_mode == AuthMode::Dev {
                        config.profiles[idx].auth_mode = AuthMode::OfflineToken;
                    }
                }
                save_or_exit(&config);
                println!("Developer mode disabled on active profile");
            }
        }

        ConfigAction::Import { file } => {
            let contents = match std::fs::read_to_string(file) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error reading '{file}': {e}");
                    process::exit(1);
                }
            };
            let imported: AppConfig = match serde_json::from_str(&contents) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: invalid config JSON: {e}");
                    process::exit(1);
                }
            };
            save_or_exit(&imported);
            println!("Configuration imported from '{file}'");
        }

        ConfigAction::Export { file } => {
            let json = serde_json::to_string_pretty(&config).unwrap();
            match file {
                Some(path) => {
                    if let Err(e) = std::fs::write(path, &json) {
                        eprintln!("Error writing '{path}': {e}");
                        process::exit(1);
                    }
                    println!("Configuration exported to '{path}'");
                }
                None => println!("{json}"),
            }
        }

        ConfigAction::Path => {
            println!("{}", AppConfig::config_path().display());
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_or_exit() -> AppConfig {
    match AppConfig::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error loading configuration: {e}");
            process::exit(1);
        }
    }
}

fn save_or_exit(config: &AppConfig) {
    if let Err(e) = config.save() {
        eprintln!("Error saving configuration: {e}");
        process::exit(1);
    }
}
