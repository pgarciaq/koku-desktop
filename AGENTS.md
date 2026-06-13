# Agent Guide for koku-desktop

This document is for AI coding agents (Cursor, Copilot, Claude, etc.) working on the koku-desktop codebase. It describes the patterns, pitfalls, and context that save time and prevent common mistakes.

## What This Project Is

koku-desktop is a Tauri v2 desktop application that wraps the koku-ui on-prem frontend and proxies API requests to a remote Koku backend. It is **not** a web application — it is a native desktop app that uses a system webview.

The critical design constraint: **koku-ui source code is never modified**. All customization (theming, menus, titlebar, shortcuts) happens via HTML/CSS/JS injection in the proxy server at serve time.

## File Map

| File | What it does | When to edit |
|------|-------------|--------------|
| `src-tauri/src/main.rs` | Tauri entry point, IPC commands, tray, downloads | Adding IPC commands, changing app lifecycle |
| `src-tauri/src/proxy.rs` | axum HTTP server, static files, API proxy, **HTML injection** | Changing UI injection (menus, titlebar, theme), adding proxy routes |
| `src-tauri/src/config.rs` | `AppConfig` struct, serialization, defaults | Adding config fields |
| `src-tauri/src/auth/mod.rs` | `AuthProvider` trait | Changing the auth interface |
| `src-tauri/src/auth/dev.rs` | Dev-mode auth (X-Rh-Identity) | Changing dev auth behavior |
| `src-tauri/src/auth/oidc.rs` | Keycloak OIDC auth | Changing OIDC flow |
| `src-tauri/tauri.conf.json` | Window config, bundle settings | Changing window behavior, packaging |
| `src-tauri/capabilities/default.json` | IPC permissions, remote URL access | Adding new IPC commands |
| `src-tauri/permissions/default.toml` | Custom IPC command permission defs | Adding new IPC commands |
| `settings/index.html` | Settings page (plain HTML) | Changing settings UI |
| `about/index.html` | About window (plain HTML) | Changing about window |
| `scripts/build-ui.sh` | Builds koku-ui, copies dist to `ui/` | Changing the UI build process |

## Critical Patterns

### 1. IPC commands require FOUR changes

Adding a new Tauri IPC command requires changes in all four places or it will fail at runtime with `"command not allowed"`:

1. **`main.rs`**: Define the `#[tauri::command]` function
2. **`main.rs`**: Register it in `tauri::generate_handler![...]`
3. **`permissions/default.toml`**: Add `"allow-<name>"` entry
4. **`capabilities/default.json`**: Reference the permission set

### 2. The HTML injection in proxy.rs is the hardest code to edit

The `build_head_injection()` function in `proxy.rs` generates a large HTML/CSS/JS string that gets injected into every HTML page. Key hazards:

- **Rust format string escaping**: Every `{` and `}` in CSS/JS must be doubled (`{{` / `}}`). A single unescaped brace causes a Rust compile error.
- **The injection runs BEFORE React renders**: The injected script uses `DOMContentLoaded` and `MutationObserver` to wait for the PatternFly masthead to appear, then injects menu elements into it.
- **DOM nodes must stay in the React tree**: Never move (reparent) React-managed DOM nodes out of their original container. React 18's event delegation is attached to the root container — moved nodes won't receive React synthetic events. Instead, inject new elements INTO existing React containers.
- **The masthead approach**: Our menus and drag region are injected INTO the `.pf-v6-c-masthead` element. The masthead gets the `.kd-merged` class which restyles it as a compact fixed titlebar. The brand/logo is hidden. The PF hamburger and user profile stay in place and keep working.
- **Fallback bar**: For non-koku-ui pages (Settings, About), a standalone `#kd-bar` div is created instead.

### 3. Theme handling

The theme is controlled by the `pf-v6-theme-dark` CSS class on `<html>`. The proxy injection sets this based on the config (`light`/`dark`/`system`). All custom CSS must use CSS variables (defined in `:root` and `.pf-v6-theme-dark` selectors) to adapt to both themes. Never hardcode colors.

### 4. Authentication flow

- **Dev mode**: `X-Rh-Identity` header with base64-encoded JSON identity blob. The Koku backend must run with `DEVELOPMENT=True`.
- **OIDC mode**: Keycloak password-grant flow. Tokens are cached in memory and refreshed automatically. Uses `reqwest::blocking` on a spawned thread (not the async runtime).
- The `AuthProvider` trait is behind `Arc<RwLock<>>` — it can be swapped at runtime when the user saves new config.

### 5. TLS and self-signed certificates

All `reqwest::Client` instances use `danger_accept_invalid_certs(true)` because lab/on-prem environments commonly use self-signed certificates. This applies in:
- `proxy.rs` (API proxy client)
- `main.rs` (`test_connection` and `get_server_status` commands)
- `auth/oidc.rs` (token endpoint calls)

## Common Tasks

### Adding a menu item

Edit `build_head_injection()` in `proxy.rs`. Find the `menuDefs` JavaScript object and add your item to the appropriate menu. If it navigates to a URL, use `{ label: 'Name', nav: '/path' }`. If it performs an action, use `{ label: 'Name', action: 'actionName' }` and add the action handler to the `doAction()` function.

### Adding a config field

1. Add the field to `AppConfig` (or a nested struct) in `config.rs` with `#[serde(default)]` for backward compatibility
2. If it needs a default value, implement it
3. Update `settings/index.html` to show a UI control for it
4. Use the field in `main.rs` or `proxy.rs` as needed

### Adding a custom page

1. Create `pagename/index.html` with inline CSS/JS
2. Add `.nest_service("/_pagename", ServeDir::new(state.base_path.join("pagename")))` to `build_router()` in `proxy.rs`
3. Optionally add a menu entry in `build_head_injection()`

### Changing the proxy behavior

The proxy is an axum router in `proxy.rs`. Routes are defined in `build_router()`. API proxying is in `proxy_api()`. Static file serving is in `serve_ui()` with SPA fallback (non-file paths return `index.html`).

## Build and Test

```bash
# Development (hot-reload Rust, opens window)
cargo tauri dev

# Production build (binary + RPM + DEB)
cargo tauri build

# Rust tests only
cd src-tauri && cargo test

# Build UI assets (requires koku-ui repo)
./scripts/build-ui.sh
```

## Things to Avoid

1. **Never modify koku-ui source code.** All customization is via HTML injection.
2. **Never move React DOM nodes out of the React tree.** Inject into, don't reparent out of.
3. **Never hardcode colors in the injected CSS.** Use the `--kd-*` CSS variables.
4. **Never forget the IPC permission chain** (function → handler → toml → json).
5. **Never use single `{` or `}` in the `format!()` macro** inside `build_head_injection()`. They must be `{{` and `}}`.
6. **Never store secrets in config.json.** Use the OS keychain via the `keyring` crate.
7. **Never assume `decorations: false` works on Linux.** GNOME/Wayland ignores it.

## Upstream Context

This project is part of the Koku ecosystem:

- **koku** — Django backend (API, data pipeline, Celery workers)
- **koku-ui** — React frontend (the SPA we bundle)
- **koku-metrics-operator** — Go OpenShift operator

See the workspace rule at `.cursor/rules/koku-ecosystem.mdc` for full ecosystem documentation.
