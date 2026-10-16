//! Conversión de un FETCH IMAP en una fila de `messages`.

use async_imap::imap_proto::{BodyStructure, ContentEncoding};
use chrono::{DateTime, TimeZone, Utc};
use mail_parser::{HeaderValue, MessageParser};
use rvmail_core::models::Address;
use uuid::Uuid;

#[derive(Debug)]
pub struct MessageRow {
    pub account_id: Uuid,
    pub folder_id: Uuid,
    pub uid: i64,
    /// Id del mensaje en el servidor JMAP (None en IMAP)
    pub remote_id: Option<String>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub thread_key: String,
    pub subject: String,
    pub from_name: Option<String>,
    pub from_address: Option<String>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub date: DateTime<Utc>,
    pub snippet: String,
    pub flags: Vec<String>,
    pub seen: bool,
    pub flagged: bool,
    pub has_attachments: bool,
    pub size: u32,
}

#[allow(clippy::too_many_arguments)]
pub fn to_row(
    account_id: Uuid,
    folder_id: Uuid,
    uid: u32,
    flags: Vec<String>,
    size: u32,
    internal_date: Option<DateTime<Utc>>,
    structure: Option<&BodyStructure<'_>>,
    header: &[u8],
    snippet_text: Option<String>,
) -> MessageRow {
    let parsed = MessageParser::default().parse(header);
    let msg = parsed.as_ref();

    let subject = msg
        .and_then(|m| m.subject())
        .unwrap_or_default()
        .to_string();
    let from = msg.and_then(|m| m.from()).and_then(|a| a.first());
    let date = msg
        .and_then(|m| m.date())
        .and_then(|d| Utc.timestamp_opt(d.to_timestamp(), 0).single())
        .or(internal_date)
        .unwrap_or_else(Utc::now);

    MessageRow {
        account_id,
        folder_id,
        uid: i64::from(uid),
        remote_id: None,
        message_id: msg.and_then(|m| m.message_id()).map(|id| format!("<{id}>")),
        in_reply_to: msg
            .and_then(|m| first_id(m.in_reply_to()))
            .map(|id| format!("<{id}>")),
        thread_key: normalize_subject(&subject),
        from_name: from
            .and_then(|a| a.name())
            .filter(|n| !n.is_empty())
            .map(str::to_string),
        from_address: from.and_then(|a| a.address()).map(str::to_string),
        to: addresses(msg.and_then(|m| m.to())),
        cc: addresses(msg.and_then(|m| m.cc())),
        subject,
        date,
        snippet: snippet_text.map(|t| collapse(&t)).unwrap_or_default(),
        seen: flags.iter().any(|f| f == "\\Seen"),
        flagged: flags.iter().any(|f| f == "\\Flagged"),
        flags,
        has_attachments: structure.is_some_and(has_attachment),
        size,
    }
}

fn first_id(v: &HeaderValue<'_>) -> Option<String> {
    match v {
        HeaderValue::Text(t) => Some(t.to_string()),
        HeaderValue::TextList(l) => l.first().map(|t| t.to_string()),
        _ => None,
    }
}

fn addresses(a: Option<&mail_parser::Address<'_>>) -> Vec<Address> {
    a.map(|a| {
        a.iter()
            .filter_map(|x| {
                Some(Address {
                    name: x.name().filter(|n| !n.is_empty()).map(str::to_string),
                    address: x.address()?.to_string(),
                })
            })
            .collect()
    })
    .unwrap_or_default()
}

fn collapse(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(240).collect()
}

/// Parte de texto que se usa para la vista previa: sección IMAP (p. ej. "1.1"),
/// cabeceras MIME mínimas para decodificarla y tamaño en bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPart {
    pub section: String,
    pub mime_header: String,
    pub octets: u32,
    pub html: bool,
}

