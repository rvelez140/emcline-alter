//! Pantalla principal: carpetas (todas las cuentas y por cuenta), lista de
//! mensajes con búsqueda y paginación, y lectura segura.

use crate::api::{self, View};
use crate::sanitize;
use chrono::{DateTime, Local, Utc};
use leptos::prelude::*;
use leptos::task::spawn_local;
use rvmail_types::{AccountDto, FolderDto, MessageFull, MessageSummary, UserDto, SPECIALS};
use uuid::Uuid;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// En pantallas pequeñas se ve un panel a la vez.
#[derive(Clone, Copy, PartialEq)]
enum Pane {
    Nav,
    List,
    Read,
}

/// Nombre en español de las carpetas especiales (el servidor usa INBOX, Sent Items…).
fn folder_label(f: &FolderDto) -> String {
    match f.special_use.as_deref() {
        Some("\\Inbox") => "Bandeja de entrada".into(),
        Some("\\Sent") => "Enviados".into(),
        Some("\\Drafts") => "Borradores".into(),
        Some("\\Trash") => "Papelera".into(),
        Some("\\Junk") => "Spam".into(),
        Some("\\Archive") => "Archivo".into(),
        _ => f.name.clone(),
    }
}

fn short_date(d: &DateTime<Utc>) -> String {
    let local = d.with_timezone(&Local);
    if local.date_naive() == Local::now().date_naive() {
        local.format("%H:%M").to_string()
    } else {
        local.format("%d/%m/%y").to_string()
    }
}

fn prefers_dark() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
        .is_some_and(|m| m.matches())
}

/// Tiempo real: cualquier evento del servidor refresca listas y contadores.
fn live_events(refresh: RwSignal<u32>) {
    let Ok(es) = web_sys::EventSource::new("/api/events") else {
        return;
    };
    let cb = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_ev: web_sys::MessageEvent| {
        refresh.update(|n| *n += 1);
    });
    for kind in [
        "message.new",
        "message.updated",
        "folder.updated",
        "account.status",
    ] {
        let _ = es.add_event_listener_with_callback(kind, cb.as_ref().unchecked_ref());
    }
    cb.forget();
    let es = send_wrapper::SendWrapper::new(es);
    on_cleanup(move || es.close());
}

