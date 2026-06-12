use crate::auth::AuthProvider;
use crate::config::{AppConfig, Theme};
use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Router,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

#[derive(Clone)]
struct ProxyState {
    config: Arc<RwLock<AppConfig>>,
    auth: Arc<RwLock<Arc<dyn AuthProvider>>>,
    client: reqwest::Client,
    base_path: PathBuf,
}

pub struct ProxyServer {
    port: u16,
    _shutdown_tx: tokio::sync::oneshot::Sender<()>,
    _handle: tokio::task::JoinHandle<()>,
}

impl ProxyServer {
    pub async fn start(
        config: Arc<RwLock<AppConfig>>,
        auth: Arc<RwLock<Arc<dyn AuthProvider>>>,
        base_path: PathBuf,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("building HTTP client for reverse proxy")?;

        let state = ProxyState {
            config,
            auth,
            client,
            base_path,
        };

        let app = build_router(state);

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("binding proxy server to 127.0.0.1:0")?;
        let port = listener.local_addr()?.port();

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        let handle = tokio::spawn(async move {
            let server = axum::serve(listener, app).with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            });
            if let Err(err) = server.await {
                log::error!("proxy server error: {err}");
            }
        });

        Ok(Self {
            port,
            _shutdown_tx: shutdown_tx,
            _handle: handle,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

fn build_router(state: ProxyState) -> Router {
    let settings_dir = state.base_path.join("settings");
    let about_dir = state.base_path.join("about");
    let splash_dir = state.base_path.join("splash");

    Router::new()
        .route("/api/me", get(api_me))
        .route(
            "/costManagementRos/plugin-manifest.json",
            get(ros_plugin_manifest),
        )
        .route("/sources/plugin-manifest.json", get(sources_plugin_manifest))
        .nest_service("/_settings", ServeDir::new(settings_dir))
        .nest_service("/_about", ServeDir::new(about_dir))
        .nest_service("/_splash", ServeDir::new(splash_dir))
        .fallback(any(fallback_handler))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn api_me(State(state): State<ProxyState>) -> impl IntoResponse {
    let auth = state.auth.read().await;
    let info = auth.user_info();
    axum::Json(info)
}

async fn ros_plugin_manifest(State(state): State<ProxyState>) -> Response {
    let config = state.config.read().await;
    if !config.modules.ros {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve_ui_file(&state, "costManagementRos/plugin-manifest.json").await
}

async fn sources_plugin_manifest(State(state): State<ProxyState>) -> Response {
    let config = state.config.read().await;
    if !config.modules.sources {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve_ui_file(&state, "sources/plugin-manifest.json").await
}

async fn fallback_handler(State(state): State<ProxyState>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    if path.starts_with("/api/") {
        return proxy_api(state, request).await;
    }
    serve_ui(state, request).await
}

async fn proxy_api(state: ProxyState, request: Request) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let headers = request.headers().clone();
    let body = request.into_body();

    let config = state.config.read().await;
    let server_url = config.server_url.trim_end_matches('/').to_string();
    drop(config);

    let path_and_query = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let target_url = format!("{server_url}{path_and_query}");

    let body_bytes = match axum::body::to_bytes(body, 50 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(err) => {
            log::error!("failed to read proxy request body: {err}");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let mut req_builder = state.client.request(method, &target_url);

    for (name, value) in headers.iter() {
        if name == header::HOST || name == header::CONNECTION {
            continue;
        }
        req_builder = req_builder.header(name, value);
    }

    let auth = state.auth.read().await;
    for (name, value) in auth.request_headers().iter() {
        req_builder = req_builder.header(name, value);
    }
    drop(auth);

    let upstream = match req_builder.body(body_bytes).send().await {
        Ok(resp) => resp,
        Err(err) => {
            log::error!("proxy request to {target_url} failed: {err}");
            return (
                StatusCode::BAD_GATEWAY,
                format!("Failed to reach server: {err}"),
            )
                .into_response();
        }
    };

    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::OK);
    let upstream_body = match upstream.bytes().await {
        Ok(bytes) => bytes,
        Err(err) => {
            log::error!("failed to read upstream response body: {err}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };

    let mut response = Response::new(Body::from(upstream_body));
    *response.status_mut() = status;
    for (name, value) in upstream.headers().iter() {
        if name == header::TRANSFER_ENCODING || name == header::CONNECTION {
            continue;
        }
        response.headers_mut().insert(name.clone(), value.clone());
    }
    response
}

async fn serve_ui(state: ProxyState, request: Request) -> Response {
    let path = request.uri().path();
    let relative = path.trim_start_matches('/');
    let ui_root = state.base_path.join("ui");
    let file_path = if relative.is_empty() {
        ui_root.join("index.html")
    } else {
        ui_root.join(relative)
    };

    if file_path.is_file() {
        return serve_path(&state, &file_path, path.ends_with(".html")).await;
    }

    let index_path = ui_root.join("index.html");
    if index_path.is_file() {
        return serve_path(&state, &index_path, true).await;
    }

    missing_ui_response().into_response()
}

async fn serve_ui_file(state: &ProxyState, relative: &str) -> Response {
    let file_path = state.base_path.join("ui").join(relative);
    if file_path.is_file() {
        serve_path(state, &file_path, false).await
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn serve_path(state: &ProxyState, path: &Path, inject_theme: bool) -> Response {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(err) => {
            log::error!("failed to read {}: {err}", path.display());
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mime = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();

    if inject_theme || path.file_name().is_some_and(|n| n == "index.html") {
        if mime.contains("html") {
            let config = state.config.read().await;
            let html = inject_theme_script(&bytes, &config);
            drop(config);
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(Body::from(html))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn inject_theme_script(html_bytes: &[u8], config: &AppConfig) -> Vec<u8> {
    let script = theme_script(config);
    let html = String::from_utf8_lossy(html_bytes);
    let injected = if let Some(pos) = html.find("</head>") {
        let mut out = String::with_capacity(html.len() + script.len());
        out.push_str(&html[..pos]);
        out.push_str(&script);
        out.push_str(&html[pos..]);
        out
    } else {
        format!("{script}{html}")
    };
    injected.into_bytes()
}

fn theme_script(config: &AppConfig) -> String {
    let toggle_expr = match config.theme {
        Theme::Light => "false".to_string(),
        Theme::Dark => "true".to_string(),
        Theme::System => {
            "window.matchMedia('(prefers-color-scheme: dark)').matches".to_string()
        }
    };
    format!(
        "<script>document.documentElement.classList.toggle('pf-v6-theme-dark', {toggle_expr});</script>"
    )
}

fn missing_ui_response() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Cost Management Desktop</title>
</head>
<body>
  <h1>UI not built yet</h1>
  <p>The koku-ui static build has not been copied to the <code>ui/</code> directory.</p>
  <p>Run <code>scripts/build-ui.sh</code> to build koku-ui, or open
     <a href="/_settings/">Settings</a> to configure the server connection.</p>
</body>
</html>"#,
    )
}
