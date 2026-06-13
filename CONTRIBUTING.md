# Contributing to Cost Management Desktop

## Project Structure

```
koku-desktop/
├── src-tauri/                    # Rust backend (Tauri v2 + axum proxy)
│   ├── Cargo.toml                # Rust dependencies
│   ├── build.rs                  # Compile-time env: GIT_HASH, BUILD_DATE
│   ├── tauri.conf.json           # Window config, bundle settings, plugins
│   ├── capabilities/default.json # IPC permissions and remote URL access
│   ├── permissions/default.toml  # Custom IPC command permission definitions
│   └── src/
│       ├── main.rs               # Entry point: proxy startup, Tauri builder, IPC commands
│       ├── config.rs             # AppConfig struct, load/save, enums (AuthMode, Theme)
│       ├── proxy.rs              # axum HTTP server: static files, API proxy, HTML injection
│       └── auth/
│           ├── mod.rs            # AuthProvider trait + UserInfo struct
│           ├── dev.rs            # DevAuthProvider: X-Rh-Identity from config
│           └── oidc.rs           # OidcAuthProvider: Keycloak password-grant
├── settings/index.html           # Settings/first-launch wizard (plain HTML/CSS/JS)
├── about/index.html              # About window (plain HTML/CSS/JS)
├── splash/                       # Splash screen (index.html + logo.png)
├── fonts/                        # Red Hat variable fonts (.woff2, SIL OFL license)
├── scripts/build-ui.sh           # Builds koku-ui and copies dist into ui/
├── ui/                           # Pre-built koku-ui static assets (gitignored)
├── docs/architecture.md          # Full design document
├── README.md                     # User and developer documentation
├── CONTRIBUTING.md               # This file
└── AGENTS.md                     # AI agent guidance
```

## How It Works

### Startup Flow

1. `main.rs`: Load config from disk (or detect first launch)
2. `main.rs`: Create auth provider based on config (`dev` or `oidc`)
3. `proxy.rs`: Start axum HTTP server on `127.0.0.1:0` (random port)
4. `main.rs`: Create Tauri webview pointing at `http://127.0.0.1:<port>/`
   - First launch (no config): navigates to `/_settings/`
   - Configured: navigates to `/` (koku-ui SPA)
5. `main.rs`: Set up system tray, download handler, close-to-tray behavior

### Proxy Server (proxy.rs)

The axum server has three route groups:

| Route | Handler | Purpose |
|-------|---------|---------|
| `GET /api/me` | `api_me()` | Returns `{username, email}` from auth provider |
| `/api/*` | `proxy_api()` | Reverse proxy to Koku backend with auth headers |
| `/_settings/`, `/_about/`, `/_splash/` | `ServeDir` | Custom pages (not from koku-ui) |
| Everything else | `serve_ui()` | Static files from `ui/` with SPA fallback to `index.html` |

#### HTML Injection

Every HTML page served by the proxy gets a `<script>` and `<style>` block injected before `</head>` (see `build_head_injection()` in `proxy.rs`). This injection:

- Sets the dark/light theme class on `<html>`
- Adds CSS to restyle the PatternFly masthead as a compact unified titlebar
- Adds JavaScript that injects File/Navigate/View/Help menus into the masthead
- Provides keyboard shortcuts for navigation and actions
- Creates a fallback titlebar for non-koku-ui pages (Settings, About)

This is the most complex part of the codebase. All UI customization happens here — no koku-ui source code is modified.

### Authentication (auth/)

The `AuthProvider` trait defines two methods:

```rust
trait AuthProvider: Send + Sync {
    fn request_headers(&self) -> HeaderMap;  // injected on every proxied /api/ request
    fn user_info(&self) -> UserInfo;         // returned by GET /api/me
}
```

| Implementation | Auth mode | Header | Backend requirement |
|----------------|-----------|--------|---------------------|
| `DevAuthProvider` | `dev` | `X-Rh-Identity: <base64>` | `DEVELOPMENT=True` |
| `OidcAuthProvider` | `oidc` | `Authorization: Bearer <jwt>` | Keycloak + gateway |

### IPC Commands (main.rs)

Tauri IPC commands are Rust functions callable from JavaScript via `window.__TAURI__.core.invoke()`:

| Command | Purpose |
|---------|---------|
| `get_config` | Load current config |
| `save_config` | Persist config, restart proxy |
| `test_connection` | Hit Koku status endpoint |
| `get_about_info` | Return app version, OS info, build details |
| `get_server_status` | Return Koku server status JSON |
| `quit_app` | Exit the application |

Every IPC command must be:
1. Defined as a `#[tauri::command]` function in `main.rs`
2. Registered in the Tauri builder's `.invoke_handler(tauri::generate_handler![...])` call
3. Permitted in `src-tauri/permissions/default.toml`
4. Referenced in `src-tauri/capabilities/default.json`

