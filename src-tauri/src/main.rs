#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod auth;
mod cli;
mod config;
mod proxy;

use auth::{
    AuthProvider, DevAuthProvider, OfflineTokenAuthProvider, PasswordAuthProvider,
    ServiceAccountAuthProvider,
};
use config::{AppConfig, AuthMode, ConnectionProfile, Theme};
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

fn find_base_path() -> PathBuf {
    let dev_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    if dev_root.join("ui").is_dir() && dev_root.join("settings").is_dir() {
        return dev_root;
    }

    let exe = std::env::current_exe().expect("failed to find executable path");
    let exe_dir = exe.parent().expect("failed to find executable directory");

    #[cfg(target_os = "linux")]
    {
        for name in &["Cost Management Desktop", "koku-desktop"] {
            let lib_dir = exe_dir.join("../lib").join(name);
            if lib_dir.join("ui").is_dir() {
                return lib_dir;
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let resources = exe_dir.join("../Resources");
        if resources.join("ui").is_dir() {
            return resources;
        }
    }

    exe_dir.to_path_buf()
}

fn create_auth_provider(
    profile: &ConnectionProfile,
    dev_identity: &serde_json::Value,
) -> anyhow::Result<Arc<dyn AuthProvider>> {
    match profile.auth_mode {
        AuthMode::Dev => Ok(Arc::new(DevAuthProvider::new(dev_identity.clone())?)),
        AuthMode::ServiceAccount => Ok(Arc::new(ServiceAccountAuthProvider::new(
            profile.effective_token_endpoint().to_string(),
            profile.service_account.client_id.clone(),
            profile.service_account.client_secret.clone(),
            profile.service_account.display_name.clone(),
            profile.is_saas(),
        ))),
        AuthMode::OfflineToken => Ok(Arc::new(OfflineTokenAuthProvider::new(
            profile.effective_token_endpoint().to_string(),
            profile.offline_token.clone(),
            profile.is_saas(),
        ))),
        AuthMode::Password => Ok(Arc::new(PasswordAuthProvider::new(
            profile.effective_token_endpoint().to_string(),
            profile.service_account.client_id.clone(),
            profile.service_account.client_secret.clone(),
            profile.kc_username.clone(),
            profile.kc_password.clone(),
        ))),
    }
}

fn proxy_url(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{port}{path}")
}

fn navigate(app: &AppHandle, path: &str) {
    let state = app.state::<AppState>();
    if let Some(window) = app.get_webview_window("main") {
        if path.starts_with("/_") {
            let url = proxy_url(state.proxy_port, path);
            let script = format!("window.location.href = {url:?};");
            let _ = window.eval(&script);
        } else {
            let script = format!(
                "if (typeof window.kdNavigate === 'function') {{ window.kdNavigate({path:?}); }} \
                 else {{ window.location.href = {path:?}; }}"
            );
            let _ = window.eval(&script);
        }
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
            &MenuItem::with_id(app, "tray_open", "Open Lightspeed Cost Management", true, None::<&str>)?,
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
    let profile = config.active_profile().clone();
    let auth =
        create_auth_provider(&profile, &config.dev_identity).map_err(|e| e.to_string())?;

    *state.config.write().await = config;
    *state.auth.write().await = auth;

    Ok(())
}

#[tauri::command]
async fn test_connection(
    url: String,
    token_endpoint: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    username: Option<String>,
    password: Option<String>,
) -> Result<serde_json::Value, String> {
    let base = url.trim_end_matches('/');
    let stripped = base.trim_end_matches("/api");
    let status_url = format!("{stripped}/api/cost-management/v1/status/");

    log::info!("test_connection: url={base}");
    log::info!("test_connection: token_endpoint={:?}", token_endpoint);
    log::info!("test_connection: client_id={:?}", client_id);
    log::info!(
        "test_connection: client_secret={}",
        if client_secret.as_ref().map_or(true, |s| s.is_empty()) {
            "(empty)"
        } else {
            "(set)"
        }
    );
    log::info!("test_connection: username={:?}", username);
    log::info!(
        "test_connection: password={}",
        if password.as_ref().map_or(true, |s| s.is_empty()) {
            "(empty)"
        } else {
            "(set)"
        }
    );

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| e.to_string())?;

    let mut req = client.get(&status_url);
    let mut got_token = false;

    if let (Some(endpoint), Some(cid), Some(csecret)) =
        (&token_endpoint, &client_id, &client_secret)
    {
        if !endpoint.is_empty() && !cid.is_empty() {
            let mut params = vec![
                ("client_id", cid.as_str()),
                ("client_secret", csecret.as_str()),
            ];
            if let (Some(u), Some(p)) = (&username, &password) {
                if !u.is_empty() {
                    params.push(("grant_type", "password"));
                    params.push(("username", u.as_str()));
                    params.push(("password", p.as_str()));
                } else {
                    params.push(("grant_type", "client_credentials"));
                }
            } else {
                params.push(("grant_type", "client_credentials"));
            }

            log::info!(
                "test_connection: requesting token from {} with grant_type={}",
                endpoint,
                params.iter().find(|(k, _)| *k == "grant_type").map(|(_, v)| *v).unwrap_or("?")
            );

            match client.post(endpoint.as_str()).form(&params).send().await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        if let Some(token) = body.get("access_token").and_then(|v| v.as_str()) {
                            log::info!(
                                "test_connection: got access_token (len={})",
                                token.len()
                            );
                            req = req.bearer_auth(token);
                            got_token = true;
                        } else {
                            log::warn!(
                                "test_connection: token response has no access_token: {:?}",
                                body
                            );
                        }
                    }
                }
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let body = resp.text().await.unwrap_or_default();
                    log::error!(
                        "test_connection: token endpoint returned {status}: {body}"
                    );
                    return Ok(serde_json::json!({
                        "success": false,
                        "error": format!("Token endpoint returned {status}: {body}"),
                    }));
                }
                Err(err) => {
                    log::error!("test_connection: token endpoint error: {err}");
                    return Ok(serde_json::json!({
                        "success": false,
                        "error": format!("Failed to reach token endpoint: {err}"),
                    }));
                }
            }
        } else {
            log::warn!(
                "test_connection: skipping auth — endpoint or client_id is empty"
            );
        }
    } else {
        log::warn!("test_connection: no auth params provided (all None)");
    }

    log::info!(
        "test_connection: GET {status_url} (auth={})",
        if got_token { "Bearer" } else { "none" }
    );

    match req.send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let success = response.status().is_success();
            let body = response.text().await.unwrap_or_default();
            log::info!("test_connection: API returned {status}");
            Ok(serde_json::json!({
                "success": success,
                "status": status,
                "body": body,
            }))
        }
        Err(err) => {
            log::error!("test_connection: request error: {err}");
            Ok(serde_json::json!({
                "success": false,
                "error": err.to_string(),
            }))
        }
    }
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
async fn save_blob_download(
    app: AppHandle,
    data_base64: String,
    filename: String,
) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &data_base64,
    )
    .map_err(|e| format!("base64 decode error: {e}"))?;

    let download_dir = dirs::download_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Downloads")
    });

    let dest = app
        .dialog()
        .file()
        .set_file_name(&filename)
        .set_directory(&download_dir)
        .blocking_save_file();

    let target: PathBuf = match dest {
        Some(path) => path.as_path().map(|p| p.to_path_buf()).ok_or("invalid path")?,
        None => return Ok(String::new()),
    };

    std::fs::write(&target, &bytes).map_err(|e| format!("write error: {e}"))?;
    log::info!("Blob download saved: {}", target.display());
    Ok(target.display().to_string())
}

