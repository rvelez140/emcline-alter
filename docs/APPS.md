# Apps de escritorio y móvil

RV Mail se usa desde tres sitios, todos contra el mismo servidor:

| Cliente | Qué es | Estado |
|---|---|---|
| **PWA** (navegador) | La web instalada como app (iPhone, Android, Windows, Linux, macOS) | Lista |
| **App de escritorio** (Tauri 2) | Windows, macOS y Linux: bandeja del sistema, avisos nativos, `mailto:` | Lista (Linux probado; instaladores de Windows/macOS por CI) |
| **App móvil** (Tauri 2) | Android e iOS con el mismo código | Base lista; falta generar los proyectos nativos (ver abajo) |

## App de escritorio (`apps/desktop`)

La ventana muestra la web de **tu** servidor (misma interfaz y funciones que la
PWA, se actualiza sola con el servidor) y la app añade lo nativo:

- **Primera vez:** pide la dirección del servidor (`correo.tuempresa.com`), la
  valida con `/.well-known/rvmail` y la recuerda. Tray → *Cambiar de servidor…*.
- **Bandeja del sistema:** cerrar la ventana la deja en segundo plano avisando
  del correo nuevo. Menú: abrir, redactar, cambiar de servidor, salir.
- **Avisos nativos** de correo nuevo y **contador de no leídos** (tooltip,
  título e insignia del icono en macOS/Linux).
- **Enlaces `mailto:`**: la app se registra como manejador; al pulsar un enlace
  de correo se abre la redacción con destinatario, asunto, CC y cuerpo.
- **Google, Microsoft y Yahoo** se abren en el **navegador del sistema** (Google
  bloquea OAuth dentro de webviews) y vuelven a la app por `rvmail://auth` con
  PKCE: nadie más puede canjear el código.
- **Seguridad:** la ventana solo navega dentro del servidor elegido; cualquier
  otro enlace se abre en el navegador. La web remota solo puede usar los
  comandos de avisos, contador, SSO y cambiar de servidor
  (`src-tauri/capabilities/remote.json`).

### Compilar

```bash
# Linux: librerías del sistema
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev
pnpm install
pnpm desktop                                   # desarrollo (tauri dev)
pnpm --filter @rvmail/desktop build            # instaladores en src-tauri/target/release/bundle
```

Windows necesita WebView2 (incluido en Windows 10/11) y macOS Xcode CLT. Al
crear un tag `v*`, el workflow publica en la release los instaladores de
Windows (`.msi`/`.exe`), macOS universal (`.dmg`) y Linux (`.deb`, `.rpm`,
`.AppImage`). Firma de código: añade los secretos de Apple/Windows que indica
la documentación de `tauri-action` cuando quieras distribuir sin avisos.

### Móvil (Android / iOS)

El código (`src-tauri/src/lib.rs`, con `mobile_entry_point`) ya es compatible.
Para generar los proyectos nativos hace falta el SDK de cada plataforma:

```bash
cd apps/desktop
pnpm tauri android init && pnpm tauri android build   # Android Studio + NDK
pnpm tauri ios init && pnpm tauri ios build           # macOS + Xcode
```

Pendiente en móvil: avisos push con la app cerrada (FCM/APNs) y el esquema
`rvmail://` como *app link*. Mientras tanto, en el móvil la PWA ofrece avisos
mientras está abierta y se instala desde el navegador.

## API para apps nativas e integraciones

Para clientes propios (o la app móvil sin webview) hay sesiones por
dispositivo con tokens. Todo bajo `/api`:

| Método | Ruta | Uso |
|---|---|---|
| `GET` | `/.well-known/rvmail` | Descubrimiento: versión, métodos de acceso, proveedores SSO (CORS abierto) |
| `POST` | `/auth/token` | `{email, password, deviceName, platform}` → `{accessToken, refreshToken, expiresIn, user}` |
| `POST` | `/auth/token/refresh` | `{refreshToken}` → tokens nuevos (el refresh **rota** en cada uso) |
| `POST` | `/auth/token/revoke` | `{refreshToken}` → cierra la sesión del dispositivo |
| `GET` | `/auth/sso/{google\|microsoft\|yahoo}/start?app_redirect=rvmail://auth&app_challenge=<S256>` | SSO desde una app (navegador del sistema) |
| `POST` | `/auth/token/exchange` | `{code, codeVerifier, deviceName, platform}` → tokens |
| `GET` | `/auth/token/web?code=&code_verifier=` | Igual, pero deja la sesión de la web (cookie); lo usa la app de escritorio |
| `GET`/`DELETE` | `/auth/sessions[/{id}]` | Dispositivos conectados (también en Configuración → Inicio de sesión) |

- **Access token:** JWT de 15 minutos, `Authorization: Bearer …`, vale para toda
  la API del usuario (incluido el tiempo real `/api/events`). Si se cierra la
  sesión del dispositivo deja de valer al instante.
- **Refresh token:** `rvr_…`, 60 días desde el último uso, guardado solo como
  hash. Si alguien presenta uno ya rotado (copiado y usado antes), se revoca la
  sesión entera de ese dispositivo.
- **`app_redirect`** solo admite `rvmail://…` o `http://127.0.0.1:<puerto>/…`
  (RFC 8252); el código vale 2 minutos y una sola vez.
- **CORS:** solo para tokens Bearer, nunca cookies. Orígenes en `CORS_ORIGINS`
  (por defecto los de Tauri).
- Los **tokens de agentes de IA** (`rvm_…`, con ámbitos) siguen siendo otra cosa:
  ver [AI-AGENT.md](AI-AGENT.md).
