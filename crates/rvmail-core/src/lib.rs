//! Núcleo compartido de RV Mail en Rust: configuración, cifrado de
//! credenciales (compatible con el formato `v1:` de la versión Node),
//! modelos de la base de datos y eventos de tiempo real.

pub mod config;
pub mod crypto;
pub mod db;
pub mod events;
pub mod models;