#[tauri::command]
fn get_about_info() -> Result<serde_json::Value, String> {
    let os = os_info::get();
    Ok(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "build_id": env!("BUILD_ID"),
        "build_date": env!("BUILD_DATE"),
        "git_hash": env!("GIT_HASH"),
        "ui_date": env!("UI_DATE"),
        "ui_hash": env!("UI_HASH"),
        "ui_ref": env!("UI_REF"),
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
        let server_url = config.active_profile().effective_server_url().trim_end_matches('/').to_string();
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
    let cli = match cli::run() {
        Some(c) => c,
        None => return, // subcommand handled, exit
    };

    let log_dir = AppConfig::config_dir().join("logs");
    let _ = std::fs::create_dir_all(&log_dir);

    let mut loaded_config = AppConfig::load().expect("failed to load configuration");
    let first_launch = AppConfig::is_first_launch();

    if let Some(ref profile_name) = cli.profile_override {
        match loaded_config.profiles.iter().find(|p| p.name == *profile_name) {
            Some(p) => loaded_config.active_profile = p.id.clone(),
            None => {
                eprintln!("Error: profile '{profile_name}' not found");
                std::process::exit(1);
            }
        }
    }

    let active = loaded_config.active_profile().clone();
    let auth_provider = create_auth_provider(&active, &loaded_config.dev_identity)
        .expect("failed to create auth provider");
    let config = Arc::new(RwLock::new(loaded_config));
    let auth = Arc::new(RwLock::new(auth_provider));

    let base_path = find_base_path();
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

            let splash_url = proxy_url(proxy_port, "/_splash/");
            let actual_start_url = start_url.clone();
            #[allow(unused_mut)]
            let mut win_builder = WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::External(
                    splash_url
                        .parse()
                        .map_err(|e| anyhow::anyhow!("invalid splash URL: {e}"))?,
                ),
            )
            .title("Red Hat Lightspeed Cost Management Desktop")
            .inner_size(1280.0, 800.0)
            .min_inner_size(800.0, 600.0);

            // On Windows and macOS, disable native decorations since we draw our
            // own titlebar (see proxy.rs). On Linux, we keep decorations enabled
            // and use a GTK hack to hide them (see below).
            #[cfg(not(target_os = "linux"))]
            {
                win_builder = win_builder.decorations(false);
            }

            let main_window = win_builder
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

            // Belt-and-suspenders: also set decorations(false) after the window
            // is built. On Windows, the builder setting alone can be ignored when
            // WebView2 initializes and resets the window style flags.
            #[cfg(not(target_os = "linux"))]
            {
                let _ = main_window.set_decorations(false);
            }

            // GNOME/Wayland titlebar removal hack
            //
            // Problem: koku-desktop uses a custom titlebar injected into the webview
            // (see proxy.rs build_head_injection). On GNOME/Wayland, the native
            // titlebar cannot be removed through Tauri's config alone:
            //
            //   - `decorations: false` in tauri.conf.json does NOT work on GNOME.
            //     GNOME's Mutter compositor draws server-side decorations (SSD)
            //     regardless, resulting in a duplicate titlebar.
            //
            //   - `gtk_window.set_titlebar(None)` removes GTK's client-side
            //     decorations (CSD), but GNOME interprets the absence of CSD as a
            //     signal to draw SSD — again producing a duplicate titlebar.
            //
            // Solution (two parts):
            //
            // 1. Set an invisible zero-height GTK widget as the titlebar. This
            //    tells GNOME "this app draws its own titlebar" so it won't add
            //    SSD, but the widget is invisible so only our webview-injected
            //    titlebar is visible.
            //
            // 2. Override GTK CSS to eliminate all space reserved for the
            //    decoration frame: margins, padding, border-radius, and
            //    box-shadow on the `decoration`, `headerbar`, `.titlebar`, and
            //    `window.background.csd` nodes. Without this, GTK reserves a
            //    transparent gap around the window where the decoration shadow
            //    would normally render.
            //
            // Because window controls are hidden along with the native titlebar,
            // custom minimize/maximize/close buttons are injected into the webview
            // titlebar (see buildWinControls() in proxy.rs) and wired to Tauri's
            // Window API.
            //
            // tauri.conf.json must keep `decorations: true` so tao creates a CSD
            // window, which is what makes the empty-titlebar trick work.
            //
            // The `set_titlebar()` call on a realized window emits a GTK warning
            // ("gtk_window_set_titlebar() called on a realized window"). This is
            // harmless — Tauri realizes the window during `.build()` and there is
            // no pre-realization hook. The call still takes effect.
            //
            // References:
            //   - https://github.com/tauri-apps/tauri/issues/13142
            //   - https://github.com/tauri-apps/tao/issues/1046
            //   - https://github.com/velitasali/gtktitlebar (GNOME extension approach)
            #[cfg(target_os = "linux")]
            {
                use gtk::prelude::{CssProviderExt, GtkWindowExt, StyleContextExt, WidgetExt};
                use gtk::glib::object::ObjectExt;

                // Tell GTK to prefer dark theme so tray menu text is
                // readable on GNOME dark backgrounds.
                if let Some(settings) = gtk::Settings::default() {
                    let prefers_dark = std::process::Command::new("gsettings")
                        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
                        .output()
                        .ok()
                        .and_then(|o| String::from_utf8(o.stdout).ok())
                        .map(|s| s.contains("dark"))
                        .unwrap_or(false);
                    settings.set_property("gtk-application-prefer-dark-theme", prefers_dark);
                }

                let gtk_window = main_window.gtk_window()
                    .map_err(|e| anyhow::anyhow!("failed to get GTK window: {e}"))?;

                gtk_window.style_context().add_class("kd-main");

                let css = gtk::CssProvider::new();
                css.load_from_data(
                    b"window.kd-main headerbar, \
                      window.kd-main .titlebar { \
                        min-height: 0; padding: 0; margin: 0; border: 0; \
                        background: transparent; box-shadow: none; \
                      } \
                      window.kd-main decoration, \
                      window.kd-main decoration:backdrop { \
                        margin: 0; border: none; padding: 0; \
                        box-shadow: none; border-radius: 0; \
                        outline: none; \
                      } \
                      window.kd-main.background.csd { \
                        border-radius: 0; box-shadow: none; \
                        margin: 0; padding: 0; border: none; outline: none; \
                      }",
                )?;
                gtk::StyleContext::add_provider_for_screen(
                    &gtk::gdk::Screen::default()
                        .ok_or_else(|| anyhow::anyhow!("no default GDK screen"))?,
                    &css,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
                );

                let empty = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                empty.set_size_request(-1, 0);
                gtk_window.set_titlebar(Some(&empty));
            }

            // Navigate from splash to actual content after a short delay
            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                if let Some(win) = app_handle.get_webview_window("main") {
                    let _ = win.navigate(
                        actual_start_url.parse().expect("invalid start URL"),
                    );
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            test_connection,
            get_about_info,
            get_server_status,
            quit_app,
            save_blob_download,
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
