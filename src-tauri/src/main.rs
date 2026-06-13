#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod auth;
mod config;
mod proxy;

use auth::{AuthProvider, DevAuthProvider, OidcAuthProvider};
use config::{AppConfig, AuthMode, Theme};
use proxy::ProxyServer;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::{DownloadEvent, WebviewWindowBuilder},
    AppHandle, Manager, RunEvent, State, WebviewUrl,
};

use tauri_plugin_log::{Target, TargetKind};
use tokio::sync::RwLock;

struct AppState {
    config: Arc<RwLock<AppConfig>>,
    auth: Arc<RwLock<Arc<dyn AuthProvider>>>,
    proxy_port: u16,
}

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn create_auth_provider(config: &AppConfig) -> anyhow::Result<Arc<dyn AuthProvider>> {
    match config.auth_mode {
        AuthMode::Dev => Ok(Arc::new(DevAuthProvider::new(
            config.dev_identity.clone(),
        )?)),
        AuthMode::Oidc => Ok(Arc::new(OidcAuthProvider::new(&config.oidc)?)),
    }
}

fn proxy_url(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{port}{path}")
}

fn navigate(app: &AppHandle, path: &str) {
    let state = app.state::<AppState>();
    let url = proxy_url(state.proxy_port, path);
    if let Some(window) = app.get_webview_window("main") {
        let script = format!("window.location.href = {url:?};");
        let _ = window.eval(&script);
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn hide_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

fn toggle_theme(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut config = tauri::async_runtime::block_on(state.config.write());
    config.theme = match config.theme {
        Theme::Dark => Theme::Light,
        _ => Theme::Dark,
    };
    let _ = config.save();
    drop(config);

    navigate(app, "/");
}

fn build_tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, "tray_open", "Open Cost Management", true, None::<&str>)?,
            &MenuItem::with_id(app, "tray_settings", "Settings", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "tray_quit", "Quit", true, None::<&str>)?,
        ],
    )
}

fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        "file_settings" | "tray_settings" => navigate(app, "/_settings/"),
        "file_print" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.print();
            }
        }
        "file_quit" | "tray_quit" => {
            app.exit(0);
        }
        "nav_overview" => navigate(app, "/openshift/cost-management/"),
        "nav_openshift" => navigate(app, "/openshift/cost-management/ocp"),
        "nav_aws" => navigate(app, "/openshift/cost-management/aws"),
        "nav_azure" => navigate(app, "/openshift/cost-management/azure"),
        "nav_gcp" => navigate(app, "/openshift/cost-management/gcp"),
        "nav_explorer" => navigate(app, "/openshift/cost-management/explorer"),
        "nav_optimizations" => navigate(app, "/openshift/cost-management/optimizations"),
        "nav_settings" => navigate(app, "/openshift/cost-management/settings"),
        "view_theme" => toggle_theme(app),
        "help_about" => navigate(app, "/_about/"),
        "tray_open" => show_main_window(app),
        _ => {}
    }
}

fn default_download_filename(url: &url::Url) -> String {
    url.path_segments()
        .and_then(|segments| segments.last())
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string())
        .unwrap_or_else(|| "download.csv".to_string())
}

fn unique_path(path: &std::path::Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let parent = path.parent().unwrap_or(std::path::Path::new("."));
    for i in 1..1000 {
        let name = if ext.is_empty() {
            format!("{stem} ({i})")
        } else {
            format!("{stem} ({i}).{ext}")
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> Result<AppConfig, String> {
    Ok(tauri::async_runtime::block_on(state.config.read()).clone())
}

#[tauri::command]
async fn save_config(state: State<'_, AppState>, config: AppConfig) -> Result<(), String> {
    config.save().map_err(|e| e.to_string())?;
    let auth = create_auth_provider(&config).map_err(|e| e.to_string())?;

    *state.config.write().await = config;
    *state.auth.write().await = auth;

    Ok(())
}

#[tauri::command]
async fn test_connection(url: String) -> Result<serde_json::Value, String> {
    let base = url.trim_end_matches('/');
    let status_url = format!("{base}/api/cost-management/v1/status/");

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| e.to_string())?;
    match client.get(&status_url).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let success = response.status().is_success();
            let body = response.text().await.unwrap_or_default();
            Ok(serde_json::json!({
                "success": success,
                "status": status,
                "body": body,
            }))
        }
        Err(err) => Ok(serde_json::json!({
            "success": false,
            "error": err.to_string(),
        })),
    }
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn get_about_info() -> Result<serde_json::Value, String> {
    let os = os_info::get();
    Ok(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "build_date": env!("BUILD_DATE"),
        "git_hash": env!("GIT_HASH"),
        "os_name": os.os_type().to_string(),
        "os_version": os.version().to_string(),
        "os_arch": std::env::consts::ARCH,
        "tauri_version": tauri::VERSION,
    }))
}

