//! RV Mail — cliente web en Rust (Leptos + WebAssembly), fase C3.
//!
//! Se reescribe por pantallas sobre la misma API que la PWA React y con los
//! tipos compartidos de `crates/rvmail-types`. Hecho: inicio de sesión, bandeja
//! unificada y por carpeta, búsqueda, lectura segura y tiempo real.

mod api;
mod mail;
mod sanitize;

use leptos::prelude::*;
use leptos::task::spawn_local;
use rvmail_types::UserDto;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let user = RwSignal::new(None::<UserDto>);
    let checked = RwSignal::new(false);
    spawn_local(async move {
        if let Ok(u) = api::me().await {
            user.set(Some(u));
        }
        checked.set(true);
    });
    view! {
        <Show when=move || checked.get() fallback=|| view! { <div class="center muted">"Cargando…"</div> }>
            {move || match user.get() {
                Some(u) => view! { <mail::MailApp user=u on_logout=move |_| user.set(None) /> }.into_any(),
                None => view! { <Login on_login=move |u| user.set(Some(u)) /> }.into_any(),
            }}
        </Show>
    }
}

#[component]
fn Login(#[prop(into)] on_login: Callback<UserDto>) -> impl IntoView {
    let email = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match api::login(email.get_untracked(), password.get_untracked()).await {
                Ok(u) => on_login.run(u),
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    view! {
        <main class="center">
            <form class="card auth" on:submit=submit>
                <div class="brand">
                    <span class="logo">"✉"</span>
                    <div>
                        <h1>"RV Mail"</h1>
                        <p class="muted">"Soluciones RV · versión Rust"</p>
                    </div>
                </div>
                <label>
                    "Correo"
                    <input type="email" required autocomplete="username" bind:value=email />
                </label>
                <label>
                    "Contraseña"
                    <input type="password" required autocomplete="current-password" bind:value=password />
                </label>
                <button class="primary" type="submit" disabled=move || busy.get()>
                    {move || if busy.get() { "Entrando…" } else { "Entrar" }}
                </button>
                <p class="error" role="alert">{move || error.get().unwrap_or_default()}</p>
            </form>
        </main>
    }
}
