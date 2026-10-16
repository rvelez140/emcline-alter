# Google, Microsoft y Yahoo: inicio de sesión y cuentas sin contraseñas

RV Mail usa **OpenID Connect / OAuth2** con cada proveedor para dos cosas:

1. **Entrar a RV Mail** con el botón "Continuar con Google / Microsoft / Yahoo".
2. **Conectar un buzón** (Gmail, Outlook/Microsoft 365, Yahoo) sin contraseñas de
   aplicación: RV Mail guarda un *refresh token* cifrado y usa XOAUTH2 en IMAP/SMTP.

Cada proveedor exige que **tú** registres una aplicación. Lo que no configures
simplemente no aparece.

**URL de retorno (redirect URI)** de cada proveedor, exactamente así:

```
https://TU_DOMINIO/api/auth/sso/google/callback
https://TU_DOMINIO/api/auth/sso/microsoft/callback
https://TU_DOMINIO/api/auth/sso/yahoo/callback
```

Lo más fácil: `sudo rvmail config sso` (pide Client ID y secreto de cada proveedor y aplica el cambio). A mano: pon los datos en `.env` y reinicia con `docker compose up -d`.

## Google

1. <https://console.cloud.google.com> → crea un proyecto.
2. **APIs y servicios → Pantalla de consentimiento de OAuth**: tipo *Externo*;
   añade los ámbitos `openid`, `email`, `profile` y, para conectar buzones,
   `https://mail.google.com/`.
3. **Credenciales → Crear credenciales → ID de cliente de OAuth → Aplicación web**,
   con la URL de retorno de arriba.
4. `.env`: `SSO_GOOGLE_CLIENT_ID=…` y `SSO_GOOGLE_CLIENT_SECRET=…`

> El ámbito de Gmail (`https://mail.google.com/`) es **restringido**: en modo
> *Prueba* funciona para hasta 100 usuarios que añadas como testers. Para uso
> público Google exige verificación y una evaluación de seguridad. Solo para
> *iniciar sesión* (sin conectar Gmail) no hace falta.

## Microsoft (Entra ID: Outlook.com y Microsoft 365)

1. <https://entra.microsoft.com> → **Aplicaciones → Registros de aplicaciones → Nuevo registro**.
2. *Tipos de cuenta*: "Cuentas en cualquier directorio y cuentas personales"
   (o solo tu organización). URI de redirección **Web** = la de arriba.
3. **Certificados y secretos → Nuevo secreto de cliente**.
4. **Permisos de API → Agregar → APIs que usa mi organización → Office 365 Exchange Online**
   (o "Microsoft Graph" para `openid/email/profile`): `IMAP.AccessAsUser.All`,
   `SMTP.Send`, `offline_access`, `openid`, `email`, `profile`.
5. `.env`: `SSO_MICROSOFT_CLIENT_ID`, `SSO_MICROSOFT_CLIENT_SECRET` y
   `SSO_MICROSOFT_TENANT` (`common`, `organizations`, `consumers` o el ID de tu inquilino).

> En Microsoft 365 el administrador del inquilino debe tener **SMTP AUTH**
> habilitado para el buzón si se va a enviar por SMTP.

## Yahoo

1. <https://developer.yahoo.com/apps/> → **Create an App**, tipo *Web Application*,
   con la URL de retorno de arriba.
2. Permisos: **OpenID Connect** (Email) y, para conectar buzones, **Mail (Read/Write)**.
3. `.env`: `SSO_YAHOO_CLIENT_ID`, `SSO_YAHOO_CLIENT_SECRET`.

> Yahoo aprueba el permiso de correo caso por caso. Mientras no lo apruebe,
> el inicio de sesión funciona pero "Conectar con Yahoo" no.

## Quién puede entrar

- Si la identidad ya está **vinculada** a un usuario, entra.
- Si no, y Google o Yahoo confirman que el correo está **verificado** y ya existe
  un usuario con ese correo, se vincula automáticamente.
- Con **Microsoft no se vincula por correo** (en cuentas de organización el
  correo no siempre está verificado; es el fallo conocido como "nOAuth"): el
  usuario entra con su contraseña y lo vincula en **Configuración → Inicio de sesión**.
- Personas nuevas: solo si `SSO_AUTO_SIGNUP=true` (desactivado por defecto).

## Seguridad

- Flujo de código con `state`, `nonce` y PKCE (S256) en Google y Microsoft.
- Se valida el `id_token` (`iss`, `aud`, `exp`, `nonce`); llega directamente del
  proveedor por TLS (OIDC Core 3.1.3.7).
- Refresh tokens cifrados con AES-256-GCM (`MASTER_KEY`); access tokens cifrados
  en Redis solo mientras son válidos. Si el usuario revoca el acceso, la cuenta
  pasa a "vuelve a conectar la cuenta".