#[component]
pub fn MailApp(user: UserDto, #[prop(into)] on_logout: Callback<()>) -> impl IntoView {
    let view_sig = RwSignal::new(View::Unified("inbox"));
    let search = RwSignal::new(String::new());
    let refresh = RwSignal::new(0u32);
    let selected = RwSignal::new(None::<Uuid>);
    let pane = RwSignal::new(Pane::List);

    let accounts = LocalResource::new(move || {
        refresh.track();
        async { api::accounts().await.unwrap_or_default() }
    });
    let folders = LocalResource::new(move || {
        refresh.track();
        async { api::folders().await.unwrap_or_default() }
    });

    // Lista con paginación por cursor
    let items = RwSignal::new(Vec::<MessageSummary>::new());
    let cursor = RwSignal::new(None::<String>);
    let loading = RwSignal::new(false);
    let list_error = RwSignal::new(None::<String>);
    let load = move |append: bool| {
        let v = view_sig.get_untracked();
        let q = search.get_untracked();
        let c = if append { cursor.get_untracked() } else { None };
        loading.set(true);
        spawn_local(async move {
            match api::messages(&v, &q, c.as_deref()).await {
                Ok(page) => {
                    if append {
                        items.update(|i| i.extend(page.items));
                    } else {
                        items.set(page.items);
                    }
                    cursor.set(page.next_cursor);
                    list_error.set(None);
                }
                Err(e) => list_error.set(Some(e.message)),
            }
            loading.set(false);
        });
    };
    Effect::new(move |_| {
        view_sig.track();
        refresh.track();
        load(false);
    });
    live_events(refresh);

    let open_view = move |v: View| {
        view_sig.set(v);
        selected.set(None);
        pane.set(Pane::List);
    };
    let logout = move |_| {
        spawn_local(async move {
            api::logout().await;
            on_logout.run(());
        });
    };

    let sidebar = move || {
        let all: Vec<FolderDto> = folders.get().unwrap_or_default();
        let accs: Vec<AccountDto> = accounts.get().unwrap_or_default();
        let inbox_unread: i32 = all
            .iter()
            .filter(|f| f.special_use.as_deref() == Some("\\Inbox"))
            .map(|f| f.unread_count)
            .sum();
        let unified = SPECIALS
            .iter()
            .map(|(key, label)| {
                let key: &'static str = key;
                let active = move || view_sig.get() == View::Unified(key);
                let count = if key == "inbox" { inbox_unread } else { 0 };
                view! {
                    <button class="nav-item" class:active=active on:click=move |_| open_view(View::Unified(key))>
                        <span>{*label}</span>
                        {(count > 0).then(|| view! { <span class="badge">{count}</span> })}
                    </button>
                }
            })
            .collect_view();
        let per_account = accs
            .into_iter()
            .map(|a| {
                let mut fs: Vec<FolderDto> = all.iter().filter(|f| f.account_id == a.id).cloned().collect();
                fs.sort_by_key(|f| (f.special_use.as_deref() != Some("\\Inbox"), f.name.to_lowercase()));
                let warn = matches!(a.status.as_str(), "error" | "auth_error");
                let title = a.last_error.clone().unwrap_or_default();
                view! {
                    <div class="account">
                        <p class="account-name" title=title>
                            <span class="dot" style=format!("background:{}", a.color)></span>
                            {a.display_name.clone()}
                            {warn.then(|| view! { <span class="warn" title="Revisa la conexión de esta cuenta">"⚠"</span> })}
                        </p>
                        {fs
                            .into_iter()
                            .map(|f| {
                                let label = folder_label(&f);
                                let (account, folder, name) = (f.account_id, f.id, label.clone());
                                let active = move || matches!(view_sig.get(), View::Folder { folder: x, .. } if x == folder);
                                view! {
                                    <button
                                        class="nav-item sub"
                                        class:active=active
                                        on:click=move |_| open_view(View::Folder { account, folder, name: name.clone() })
                                    >
                                        <span>{label}</span>
                                        {(f.unread_count > 0).then(|| view! { <span class="count">{f.unread_count}</span> })}
                                    </button>
                                }
                            })
                            .collect_view()}
                    </div>
                }
            })
            .collect_view();
        view! {
            <p class="section">"Todas las cuentas"</p>
            {unified}
            {per_account}
        }
    };

    let title = move || match view_sig.get() {
        View::Unified(k) => SPECIALS
            .iter()
            .find(|(s, _)| *s == k)
            .map(|(_, l)| l.to_string())
            .unwrap_or_default(),
        View::Folder { name, .. } => name,
    };
    let on_search = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        selected.set(None);
        pane.set(Pane::List);
        load(false);
    };
    let user_name = user.name.clone();
    let user_email = user.email.clone();

    view! {
        <div class="app" data-pane=move || match pane.get() { Pane::Nav => "nav", Pane::List => "list", Pane::Read => "read" }>
            <aside class="pane nav">
                <div class="nav-scroll">{sidebar}</div>
                <div class="me">
                    <div>
                        <p class="strong">{user_name}</p>
                        <p class="muted small">{user_email}</p>
                    </div>
                    <button class="ghost" title="Salir" on:click=logout>"Salir"</button>
                </div>
            </aside>
            <section class="pane list">
                <form class="toolbar" on:submit=on_search>
                    <button type="button" class="ghost only-mobile" on:click=move |_| pane.set(Pane::Nav)>"☰"</button>
                    <input type="search" placeholder="Buscar en el correo" bind:value=search />
                </form>
                <h2 class="list-title">{title}</h2>
                <p class="error">{move || list_error.get().unwrap_or_default()}</p>
                <div class="messages">
                    <For
                        each=move || items.get()
                        key=|m| (m.id, m.seen, m.flagged)
                        children=move |m: MessageSummary| {
                            let id = m.id;
                            let from = m.from_name.clone().filter(|s| !s.is_empty()).or(m.from_address.clone()).unwrap_or_else(|| "(sin remitente)".into());
                            view! {
                                <button
                                    class="row"
                                    class:unread=!m.seen
                                    class:active=move || selected.get() == Some(id)
                                    on:click=move |_| {
                                        selected.set(Some(id));
                                        pane.set(Pane::Read);
                                    }
                                >
                                    <span class="bar" style=format!("background:{}", m.account_color)></span>
                                    <span class="row-main">
                                        <span class="row-top">
                                            <span class="from">{from}</span>
                                            {m.flagged.then(|| view! { <span class="star">"★"</span> })}
                                            <span class="date">{short_date(&m.date)}</span>
                                        </span>
                                        <span class="subject">{if m.subject.is_empty() { "(sin asunto)".to_string() } else { m.subject.clone() }}</span>
                                        <span class="snippet">{m.snippet.clone()}</span>
                                    </span>
                                </button>
                            }
                        }
                    />
                    <Show when=move || !loading.get() && items.with(Vec::is_empty) && list_error.get().is_none()>
                        <p class="center muted pad">"No hay mensajes"</p>
                    </Show>
                    <Show when=move || cursor.get().is_some()>
                        <button class="ghost more" disabled=move || loading.get() on:click=move |_| load(true)>
                            {move || if loading.get() { "Cargando…" } else { "Cargar más" }}
                        </button>
                    </Show>
                </div>
            </section>
            <section class="pane read">
                {move || match selected.get() {
                    None => view! { <p class="center muted">"Selecciona un mensaje para leerlo"</p> }.into_any(),
                    Some(id) => view! { <Reader id=id items=items folders_refresh=refresh on_back=move |_| pane.set(Pane::List) /> }.into_any(),
                }}
            </section>
        </div>
    }
}