/// Elige la primera parte text/plain (o, si no hay, text/html) que no sea adjunto.
pub fn text_part(b: &BodyStructure<'_>) -> Option<TextPart> {
    fn walk<'a>(
        b: &'a BodyStructure<'a>,
        path: &mut Vec<u32>,
        top: bool,
        out: &mut Vec<(Vec<u32>, &'a BodyStructure<'a>)>,
    ) {
        match b {
            BodyStructure::Multipart { bodies, .. } => {
                for (i, child) in bodies.iter().enumerate() {
                    path.push(i as u32 + 1);
                    walk(child, path, false, out);
                    path.pop();
                }
            }
            BodyStructure::Text { .. } => {
                // Un mensaje no multiparte tiene su cuerpo en la parte "1"
                out.push((if top { vec![1] } else { path.clone() }, b));
            }
            _ => {}
        }
    }
    let mut parts = Vec::new();
    walk(b, &mut Vec::new(), true, &mut parts);
    let usable = |(_, p): &&(Vec<u32>, &BodyStructure<'_>)| {
        let BodyStructure::Text { common, .. } = p else {
            return false;
        };
        !common
            .disposition
            .as_ref()
            .is_some_and(|d| d.ty.eq_ignore_ascii_case("attachment"))
    };
    let pick = parts
        .iter()
        .filter(usable)
        .find(|(_, p)| matches!(p, BodyStructure::Text { common, .. } if common.ty.subtype.eq_ignore_ascii_case("plain")))
        .or_else(|| parts.iter().filter(usable).find(|(_, p)| {
            matches!(p, BodyStructure::Text { common, .. } if common.ty.subtype.eq_ignore_ascii_case("html"))
        }))?;
    let BodyStructure::Text { common, other, .. } = pick.1 else {
        return None;
    };
    let charset = common
        .ty
        .params
        .as_ref()
        .and_then(|ps| ps.iter().find(|(k, _)| k.eq_ignore_ascii_case("charset")))
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| "us-ascii".into());
    let encoding = match &other.transfer_encoding {
        ContentEncoding::Base64 => "base64".to_string(),
        ContentEncoding::QuotedPrintable => "quoted-printable".into(),
        ContentEncoding::EightBit => "8bit".into(),
        ContentEncoding::Binary => "binary".into(),
        ContentEncoding::SevenBit => "7bit".into(),
        ContentEncoding::Other(o) => o.to_string(),
    };
    Some(TextPart {
        section: pick.0.iter().map(u32::to_string).collect::<Vec<_>>().join("."),
        mime_header: format!(
            "Content-Type: text/{}; charset=\"{charset}\"\r\nContent-Transfer-Encoding: {encoding}\r\n\r\n",
            common.ty.subtype.to_lowercase()
        ),
        octets: other.octets,
        html: common.ty.subtype.eq_ignore_ascii_case("html"),
    })
}

/// Decodifica los bytes de una parte de texto (transfer-encoding y charset).
pub fn decode_text(part: &TextPart, data: &[u8]) -> Option<String> {
    let mut raw = part.mime_header.as_bytes().to_vec();
    raw.extend_from_slice(data);
    let m = MessageParser::default().parse(&raw)?;
    if part.html {
        m.body_html(0).map(|h| strip_tags(&h))
    } else {
        m.body_text(0).map(|t| t.into_owned())
    }
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Clave de hilo: asunto sin prefijos Re:/Fwd:/RV:… en minúsculas.
pub fn normalize_subject(s: &str) -> String {
    let mut rest = s.trim();
    loop {
        let lower = rest.to_lowercase();
        let Some(prefix) = ["re", "fw", "fwd", "rv", "res", "enc"]
            .iter()
            .find(|p| lower.starts_with(*p) && lower[p.len()..].trim_start().starts_with(':'))
        else {
            break;
        };
        let after = &rest[prefix.len()..];
        rest = after.trim_start()[1..].trim_start();
    }
    rest.to_lowercase()
}

fn has_attachment(b: &BodyStructure<'_>) -> bool {
    let (common, children): (_, Vec<&BodyStructure<'_>>) = match b {
        BodyStructure::Basic { common, .. } | BodyStructure::Text { common, .. } => {
            (common, vec![])
        }
        BodyStructure::Message { common, body, .. } => (common, vec![body.as_ref()]),
        BodyStructure::Multipart { common, bodies, .. } => (common, bodies.iter().collect()),
    };
    if common
        .disposition
        .as_ref()
        .is_some_and(|d| d.ty.eq_ignore_ascii_case("attachment"))
    {
        return true;
    }
    children.into_iter().any(has_attachment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_prefixes() {
        assert_eq!(normalize_subject("Re: RE: Fwd: Hola Mundo"), "hola mundo");
        assert_eq!(normalize_subject("RV:Cotización"), "cotización");
        assert_eq!(normalize_subject("Reunión mañana"), "reunión mañana");
    }

    #[test]
    fn parses_message() {
        let raw = b"From: =?UTF-8?Q?Jos=C3=A9?= <jose@example.com>\r\nTo: a@x.com, B <b@x.com>\r\nSubject: =?UTF-8?Q?Cotizaci=C3=B3n?=\r\nMessage-ID: <abc@x>\r\nDate: Tue, 13 Oct 2026 10:00:00 +0000\r\n\r\nHola,\r\n  necesito   la cotizaci\xc3\xb3n.\r\n";
        let r = to_row(
            Uuid::nil(),
            Uuid::nil(),
            7,
            vec!["\\Seen".into()],
            100,
            None,
            None,
            raw,
            Some("Hola,\r\n  necesito   la cotizaci\u{f3}n.".into()),
        );
        assert_eq!(r.subject, "Cotización");
        assert_eq!(r.from_name.as_deref(), Some("José"));
        assert_eq!(r.to.len(), 2);
        assert_eq!(r.message_id.as_deref(), Some("<abc@x>"));
        assert_eq!(r.snippet, "Hola, necesito la cotización.");
        assert!(r.seen);
        assert_eq!(r.date.to_rfc3339(), "2026-10-13T10:00:00+00:00");
    }

    #[test]
    fn decodes_quoted_printable_part() {
        let part = TextPart {
            section: "1".into(),
            mime_header: "Content-Type: text/plain; charset=\"utf-8\"\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n".into(),
            octets: 10,
            html: false,
        };
        assert_eq!(
            decode_text(&part, b"n=C3=BAmero con =C3=B1").as_deref(),
            Some("n\u{fa}mero con \u{f1}")
        );
    }
}