#[tauri::command]
async fn get_server_status(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let (server_url, auth) = {
        let config = state.config.read().await;
        let server_url = config.server_url.trim_end_matches('/').to_string();
        let auth = state.auth.read().await.clone();
        (server_url, auth)
    };

    let status_url = format!("{server_url}/api/cost-management/v1/status/");
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| e.to_string())?;
    let mut request = client.get(&status_url);
    for (name, value) in auth.request_headers().iter() {
        request = request.header(name, value);
    }

    match request.send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let success = response.status().is_success();
            let body: serde_json::Value = response
                .json()
                .await
                .unwrap_or(serde_json::json!({ "raw": "invalid JSON response" }));
            Ok(serde_json::json!({
                "success": success,
                "status": status,
                "body": body,
            }))
        }
        Err(err) => Ok(serde_json::json!({
            "success": false,
            "error": err.to_string(),
        })),
    }
}

fn main() {
    let log_dir = AppConfig::config_dir().join("logs");
    let _ = std::fs::create_dir_all(&log_dir);

    let loaded_config = AppConfig::load().expect("failed to load configuration");
    let first_launch = AppConfig::is_first_launch();
    let auth_provider =
        create_auth_provider(&loaded_config).expect("failed to create auth provider");
    let config = Arc::new(RwLock::new(loaded_config));
    let auth = Arc::new(RwLock::new(auth_provider));

    let base_path = project_root();
    let config_for_proxy = config.clone();
    let auth_for_proxy = auth.clone();

    let proxy = tauri::async_runtime::block_on(async {
        ProxyServer::start(config_for_proxy, auth_for_proxy, base_path)
            .await
            .expect("failed to start proxy server")
    });
    let proxy_port = proxy.port();

    let app_state = AppState {
        config,
        auth,
        proxy_port,
    };

    let start_path = if first_launch {
        "/_settings/"
    } else {
        "/"
    };
    let start_url = proxy_url(proxy_port, start_path);

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::Folder {
                        path: log_dir,
                        file_name: Some("koku-desktop".to_string()),
                    }),
                ])
                .build(),
        )
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        // .plugin(tauri_plugin_updater::Builder::new().build()) // TODO: enable when pubkey is configured
        .manage(app_state)
        .setup(move |app| {
            let tray_menu = build_tray_menu(app.handle())?;
            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    handle_menu_event(app, event.id().as_ref());
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            let download_dir = dirs::download_dir().unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("Downloads")
            });

            let main_window = WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::External(
                    start_url
                        .parse()
                        .map_err(|e| anyhow::anyhow!("invalid start URL: {e}"))?,
                ),
            )
            .title("Cost Management")
            .inner_size(1280.0, 800.0)
            .min_inner_size(800.0, 600.0)
            .on_download(move |_webview, event| match event {
                DownloadEvent::Requested { url, destination } => {
                    let filename = default_download_filename(&url);
                    let target = download_dir.join(&filename);
                    let target = unique_path(&target);
                    log::info!("Download starting: {} -> {}", url, target.display());
                    *destination = target;
                    true
                }
                DownloadEvent::Finished { success, .. } => {
                    if !success {
                        log::error!("Download failed");
                    }
                    true
                }
                _ => true,
            })
            .build()?;

            main_window.navigate(
                start_url
                    .parse()
                    .map_err(|e| anyhow::anyhow!("invalid start URL: {e}"))?,
            )?;
            main_window.show()?;
            main_window.set_focus()?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            test_connection,
            get_about_info,
            get_server_status,
            quit_app,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Tauri application")
        .run(|app, event| {
            if let RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } = event
            {
                if label == "main" {
                    api.prevent_close();
                    hide_main_window(app);
                }
            }
        });
}
