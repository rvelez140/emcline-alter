//! HTML de correos = no confiable (regla 3): se sanea con ammonia (equivalente a
//! DOMPurify) y se muestra en un `<iframe sandbox>` sin scripts. Las imágenes
//! remotas se bloquean salvo que el usuario las permita.

use std::collections::HashSet;

pub struct Clean {
    pub html: String,
    pub blocked_images: bool,
}

pub fn clean(html: &str, allow_remote_images: bool) -> Clean {
    use std::sync::atomic::{AtomicBool, Ordering};
    let blocked = std::sync::Arc::new(AtomicBool::new(false));
    let flag = blocked.clone();
    let out = ammonia::Builder::default()
        .url_schemes(HashSet::from(["http", "https", "mailto", "cid", "data"]))
        .link_rel(Some("noopener noreferrer"))
        .add_generic_attributes([
            "style", "align", "valign", "width", "height", "bgcolor", "color",
        ])
        .add_tags(["center", "font"])
        .attribute_filter(move |element, attribute, value| {
            let remote = value.starts_with("http://")
                || value.starts_with("https://")
                || value.starts_with("//");
            match (element, attribute) {
                // Píxeles de seguimiento e imágenes remotas: fuera salvo permiso
                ("img", "src") | ("img", "srcset") if remote && !allow_remote_images => {
                    flag.store(true, Ordering::Relaxed);
                    None
                }
                (_, "style") if !allow_remote_images && value.contains("url(") => {
                    flag.store(true, Ordering::Relaxed);
                    None
                }
                _ => Some(value.into()),
            }
        })
        .clean(html)
        .to_string();
    Clean {
        html: out,
        blocked_images: blocked.load(Ordering::Relaxed),
    }
}

/// Documento completo para el `srcdoc` del iframe: enlaces en pestaña nueva.
pub fn document(body: &str, dark: bool) -> String {
    let (bg, fg) = if dark {
        ("#0f172a", "#e2e8f0")
    } else {
        ("#ffffff", "#0f172a")
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><base target=\"_blank\">\
         <style>body{{margin:16px;font:14px/1.5 system-ui,sans-serif;background:{bg};color:{fg};word-wrap:break-word}}\
         img{{max-width:100%;height:auto}}a{{color:#2563eb}}blockquote{{margin:0 0 0 8px;padding-left:8px;border-left:2px solid #94a3b8}}</style>\
         </head><body>{body}</body></html>"
    )
}

/// Texto plano → HTML seguro.
pub fn text_to_html(text: &str) -> String {
    ammonia::clean_text(text).replace("&#10;", "<br>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scripts_and_tracking() {
        let c = clean(
            r#"<p onclick="x()">Hola<script>alert(1)</script><img src="https://t.example/pixel.gif"><a href="javascript:alert(1)">x</a></p>"#,
            false,
        );
        assert!(
            !c.html.contains("script")
                && !c.html.contains("onclick")
                && !c.html.contains("javascript")
        );
        assert!(!c.html.contains("pixel.gif"));
        assert!(c.blocked_images);
        let c = clean(r#"<img src="https://t.example/logo.png">"#, true);
        assert!(c.html.contains("logo.png") && !c.blocked_images);
    }
}
