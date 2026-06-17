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
            .danger_accept_invalid_certs(true)
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
    let splash_dir = state.base_path.join("splash");

    Router::new()
        .route("/api/me", get(api_me))
        .route(
            "/costManagementRos/plugin-manifest.json",
            get(ros_plugin_manifest),
        )
        .route("/sources/plugin-manifest.json", get(sources_plugin_manifest))
        .route("/_settings/{*rest}", get(serve_custom_page))
        .route("/_settings/", get(serve_custom_page))
        .route("/_about/{*rest}", get(serve_custom_page))
        .route("/_about/", get(serve_custom_page))
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

async fn serve_custom_page(State(state): State<ProxyState>, request: Request) -> Response {
    let path = request.uri().path();
    let dir_name = path
        .strip_prefix("/_")
        .and_then(|p| p.split('/').next())
        .unwrap_or("settings");
    let relative = path
        .strip_prefix(&format!("/_{dir_name}"))
        .unwrap_or("/")
        .trim_start_matches('/');
    let page_dir = state.base_path.join(dir_name);
    let file_path = if relative.is_empty() {
        page_dir.join("index.html")
    } else {
        page_dir.join(relative)
    };
    if file_path.is_file() {
        return serve_path(&state, &file_path, true).await;
    }
    StatusCode::NOT_FOUND.into_response()
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
    let server_url = config.active_profile().effective_server_url().trim_end_matches('/').to_string();
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
    let upstream_headers = upstream.headers().clone();
    let upstream_body = match upstream.bytes().await {
        Ok(bytes) => bytes,
        Err(err) => {
            log::error!("failed to read upstream response body: {err}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };

    let mut response = Response::new(Body::from(upstream_body));
    *response.status_mut() = status;
    for (name, value) in upstream_headers.iter() {
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

    // On Windows, "exposed-." directories are renamed to "exposed-_dot" because
    // NTFS silently strips trailing dots. Try the renamed path as a fallback.
    #[cfg(target_os = "windows")]
    if !relative.is_empty() {
        let fixed = relative.replace("exposed-.", "exposed-_dot");
        if fixed != relative {
            let alt_path = ui_root.join(&fixed);
            if alt_path.is_file() {
                return serve_path(&state, &alt_path, path.ends_with(".html")).await;
            }
        }
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
    let head_injection = build_head_injection(config);
    let html = String::from_utf8_lossy(html_bytes);
    let injected = if let Some(pos) = html.find("</head>") {
        let mut out = String::with_capacity(html.len() + head_injection.len());
        out.push_str(&html[..pos]);
        out.push_str(&head_injection);
        out.push_str(&html[pos..]);
        out
    } else {
        format!("{head_injection}{html}")
    };
    injected.into_bytes()
}

fn build_head_injection(config: &AppConfig) -> String {
    let toggle_expr = match config.theme {
        Theme::Light => "false",
        Theme::Dark => "true",
        Theme::System => "window.matchMedia('(prefers-color-scheme: dark)').matches",
    };

    format!(
        r##"
<script>document.documentElement.classList.toggle('pf-v6-theme-dark', {toggle_expr});</script>
<style>
:root {{
  --kd-h: 36px;
  --kd-bar-bg: #f0f0f0;
  --kd-bar-fg: #1b1d21;
  --kd-bar-fg2: #6a6e73;
  --kd-bar-hover: rgba(0,0,0,.08);
  --kd-bar-brd: #d2d2d2;
  --kd-dd-bg: #fff;
  --kd-dd-fg: #1b1d21;
  --kd-dd-hover-bg: #3574f0;
  --kd-dd-hover-fg: #fff;
  --kd-dd-brd: #d2d2d2;
  --kd-dd-shortcut: #6a6e73;
}}
.pf-v6-theme-dark {{
  --kd-bar-bg: #212427;
  --kd-bar-fg: #e0e0e0;
  --kd-bar-fg2: #999;
  --kd-bar-hover: rgba(255,255,255,.1);
  --kd-bar-brd: #3c3f42;
  --kd-dd-bg: #2b2d30;
  --kd-dd-fg: #ddd;
  --kd-dd-hover-bg: #3574f0;
  --kd-dd-hover-fg: #fff;
  --kd-dd-brd: #444;
  --kd-dd-shortcut: #888;
}}

/* === when masthead becomes our bar === */
.pf-v6-c-masthead.kd-merged {{
  position: fixed !important; top: 0 !important; left: 0 !important; right: 0 !important;
  height: var(--kd-h) !important; min-height: var(--kd-h) !important;
  max-height: var(--kd-h) !important;
  z-index: 999999 !important;
  display: flex !important; align-items: center !important;
  background: var(--kd-bar-bg) !important; color: var(--kd-bar-fg) !important;
  padding: 0 !important; margin: 0 !important;
  border-bottom: 1px solid var(--kd-bar-brd) !important;
  font: 500 12px/1 "RedHatText","Red Hat Text",system-ui,sans-serif;
  user-select: none; -webkit-user-select: none;
  overflow: visible !important;
}}
body.kd-ready {{
  padding-top: var(--kd-h) !important;
  /* overflow: hidden + height: 100vh on body, combined with the .pf-v6-c-page
     height below, confines the scrollbar to the content area below the fixed
     masthead/titlebar. Without this the browser's native scrollbar runs the
     full viewport height and overlaps the window control buttons. */
  overflow: hidden !important; height: 100vh !important;
}}
body.kd-ready .pf-v6-c-page {{
  height: calc(100vh - var(--kd-h)) !important;
  overflow: hidden !important;
}}
body.kd-ready .pf-v6-c-page__main {{
  overflow-y: auto !important;
  min-height: 0 !important;
}}

/* hide brand/logo */
.kd-merged .pf-v6-c-masthead__brand {{ display: none !important; }}

/* compact the toggle (PF hamburger) */
.kd-merged .pf-v6-c-masthead__toggle {{ flex-shrink: 0; }}
.kd-merged .pf-v6-c-masthead__toggle button {{
  width: 36px; height: var(--kd-h); padding: 0;
  display: flex; align-items: center; justify-content: center;
  background: none !important; border: none !important; color: var(--kd-bar-fg) !important;
}}
.kd-merged .pf-v6-c-masthead__toggle button:hover {{ background: var(--kd-bar-hover) !important; }}

/* the content area (user profile) pushed to the right */
.kd-merged .pf-v6-c-masthead__content {{
  flex-shrink: 0 !important; flex-grow: 0 !important;
  margin-left: 0 !important; padding: 0 !important;
  display: flex !important; align-items: center !important;
  height: 100% !important;
}}
.kd-merged .pf-v6-c-toolbar {{ padding: 0 !important; background: transparent !important; height: 100%; }}
.kd-merged .pf-v6-c-toolbar__content {{ padding: 0 !important; height: 100%; }}
.kd-merged .pf-v6-c-toolbar__content-section {{ gap: 0; }}
.kd-merged .pf-v6-c-toolbar__item,
.kd-merged .pf-v6-c-toolbar__group {{
  height: var(--kd-h) !important; align-items: center;
}}
.kd-merged .pf-v6-c-menu-toggle {{
  font-size: 12px !important; padding: 2px 8px !important;
  height: 28px !important; gap: 6px !important;
}}
.kd-merged .pf-v6-c-avatar {{
  width: 20px !important; height: 20px !important;
  object-fit: cover !important;
}}

/* --- menus (injected into masthead) --- */
#kd-menus {{ display: flex; height: 100%; flex-shrink: 0; align-items: center; }}
.kd-menu-trigger {{
  padding: 0 10px; height: 100%; cursor: pointer;
  background: none; border: none; color: var(--kd-bar-fg2); font: inherit;
}}
.kd-menu-trigger:hover, .kd-menu-trigger.kd-active {{ background: var(--kd-bar-hover); color: var(--kd-bar-fg); }}

/* --- center drag region with title --- */
#kd-drag {{
  flex: 1; height: 100%; min-width: 0;
  display: flex; align-items: center; justify-content: center;
}}
#kd-drag span {{
  font-size: 12px; font-weight: 500; color: var(--kd-bar-fg2);
  pointer-events: none; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}}

/* --- dropdowns --- */
.kd-dropdown {{
  display: none; position: fixed;
  min-width: 200px; background: var(--kd-dd-bg); border: 1px solid var(--kd-dd-brd);
  box-shadow: 0 4px 16px rgba(0,0,0,.25); z-index: 1000000;
  padding: 4px 0; border-radius: 0 0 4px 4px;
  font: 500 12px/1 "RedHatText","Red Hat Text",system-ui,sans-serif;
  color: var(--kd-dd-fg);
}}
.kd-dropdown.kd-open {{ display: block; }}
.kd-dropdown .kd-item {{
  display: flex; align-items: center; justify-content: space-between;
  padding: 5px 14px; cursor: pointer; font-size: 12px;
  border: none; background: none; width: 100%; text-align: left;
  font: inherit; color: inherit;
}}
.kd-dropdown .kd-item:hover {{ background: var(--kd-dd-hover-bg); color: var(--kd-dd-hover-fg); }}
.kd-dropdown .kd-item:hover .kd-shortcut {{ color: rgba(255,255,255,.7); }}
.kd-dropdown .kd-sep {{ height: 1px; background: var(--kd-dd-brd); margin: 3px 8px; }}
.kd-dropdown .kd-shortcut {{ color: var(--kd-dd-shortcut); font-size: 11px; margin-left: 20px; }}

/* --- window controls (needed because native controls are hidden by the
   GNOME titlebar removal hack in main.rs — see comments there) --- */
#kd-wc {{ display: flex; height: 100%; flex-shrink: 0; }}
#kd-wc button {{
  width: 40px; height: 100%; border: none; background: none;
  color: var(--kd-bar-fg2); cursor: pointer; display: flex;
  align-items: center; justify-content: center;
}}
#kd-wc button:hover {{ background: var(--kd-bar-hover); }}
#kd-wc button.kd-close:hover {{ background: #c42b1c; color: #fff; }}
#kd-wc button svg {{ width: 10px; height: 10px; stroke: currentColor; stroke-width: 1.5; fill: none; }}

/* fix PF dropdown/menu z-index so they appear above our bar */
.pf-v6-c-menu {{ z-index: 1000001 !important; }}
.pf-v6-c-dropdown__menu {{ z-index: 1000001 !important; }}
.pf-v6-c-select__menu {{ z-index: 1000001 !important; }}
body > .pf-v6-c-popover {{ z-index: 1000001 !important; }}
[class*="pf-v6-c-menu"] {{ z-index: 1000001 !important; }}

/* === standalone fallback bar for non-koku-ui pages === */
#kd-bar {{
  position: fixed; top: 0; left: 0; right: 0;
  height: var(--kd-h); z-index: 999999;
  display: flex; align-items: center;
  background: var(--kd-bar-bg); color: var(--kd-bar-fg);
  font: 500 12px/1 "RedHatText","Red Hat Text",system-ui,sans-serif;
  user-select: none; -webkit-user-select: none;
  border-bottom: 1px solid var(--kd-bar-brd);
}}
</style>
<script>
document.addEventListener('DOMContentLoaded', function() {{
  if (document.querySelector('.kd-merged') || document.getElementById('kd-bar')) return;
  var T = window.__TAURI__;
  var invoke = T ? T.core.invoke : null;

  var menuDefs = {{
    file: {{
      label: 'File',
      items: [
        {{ label: 'App Settings', nav: '/_settings/' }},
        {{ label: 'Print', shortcut: 'Ctrl+P', action: 'print' }},
        {{ sep: true }},
        {{ label: 'Quit', shortcut: 'Ctrl+Q', action: 'quit' }}
      ]
    }},
    navigate: {{
      label: 'Navigate',
      items: [
        {{ label: 'Overview', shortcut: 'Ctrl+H', nav: '/openshift/cost-management/' }},
        {{ label: 'OpenShift', shortcut: 'Ctrl+O', nav: '/openshift/cost-management/ocp' }},
        {{ label: 'AWS', shortcut: 'Ctrl+W', nav: '/openshift/cost-management/aws' }},
        {{ label: 'Azure', shortcut: 'Ctrl+U', nav: '/openshift/cost-management/azure' }},
        {{ label: 'Google Cloud', shortcut: 'Ctrl+G', nav: '/openshift/cost-management/gcp' }},
        {{ label: 'Cost Explorer', shortcut: 'Ctrl+E', nav: '/openshift/cost-management/explorer' }},
        {{ label: 'Optimizations', nav: '/openshift/cost-management/optimizations' }},
        {{ sep: true }},
        {{ label: 'Cost Settings', shortcut: 'Ctrl+S', nav: '/openshift/cost-management/settings' }}
      ]
    }},
    iam: {{
      label: 'Identity & Access',
      items: [
        {{ label: 'Overview', shortcut: 'Ctrl+Shift+W', nav: '/iam/user-access/overview' }},
        {{ label: 'Users', shortcut: 'Ctrl+Shift+U', nav: '/iam/user-access/users' }},
        {{ label: 'Groups', shortcut: 'Ctrl+Shift+G', nav: '/iam/user-access/groups' }},
        {{ label: 'Roles', shortcut: 'Ctrl+Shift+R', nav: '/iam/user-access/roles' }}
      ]
    }},
    view: {{
      label: 'View',
      items: [
        {{ label: 'Toggle Theme', shortcut: 'Ctrl+T', action: 'theme' }}
      ]
    }},
    help: {{
      label: 'Help',
      items: [
        {{ label: 'About', nav: '/_about/' }}
      ]
    }}
  }};

  window.kdNavigate = function(path) {{
    var onIam = window.location.pathname.startsWith('/iam');
    var toIam = path.startsWith('/iam');
    if (path.startsWith('/_') || onIam !== toIam) {{
      window.location.href = path;
    }} else {{
      history.pushState(null, '', path);
      window.dispatchEvent(new PopStateEvent('popstate'));
    }}
  }};

  function doAction(act) {{
    if (act === 'print') window.print();
    else if (act === 'quit' && invoke) invoke('quit_app');
    else if (act === 'theme') document.documentElement.classList.toggle('pf-v6-theme-dark');
  }}

  var dropdowns = [];
  var triggers = [];

  function closeAll() {{
    dropdowns.forEach(function(d) {{ d.classList.remove('kd-open'); }});
    triggers.forEach(function(t) {{ t.classList.remove('kd-active'); }});
  }}

  function buildMenus() {{
    var menusDiv = document.createElement('div');
    menusDiv.id = 'kd-menus';
    Object.keys(menuDefs).forEach(function(key) {{
      var menu = menuDefs[key];
      var trigger = document.createElement('button');
      trigger.className = 'kd-menu-trigger';
      trigger.textContent = menu.label;
      triggers.push(trigger);

      var dd = document.createElement('div');
      dd.className = 'kd-dropdown';
      menu.items.forEach(function(item) {{
        if (item.sep) {{ var s = document.createElement('div'); s.className='kd-sep'; dd.appendChild(s); return; }}
        var btn = document.createElement('button');
        btn.className = 'kd-item';
        btn.innerHTML = item.label + (item.shortcut ? '<span class="kd-shortcut">' + item.shortcut + '</span>' : '');
        btn.addEventListener('click', function() {{ closeAll(); if (item.nav) kdNavigate(item.nav); if (item.action) doAction(item.action); }});
        dd.appendChild(btn);
      }});
      dropdowns.push(dd);

      trigger.addEventListener('click', function(e) {{
        e.stopPropagation();
        var isOpen = dd.classList.contains('kd-open');
        closeAll();
        if (!isOpen) {{
          var r = trigger.getBoundingClientRect();
          dd.style.left = r.left + 'px'; dd.style.top = r.bottom + 'px';
          dd.classList.add('kd-open'); trigger.classList.add('kd-active');
        }}
      }});
      trigger.addEventListener('mouseenter', function() {{
        if (dropdowns.some(function(d){{ return d.classList.contains('kd-open'); }})) {{
          closeAll();
          var r = trigger.getBoundingClientRect();
          dd.style.left = r.left + 'px'; dd.style.top = r.bottom + 'px';
          dd.classList.add('kd-open'); trigger.classList.add('kd-active');
        }}
      }});
      menusDiv.appendChild(trigger);
    }});
    return menusDiv;
  }}

  function buildDrag() {{
    var drag = document.createElement('div');
    drag.id = 'kd-drag';
    drag.setAttribute('data-tauri-drag-region', '');
    drag.innerHTML = '<span>Red Hat Lightspeed Cost Management Desktop</span>';
    return drag;
  }}

  function buildWinControls() {{
    var wc = document.createElement('div');
    wc.id = 'kd-wc';
    wc.innerHTML = '<button class="kd-btn-min" title="Minimize"><svg viewBox="0 0 12 12"><line x1="2" y1="6" x2="10" y2="6"/></svg></button>'
      + '<button class="kd-btn-max" title="Maximize"><svg viewBox="0 0 12 12"><rect x="2" y="2" width="8" height="8" rx="0.5"/></svg></button>'
      + '<button class="kd-close" title="Close"><svg viewBox="0 0 12 12"><line x1="2" y1="2" x2="10" y2="10"/><line x1="10" y1="2" x2="2" y2="10"/></svg></button>';
    var W = T && T.window ? T.window.getCurrentWindow() : null;
    wc.querySelector('.kd-btn-min').addEventListener('click', function() {{ if (W) W.minimize(); }});
    wc.querySelector('.kd-btn-max').addEventListener('click', function() {{
      if (!W) return;
      W.isMaximized().then(function(m) {{ if (m) W.unmaximize(); else W.maximize(); }});
    }});
    wc.querySelector('.kd-close').addEventListener('click', function() {{ if (W) W.close(); else if (invoke) invoke('quit_app'); }});
    return wc;
  }}

  /* inject our menus/drag INTO the PF masthead, keeping all React elements in place */
  function mergeMasthead(mh) {{
    mh.classList.add('kd-merged');
    document.body.classList.add('kd-ready');

    var toggle = mh.querySelector('.pf-v6-c-masthead__toggle');
    var content = mh.querySelector('.pf-v6-c-masthead__content');

    var menus = buildMenus();
    if (toggle) {{
      toggle.after(menus);
    }} else {{
      mh.prepend(menus);
    }}

    var drag = buildDrag();
    if (content) {{
      content.before(drag);
    }} else {{
      mh.appendChild(drag);
    }}


    mh.appendChild(buildWinControls());

    dropdowns.forEach(function(d) {{ document.body.appendChild(d); }});

    /* remove fallback bar if present */
    var fb = document.getElementById('kd-bar');
    if (fb) fb.remove();
  }}

  /* fallback bar for non-koku-ui pages (Settings, About) */
  function createFallbackBar() {{
    document.body.classList.add('kd-ready');
    var bar = document.createElement('div');
    bar.id = 'kd-bar';
    bar.appendChild(buildMenus());
    bar.appendChild(buildDrag());
    bar.appendChild(buildWinControls());
    document.body.prepend(bar);
    dropdowns.forEach(function(d) {{ document.body.appendChild(d); }});
  }}

  var mh = document.querySelector('.pf-v6-c-masthead');
  if (mh) {{
    mergeMasthead(mh);
  }} else {{
    createFallbackBar();
    var observer = new MutationObserver(function(mutations, obs) {{
      var found = document.querySelector('.pf-v6-c-masthead');
      if (found) {{ obs.disconnect(); mergeMasthead(found); }}
    }});
    observer.observe(document.body, {{ childList: true, subtree: true }});
    setTimeout(function() {{ observer.disconnect(); }}, 10000);
  }}

  document.addEventListener('click', closeAll);

  /* inject "My User Access" into the PF user profile dropdown */
  new MutationObserver(function() {{
    var menus = document.querySelectorAll('.pf-v6-c-menu__list');
    menus.forEach(function(list) {{
      if (list.querySelector('.kd-mua-item')) return;
      var logout = null;
      list.querySelectorAll('.pf-v6-c-menu__item').forEach(function(item) {{
        if (item.textContent.trim().toLowerCase() === 'logout') logout = item.closest('.pf-v6-c-menu__list-item');
      }});
      if (!logout) return;
      var li = document.createElement('li');
      li.className = 'pf-v6-c-menu__list-item kd-mua-item';
      li.setAttribute('role', 'none');
      var btn = document.createElement('button');
      btn.className = 'pf-v6-c-menu__item';
      btn.setAttribute('role', 'menuitem');
      btn.innerHTML = '<span class="pf-v6-c-menu__item-main"><span class="pf-v6-c-menu__item-text">My User Access</span></span><span style="margin-left:auto;font-size:11px;color:var(--kd-bar-fg2,#6a6e73)">Ctrl+M</span>';
      btn.addEventListener('click', function() {{ kdNavigate('/iam/my-user-access'); }});
      li.appendChild(btn);
      logout.before(li);
    }});
  }}).observe(document.body, {{ childList: true, subtree: true }});

  /* keyboard shortcuts */
  document.addEventListener('keydown', function(e) {{
    var ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && !e.shiftKey) {{
      switch(e.key.toLowerCase()) {{
        case 'h': e.preventDefault(); kdNavigate('/openshift/cost-management/'); break;
        case 'o': e.preventDefault(); kdNavigate('/openshift/cost-management/ocp'); break;
        case 'w': e.preventDefault(); kdNavigate('/openshift/cost-management/aws'); break;
        case 'u': e.preventDefault(); kdNavigate('/openshift/cost-management/azure'); break;
        case 'g': e.preventDefault(); kdNavigate('/openshift/cost-management/gcp'); break;
        case 'e': e.preventDefault(); kdNavigate('/openshift/cost-management/explorer'); break;
        case 's': e.preventDefault(); kdNavigate('/openshift/cost-management/settings'); break;
        case 'm': e.preventDefault(); kdNavigate('/iam/my-user-access'); break;
        case 't': e.preventDefault(); doAction('theme'); break;
        case 'p': e.preventDefault(); doAction('print'); break;
        case 'q': e.preventDefault(); doAction('quit'); break;
      }}
    }}
    if (ctrl && e.shiftKey) {{
      switch(e.key.toLowerCase()) {{
        case 'w': e.preventDefault(); kdNavigate('/iam/user-access/overview'); break;
        case 'u': e.preventDefault(); kdNavigate('/iam/user-access/users'); break;
        case 'g': e.preventDefault(); kdNavigate('/iam/user-access/groups'); break;
        case 'r': e.preventDefault(); kdNavigate('/iam/user-access/roles'); break;
      }}
    }}
    if (e.key === 'Escape') closeAll();
  }});

  /* intercept blob downloads (js-file-download creates <a download> with blob: URLs)
     WebKitGTK does not trigger Tauri's on_download for JS-initiated blob downloads */
  document.addEventListener('click', function(e) {{
    var a = e.target.closest ? e.target.closest('a[download]') : null;
    if (!a) return;
    var href = a.href || '';
    if (!href.startsWith('blob:')) return;
    e.preventDefault();
    e.stopPropagation();
    var filename = a.getAttribute('download') || 'download.csv';
    fetch(href).then(function(r) {{ return r.blob(); }}).then(function(blob) {{
      var reader = new FileReader();
      reader.onload = function() {{
        var base64 = reader.result.split(',')[1] || '';
        if (invoke) {{
          invoke('save_blob_download', {{ filename: filename, dataBase64: base64 }}).then(function(path) {{
            if (path) console.log('Saved to', path);
          }}).catch(function(err) {{
            console.error('Save failed:', err);
          }});
        }}
      }};
      reader.readAsDataURL(blob);
    }});
  }}, true);
}});
</script>
"##
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
  <title>Red Hat Lightspeed Cost Management Desktop</title>
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
