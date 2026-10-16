//! RV Mail de escritorio y móvil (Tauri 2).
//!
//! La ventana muestra la web de TU servidor RV Mail (paridad total con la PWA)
//! y la app añade lo nativo: bandeja del sistema, avisos, contador de no leídos,
//! enlaces `mailto:` y el inicio de sesión con Google/Microsoft/Yahoo en el
//! navegador del sistema (Google lo bloquea dentro de webviews).
//!
//! La primera vez se abre una pantalla local (`ui/index.html`) para escribir el
//! dominio del servidor; se valida con `/.well-known/rvmail` y se recuerda.

use base64::Engine;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_store::StoreExt;

const STORE: &str = "settings.json";

#[derive(Default)]
struct AppState {
    /// Origen del servidor elegido (https://correo.ejemplo.com/)
    server: Arc<Mutex<Option<Url>>>,
    /// Verificador PKCE del inicio de sesión en curso con el navegador del sistema
    pending_sso: Mutex<Option<String>>,
}

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

fn show(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn server(app: &AppHandle) -> Option<Url> {
    app.state::<AppState>().server.lock().ok()?.clone()
}

fn navigate(app: &AppHandle, url: Url) {
    if let Some(w) = main_window(app) {
        let _ = w.navigate(url);
    }
    show(app);
}

fn local_ui(app: &AppHandle, error: Option<&str>) {
    let mut url = Url::parse("tauri://localhost/index.html").expect("url local");
    if cfg!(any(windows, target_os = "android")) {
        url = Url::parse("http://tauri.localhost/index.html").expect("url local");
    }
    if let Some(e) = error {
        url.query_pairs_mut().append_pair("error", e);
    }
    navigate(app, url);
}

/// `correo.ejemplo.com` → `https://correo.ejemplo.com/`. HTTP solo en local (desarrollo).
fn normalize_server(input: &str) -> Result<Url, String> {
    let raw = input.trim().trim_end_matches('/');
    if raw.is_empty() {
        return Err("Escribe la dirección de tu servidor RV Mail".into());
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let mut url = Url::parse(&with_scheme).map_err(|_| "Dirección no válida".to_string())?;
    let host = url.host_str().unwrap_or_default().to_string();
    let local = host == "localhost" || host == "127.0.0.1";
    match url.scheme() {
        "https" => {}
        "http" if local => {}
        _ => return Err("El servidor debe usar https://".into()),
    }
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// ---------------------------------------------------------------- comandos

#[tauri::command]
fn open_server(app: AppHandle, url: String) -> Result<(), String> {
    let url = normalize_server(&url)?;
    let store = app.store(STORE).map_err(|e| e.to_string())?;
    store.set("server", json!(url.as_str()));
    store.save().map_err(|e| e.to_string())?;
    *app.state::<AppState>()
        .server
        .lock()
        .map_err(|e| e.to_string())? = Some(url.clone());
    navigate(&app, url);
    Ok(())
}

#[tauri::command]
fn forget_server(app: AppHandle) -> Result<(), String> {
    if let Ok(store) = app.store(STORE) {
        store.delete("server");
        let _ = store.save();
    }
    *app.state::<AppState>()
        .server
        .lock()
        .map_err(|e| e.to_string())? = None;
    local_ui(&app, None);
    Ok(())
}

#[tauri::command]
fn notify(app: AppHandle, title: String, body: String) -> Result<(), String> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_unread(app: AppHandle, count: u32) {
    let tip = if count == 0 {
        "RV Mail".to_string()
    } else {
        format!("RV Mail — {count} sin leer")
    };
    #[cfg(desktop)]
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(&tip));
    }
    if let Some(w) = main_window(&app) {
        let _ = w.set_title(&tip);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let _ = w.set_badge_count(if count == 0 {
            None
        } else {
            Some(i64::from(count))
        });
    }
}

/// Abre el inicio de sesión del proveedor en el navegador del sistema; vuelve por
/// `rvmail://auth?code=…` y se canjea con PKCE en `/api/auth/token/web`.
#[tauri::command]
fn sso_login(app: AppHandle, provider: String) -> Result<(), String> {
    if !["google", "microsoft", "yahoo"].contains(&provider.as_str()) {
        return Err("Proveedor no válido".into());
    }
    let base = server(&app).ok_or("Primero elige tu servidor")?;
    let mut raw = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut raw);
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let verifier = b64.encode(raw);
    let challenge = b64.encode(Sha256::digest(verifier.as_bytes()));
    *app.state::<AppState>()
        .pending_sso
        .lock()
        .map_err(|e| e.to_string())? = Some(verifier);
    let url = base
        .join(&format!(
            "/api/auth/sso/{provider}/start?app_redirect={}&app_challenge={challenge}",
            pct("rvmail://auth")
        ))
        .map_err(|e| e.to_string())?;
    app.opener()
        .open_url(url.as_str(), None::<&str>)
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- enlaces entrantes

fn handle_link(app: &AppHandle, url: &Url) {
    match url.scheme() {
        "mailto" => {
            if let Some(base) = server(app) {
                if let Ok(to) = base.join(&format!("/?mailto={}", pct(url.as_str()))) {
                    navigate(app, to);
                }
            }
        }
        "rvmail" if url.host_str() == Some("auth") => {
            let code = url
                .query_pairs()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.to_string());
            let verifier = app
                .state::<AppState>()
                .pending_sso
                .lock()
                .ok()
                .and_then(|mut p| p.take());
            match (server(app), code, verifier) {
                (Some(base), Some(code), Some(v)) => {
                    if let Ok(to) = base.join(&format!(
                        "/api/auth/token/web?code={}&code_verifier={}",
                        pct(&code),
                        pct(&v)
                    )) {
                        navigate(app, to);
                    }
                }
                _ => show(app),
            }
        }
        _ => show(app),
    }
}