Missing any of these steps results in `"command not allowed"` errors at runtime.

### Custom Pages

Settings (`settings/index.html`) and About (`about/index.html`) are self-contained HTML files with inline CSS and JavaScript. They use no build tools and no frameworks — just plain DOM manipulation and Tauri IPC calls. They are styled to match PatternFly's look using the Red Hat fonts from `fonts/`.

These pages are served by the proxy at `/_settings/` and `/_about/`, separate from the koku-ui routes.

---

## Development Workflow

### Prerequisites

See [README.md](README.md#prerequisites) for system dependencies.

### Build the UI assets

```bash
./scripts/build-ui.sh
```

This must be done at least once before running or building the app.

### Development mode

```bash
cargo tauri dev
```

The Rust backend recompiles on source changes. The webview reloads automatically. Note that changes to the injected HTML/CSS/JS in `proxy.rs` require a full page reload in the webview (the injection happens at serve time, not at runtime).

### Production build

```bash
cargo tauri build
```

Produces the binary and distribution packages (RPM, DEB, AppImage). See [README.md](README.md#building-for-production) for output locations.

### Running tests

```bash
cd src-tauri
cargo test
```

> Note: The PoC has minimal test coverage. This is a target for improvement.

---

## Code Conventions

### Rust

- **Edition:** 2024
- **Error handling:** Use `anyhow::Result` for fallible functions. Use `log::error!()` for non-fatal errors in request handlers and return appropriate HTTP status codes.
- **Async:** The proxy server and Tauri runtime use `tokio`. IPC commands can be `async` (preferred) or synchronous.
- **Logging:** Use the `log` crate macros (`log::info!`, `log::error!`, etc.). Logs go to rotating files via `tauri-plugin-log`.
- **TLS:** The HTTP client uses `danger_accept_invalid_certs(true)` because lab/on-prem environments commonly use self-signed certificates.

### JavaScript (injected and custom pages)

- **No build tools.** Settings and About pages are plain HTML/CSS/JS.
- **Tauri IPC:** Use `window.__TAURI__.core.invoke('command_name', { args })`.
- **Browser compatibility:** The code runs in WebKitGTK (Linux), WKWebView (macOS), or WebView2 (Windows). Avoid APIs not supported by WebKitGTK.
- **Escaping in Rust format strings:** When writing JavaScript/CSS inside Rust `format!()` macros, all `{` and `}` must be doubled (`{{` / `}}`). This is the most common source of compilation errors when editing `proxy.rs`.

### Adding a new IPC command

1. Add the `#[tauri::command]` function in `main.rs`
2. Add it to `tauri::generate_handler![...]` in the builder
3. Add `"allow-<command-name>"` to `src-tauri/permissions/default.toml`
4. Reference the permission set in `src-tauri/capabilities/default.json`

### Adding a new custom page

1. Create a directory (e.g., `mypage/`) with `index.html`
2. Add a `nest_service` route in `build_router()` in `proxy.rs`:
   ```rust
   .nest_service("/_mypage", ServeDir::new(state.base_path.join("mypage")))
   ```
3. Add a menu entry or navigation link to the page

---

## Key Design Decisions

### Why a local proxy instead of direct API calls?

koku-ui uses relative URLs (`/api/cost-management/v1/...`) hardcoded in axios. Rather than forking and modifying koku-ui, we run a localhost proxy that serves the static assets and forwards API requests. This means **zero koku-ui modifications**.

### Why inject HTML/CSS/JS instead of modifying koku-ui?

The unified titlebar, theme switching, and keyboard shortcuts are all implemented via HTML injection in `proxy.rs`. This keeps koku-ui unmodified and allows the desktop client to work with any koku-ui build version.

### Why keep PatternFly elements in the React tree?

Early iterations moved DOM nodes out of the React tree (e.g., reparenting the user profile toolbar). This broke React's event delegation — clicks on moved nodes never reached React's event handlers. The current approach injects our elements **into** the PatternFly masthead, keeping all React-managed DOM in place.

### Why `decorations: true`?

GNOME/Wayland ignores `decorations: false` and draws server-side decorations regardless. This resulted in duplicate titlebars. Setting `decorations: true` accepts the native titlebar and our injected titlebar handles menus/navigation only, avoiding visual duplication.

---

## Versioning

The app version is defined in two places and must be kept in sync:

- `src-tauri/Cargo.toml` (`version = "0.1.0"`)
- `src-tauri/tauri.conf.json` (`"version": "0.1.0"`)

Build metadata (git hash, build date) is injected at compile time by `src-tauri/build.rs` via `CARGO_PKG_VERSION`, `GIT_HASH`, and `BUILD_DATE` environment variables.