#[component]
fn Reader(
    id: Uuid,
    items: RwSignal<Vec<MessageSummary>>,
    folders_refresh: RwSignal<u32>,
    #[prop(into)] on_back: Callback<()>,
) -> impl IntoView {
    let msg = RwSignal::new(None::<Result<MessageFull, String>>);
    let images = RwSignal::new(false);
    spawn_local(async move {
        let res = api::message(id).await.map_err(|e| e.message);
        if let Ok(m) = &res {
            // Abrirlo lo marca como leído en el servidor
            if items.with_untracked(|v| v.iter().any(|x| x.id == m.id && !x.seen)) {
                items.update(|v| {
                    if let Some(x) = v.iter_mut().find(|x| x.id == m.id) {
                        x.seen = true;
                    }
                });
                folders_refresh.update(|n| *n += 1);
            }
        }
        msg.set(Some(res));
    });
    let toggle_star = move |_| {
        let Some(Ok(m)) = msg.get_untracked() else {
            return;
        };
        let flagged = !m.flagged;
        spawn_local(async move {
            if api::set_flags(m.id, None, Some(flagged)).await.is_ok() {
                msg.update(|x| {
                    if let Some(Ok(x)) = x {
                        x.flagged = flagged;
                    }
                });
                items.update(|v| {
                    if let Some(x) = v.iter_mut().find(|x| x.id == m.id) {
                        x.flagged = flagged;
                    }
                });
            }
        });
    };
    move || match msg.get() {
        None => view! { <p class="center muted">"Cargando…"</p> }.into_any(),
        Some(Err(e)) => view! { <p class="center error">{e}</p> }.into_any(),
        Some(Ok(m)) => {
            let raw = m
                .html
                .clone()
                .unwrap_or_else(|| sanitize::text_to_html(m.text.as_deref().unwrap_or("")));
            let clean = sanitize::clean(&raw, images.get());
            let doc = sanitize::document(&clean.html, prefers_dark());
            let from = match (&m.from_name, &m.from_address) {
                (Some(n), Some(a)) if !n.is_empty() => format!("{n} <{a}>"),
                (_, Some(a)) => a.clone(),
                _ => "(sin remitente)".into(),
            };
            let to =
                m.to.iter()
                    .map(|a| a.address.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
            let date = m
                .date
                .with_timezone(&Local)
                .format("%d/%m/%Y %H:%M")
                .to_string();
            view! {
                <article class="reader">
                    <header>
                        <button class="ghost only-mobile" on:click=move |_| on_back.run(())>"← Volver"</button>
                        <h2>{if m.subject.is_empty() { "(sin asunto)".to_string() } else { m.subject.clone() }}</h2>
                        <p><span class="strong">{from}</span></p>
                        <p class="muted small">"Para: " {to} " · " {date} " · " {m.account_email.clone()}</p>
                        <div class="actions">
                            <button class="ghost" on:click=toggle_star>{if m.flagged { "★ Destacado" } else { "☆ Destacar" }}</button>
                            {clean.blocked_images.then(|| view! {
                                <button class="ghost" on:click=move |_| images.set(true)>"Mostrar imágenes"</button>
                            })}
                        </div>
                        {(!m.attachments.is_empty()).then(|| view! {
                            <p class="muted small">
                                "Adjuntos: "
                                {m.attachments.iter().map(|a| a.filename.clone().unwrap_or_else(|| "(sin nombre)".into())).collect::<Vec<_>>().join(", ")}
                            </p>
                        })}
                    </header>
                    // Sin allow-scripts ni allow-same-origin: el correo no puede ejecutar nada
                    <iframe class="body" title="Contenido del mensaje" sandbox="allow-popups allow-popups-to-escape-sandbox" srcdoc=doc></iframe>
                </article>
            }
            .into_any()
        }
    }
}
