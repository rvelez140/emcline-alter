fn main() {
    // Comandos propios con permisos explícitos (allow-<comando>) para poder darlos
    // a la web remota del servidor en capabilities/remote.json
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "open_server",
            "forget_server",
            "notify",
            "set_unread",
            "sso_login",
        ]),
    ))
    .expect("tauri-build");
}
