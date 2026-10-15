//! Llamadas a `/api` del mismo servidor (cookie de sesión, como la PWA).

use gloo_net::http::{Request, Response};
use rvmail_types::{
    AccountDto, ApiErrorBody, FolderDto, LoginInput, MessageFull, MessagePage, UserDto,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use uuid::Uuid;

/// Error para mostrar al usuario (en español, sin trazas).
#[derive(Clone, Debug, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

fn network() -> ApiError {
    ApiError {
        status: 0,
        message: "No se pudo conectar con el servidor".into(),
    }
}

async fn read<T: DeserializeOwned>(res: Response) -> ApiResult<T> {
    if res.ok() {
        return res.json::<T>().await.map_err(|_| ApiError {
            status: res.status(),
            message: "Respuesta inesperada del servidor".into(),
        });
    }
    let status = res.status();
    let message = res
        .json::<ApiErrorBody>()
        .await
        .map(|e| e.message)
        .unwrap_or_else(|_| format!("Error del servidor ({status})"));
    Err(ApiError { status, message })
}

async fn get<T: DeserializeOwned>(url: &str) -> ApiResult<T> {
    read(Request::get(url).send().await.map_err(|_| network())?).await
}

async fn send<B: Serialize, T: DeserializeOwned>(
    method: &str,
    url: &str,
    body: &B,
) -> ApiResult<T> {
    let builder = match method {
        "POST" => Request::post(url),
        "PATCH" => Request::patch(url),
        _ => Request::delete(url),
    };
    let req = builder.json(body).map_err(|_| network())?;
    read(req.send().await.map_err(|_| network())?).await
}

pub async fn me() -> ApiResult<UserDto> {
    get("/api/auth/me").await
}

pub async fn login(email: String, password: String) -> ApiResult<UserDto> {
    send("POST", "/api/auth/login", &LoginInput { email, password }).await
}

pub async fn logout() {
    let _ = Request::post("/api/auth/logout").send().await;
}

pub async fn accounts() -> ApiResult<Vec<AccountDto>> {
    get("/api/accounts").await
}

pub async fn folders() -> ApiResult<Vec<FolderDto>> {
    get("/api/folders").await
}

/// Qué lista se muestra: vista unificada por carpeta especial o una carpeta concreta.
#[derive(Clone, Debug, PartialEq)]
pub enum View {
    Unified(&'static str),
    Folder {
        account: Uuid,
        folder: Uuid,
        name: String,
    },
}

/// Codifica un valor para la query string.
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub async fn messages(view: &View, q: &str, cursor: Option<&str>) -> ApiResult<MessagePage> {
    let mut url = match view {
        View::Unified(special) => format!("/api/messages?special={special}&limit=50"),
        View::Folder {
            account, folder, ..
        } => {
            format!("/api/messages?accountId={account}&folderId={folder}&limit=50")
        }
    };
    if !q.trim().is_empty() {
        url.push_str(&format!("&q={}", enc(q.trim())));
    }
    if let Some(c) = cursor {
        url.push_str(&format!("&cursor={}", enc(c)));
    }
    get(&url).await
}

pub async fn message(id: Uuid) -> ApiResult<MessageFull> {
    get(&format!("/api/messages/{id}")).await
}

#[derive(Serialize)]
struct Flags {
    #[serde(skip_serializing_if = "Option::is_none")]
    seen: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flagged: Option<bool>,
}

pub async fn set_flags(
    id: Uuid,
    seen: Option<bool>,
    flagged: Option<bool>,
) -> ApiResult<serde_json::Value> {
    send(
        "PATCH",
        &format!("/api/messages/{id}"),
        &Flags { seen, flagged },
    )
    .await
}