// ---------------------------------------------------------------- arranque

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();
    #[cfg(desktop)]
    {
        // Una sola instancia: los enlaces (mailto:, rvmail://) llegan a la ventana abierta
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show(app)
        }));
    }
    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            open_server,
            forget_server,
            notify,
            set_unread,
            sso_login
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let saved = app
                .store(STORE)
                .ok()
                .and_then(|s| s.get("server"))
                .and_then(|v| v.as_str().map(str::to_string))
                .and_then(|s| normalize_server(&s).ok());
            let shared = app.state::<AppState>().server.clone();
            *shared.lock().expect("estado") = saved.clone();

            // Solo se navega dentro del servidor elegido; lo demás va al navegador del sistema
            let nav_handle = handle.clone();
            let nav_server = shared.clone();
            let start = match &saved {
                Some(u) => WebviewUrl::External(u.clone()),
                None => WebviewUrl::App("index.html".into()),
            };
            WebviewWindowBuilder::new(app, "main", start)
                .title("RV Mail")
                .inner_size(1200.0, 800.0)
                .min_inner_size(360.0, 520.0)
                .on_navigation(move |url| {
                    let local = matches!(url.scheme(), "tauri" | "about" | "blob" | "data")
                        || url.host_str() == Some("tauri.localhost");
                    let ours = nav_server
                        .lock()
                        .ok()
                        .and_then(|s| s.clone())
                        .is_some_and(|s| same_origin(&s, url));
                    if local || ours {
                        return true;
                    }
                    if matches!(url.scheme(), "http" | "https" | "mailto") {
                        let _ = nav_handle.opener().open_url(url.as_str(), None::<&str>);
                    }
                    false
                })
                .build()?;

            #[cfg(desktop)]
            tray(app)?;

            #[cfg(any(windows, target_os = "linux"))]
            {
                // En desarrollo (sin instalador) registra rvmail:// y mailto: al arrancar
                let _ = app.deep_link().register_all();
            }
            let link_handle = handle.clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    handle_link(&link_handle, &url);
                }
            });
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                for url in urls {
                    handle_link(&handle, &url);
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Cerrar la ventana la deja en la bandeja (sigue avisando del correo nuevo)
            #[cfg(desktop)]
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
            #[cfg(mobile)]
            let _ = (window, event);
        })
        .run(tauri::generate_context!())
        .expect("no se pudo iniciar RV Mail");
}

#[cfg(desktop)]
fn tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let open = MenuItem::with_id(app, "open", "Abrir RV Mail", true, None::<&str>)?;
    let compose = MenuItem::with_id(app, "compose", "Redactar correo", true, None::<&str>)?;
    let server = MenuItem::with_id(app, "server", "Cambiar de servidor…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Salir", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &compose, &sep, &server, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("RV Mail")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show(app),
            "compose" => {
                if let Ok(u) = Url::parse("mailto:") {
                    handle_link(app, &u);
                }
            }
            "server" => {
                let _ = forget_server(app.clone());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_server() {
        assert_eq!(
            normalize_server("correo.x.com").unwrap().as_str(),
            "https://correo.x.com/"
        );
        assert_eq!(
            normalize_server("https://correo.x.com/app?x=1")
                .unwrap()
                .as_str(),
            "https://correo.x.com/"
        );
        assert_eq!(
            normalize_server("http://localhost:5173").unwrap().as_str(),
            "http://localhost:5173/"
        );
        assert!(normalize_server("http://correo.x.com").is_err());
        assert!(normalize_server("").is_err());
    }

    #[test]
    fn origins() {
        let a = Url::parse("https://correo.x.com/").unwrap();
        assert!(same_origin(
            &a,
            &Url::parse("https://correo.x.com/api/x").unwrap()
        ));
        assert!(!same_origin(
            &a,
            &Url::parse("https://correo.x.com.evil.io/").unwrap()
        ));
    }
}
