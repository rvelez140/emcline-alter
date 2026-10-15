//! Muchos servidores (y GreenMail en desarrollo) no anuncian SPECIAL-USE:
//! se deduce por nombre, en español e inglés.

pub fn resolve(path: &str, name: &str, announced: Option<&str>) -> Option<String> {
    if let Some(u) = announced {
        return Some(u.to_string());
    }
    if path.eq_ignore_ascii_case("INBOX") {
        return Some("\\Inbox".into());
    }
    let leaf = name.trim().to_lowercase();
    let m = |names: &[&str]| names.contains(&leaf.as_str());
    let use_ = if m(&["inbox"]) {
        "\\Inbox"
    } else if m(&[
        "sent",
        "sent items",
        "sent messages",
        "enviados",
        "elementos enviados",
        "mensajes enviados",
    ]) {
        "\\Sent"
    } else if m(&["drafts", "borradores"]) {
        "\\Drafts"
    } else if m(&[
        "trash",
        "deleted",
        "deleted items",
        "deleted messages",
        "papelera",
        "elementos eliminados",
    ]) {
        "\\Trash"
    } else if m(&[
        "junk",
        "spam",
        "junk e-mail",
        "junk email",
        "correo no deseado",
        "no deseado",
    ]) {
        "\\Junk"
    } else if m(&["archive", "archives", "archivo", "archivados"]) {
        "\\Archive"
    } else {
        return None;
    };
    Some(use_.into())
}

/// Orden de sincronización: lo que más importa al usuario primero.
pub fn priority(special_use: Option<&str>) -> u8 {
    match special_use {
        Some("\\Inbox") => 0,
        Some("\\Sent") => 1,
        Some("\\Drafts") => 2,
        Some("\\Archive") => 3,
        Some("\\Junk") => 8,
        Some("\\Trash") => 9,
        _ => 5,
    }
}

/// Carpetas virtuales que duplican mensajes (Gmail) y no se sincronizan.
pub fn skip(special_use: Option<&str>) -> bool {
    matches!(special_use, Some("\\All" | "\\Flagged" | "\\Important"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_name() {
        assert_eq!(resolve("INBOX", "INBOX", None).as_deref(), Some("\\Inbox"));
        assert_eq!(
            resolve("Enviados", "Enviados", None).as_deref(),
            Some("\\Sent")
        );
        assert_eq!(
            resolve("x", "Correo no deseado", None).as_deref(),
            Some("\\Junk")
        );
        assert_eq!(resolve("Trabajo", "Trabajo", None), None);
        assert_eq!(
            resolve("[Gmail]/Todos", "Todos", Some("\\All")).as_deref(),
            Some("\\All")
        );
        assert!(skip(Some("\\All")));
    }
}
