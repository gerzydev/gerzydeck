//! `gdplugin://localhost/<plugin-id>/<path>` — serves plugin files (property inspectors,
//! HTML plugins, icons) to our webviews, confined to the plugin's folder. HTML responses get a
//! small bootstrap that calls `connectElgatoStreamDeckSocket` with parameters from the URL hash
//! (`#gd=<base64 json>`) and reports the page height to the parent editor.

use base64::Engine as _;
use std::path::{Component, Path, PathBuf};

pub const SCHEME: &str = "gdplugin";

pub struct Served {
    pub status: u16,
    pub mime: String,
    pub body: Vec<u8>,
}

fn not_found() -> Served {
    Served { status: 404, mime: "text/plain".into(), body: b"not found".to_vec() }
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("wav") => "audio/wav",
        Some("mp3") => "audio/mpeg",
        _ => "application/octet-stream",
    }
}

/// Directory of an installed plugin by id (with or without the `.sdPlugin` suffix).
pub fn plugin_dir(plugins_dir: &Path, id: &str) -> Option<PathBuf> {
    if id.is_empty() || id.contains(['/', '\\', ':']) || id == "." || id == ".." {
        return None;
    }
    [plugins_dir.join(id), plugins_dir.join(format!("{id}.sdPlugin"))].into_iter().find(|p| p.is_dir())
}

/// Serves `/<plugin-id>/<path>` from `plugins_dir`, confined to that plugin's folder.
pub fn serve(plugins_dir: &Path, url_path: &str) -> Served {
    let path = url_path.split(['?', '#']).next().unwrap_or("");
    let Some(path) = percent_decode(path) else { return not_found() };
    let path = path.replace('\\', "/");
    let mut parts = path.trim_start_matches('/').splitn(2, '/');
    let (Some(id), Some(rest)) = (parts.next(), parts.next()) else { return not_found() };
    let Some(dir) = plugin_dir(plugins_dir, id) else { return not_found() };
    let rel = Path::new(rest);
    if rest.is_empty() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return not_found();
    }
    let (Ok(root), Ok(file)) = (dir.canonicalize(), dir.join(rel).canonicalize()) else { return not_found() };
    if !file.starts_with(&root) || !file.is_file() {
        return not_found();
    }
    let Ok(body) = std::fs::read(&file) else { return not_found() };
    let mime = mime_for(&file);
    let body = if mime.starts_with("text/html") { inject_bootstrap(&body) } else { body };
    Served { status: 200, mime: mime.into(), body }
}

const BOOTSTRAP: &str = r#"<script>(function(){
  var m = location.hash.match(/gd=([^&]+)/);
  if (!m) return;
  var p;
  try { p = JSON.parse(decodeURIComponent(escape(atob(m[1])))); } catch (e) { console.error('gd bootstrap', e); return; }
  window.addEventListener('load', function () {
    if (typeof window.connectElgatoStreamDeckSocket === 'function') {
      window.connectElgatoStreamDeckSocket(p.port, p.uuid, p.registerEvent, JSON.stringify(p.info), p.actionInfo ? JSON.stringify(p.actionInfo) : undefined);
    } else {
      console.warn('gd bootstrap: connectElgatoStreamDeckSocket not defined');
    }
    if (window.parent !== window) {
      // Elgato PIs assume a ~350 px wide window: never scroll sideways inside our panel
      var st = document.createElement('style');
      st.textContent = 'html,body{overflow-x:hidden!important;max-width:100%}';
      document.head.appendChild(st);
      // height of the real content (body is often height:100%, so scrollHeight would never shrink)
      var contentHeight = function () {
        var bottom = 0, kids = document.body ? document.body.children : [];
        for (var i = 0; i < kids.length; i++) {
          var r = kids[i].getBoundingClientRect();
          if (r.height > 0) bottom = Math.max(bottom, r.bottom + window.scrollY);
        }
        return Math.ceil(bottom + 16);
      };
      var send = function () { window.parent.postMessage({ type: 'gd-pi-height', height: contentHeight() }, '*'); };
      send();
      if (window.ResizeObserver) new ResizeObserver(send).observe(document.body || document.documentElement);
      setInterval(send, 1000);
    }
  });
})();</script>"#;

fn inject_bootstrap(html: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(html);
    let lower = text.to_ascii_lowercase();
    let at = lower.find("<head").and_then(|i| lower[i..].find('>').map(|j| i + j + 1)).unwrap_or(0);
    let mut out = String::with_capacity(text.len() + BOOTSTRAP.len());
    out.push_str(&text[..at]);
    out.push_str(BOOTSTRAP);
    out.push_str(&text[at..]);
    out.into_bytes()
}

/// URL of a plugin page (property inspector or HTML plugin) with launch parameters in the hash.
/// Webview2 maps custom schemes to `http://<scheme>.localhost/`.
pub fn page_url(plugin_id: &str, rel_path: &str, params: &serde_json::Value) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(params.to_string());
    let rel = rel_path.replace('\\', "/");
    format!("http://{SCHEME}.localhost/{plugin_id}/{}#gd={b64}", rel.trim_start_matches('/'))
}

/// URL of a static plugin asset (icons) without parameters.
pub fn asset_url(plugin_id: &str, rel_path: &str) -> String {
    format!("http://{SCHEME}.localhost/{plugin_id}/{}", rel_path.replace('\\', "/").trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugins_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("gd-scheme-{}", crate::config::new_id()));
        let p = d.join("com.x.sdPlugin");
        std::fs::create_dir_all(p.join("pi")).unwrap();
        std::fs::write(p.join("pi/index.html"), "<html><head><title>x</title></head><body>PI</body></html>").unwrap();
        std::fs::write(p.join("icon.png"), [0x89, b'P', b'N', b'G']).unwrap();
        std::fs::write(p.join("app.js"), "let a = 1;").unwrap();
        std::fs::write(d.join("secret.txt"), "nope").unwrap();
        d
    }

    #[test]
    fn serves_files_with_mime_types() {
        let d = plugins_dir();
        let r = serve(&d, "/com.x.sdPlugin/icon.png");
        assert_eq!((r.status, r.mime.as_str()), (200, "image/png"));
        let r = serve(&d, "/com.x.sdPlugin/app.js");
        assert_eq!(r.mime, "text/javascript");
        let r = serve(&d, "/com.x/icon.png"); // id without .sdPlugin suffix also resolves
        assert_eq!(r.status, 200);
    }

    #[test]
    fn html_gets_the_bootstrap_injected() {
        let d = plugins_dir();
        let r = serve(&d, "/com.x.sdPlugin/pi/index.html");
        assert_eq!(r.mime, "text/html; charset=utf-8");
        let body = String::from_utf8(r.body).unwrap();
        assert!(body.contains("connectElgatoStreamDeckSocket"));
        assert!(body.contains("gd-pi-height"));
        assert!(body.find("<script").unwrap() < body.find("<title>").unwrap(), "bootstrap must run before page scripts");
        assert!(body.contains("PI"));
    }

    #[test]
    fn traversal_and_missing_files_are_404() {
        let d = plugins_dir();
        for p in [
            "/com.x.sdPlugin/../secret.txt",
            "/com.x.sdPlugin/%2e%2e/secret.txt",
            "/com.x.sdPlugin/..%5csecret.txt",
            "/../secret.txt",
            "/secret.txt",
            "/com.x.sdPlugin/missing.png",
            "/com.x.sdPlugin/",
            "",
        ] {
            assert_eq!(serve(&d, p).status, 404, "{p}");
        }
    }

    #[test]
    fn query_and_hash_are_ignored_for_lookup() {
        let d = plugins_dir();
        assert_eq!(serve(&d, "/com.x.sdPlugin/pi/index.html?x=1").status, 200);
    }

    #[test]
    fn launch_url_carries_parameters_in_the_hash() {
        let url = page_url("com.x.sdPlugin", "pi/index.html", &serde_json::json!({ "port": 5, "uuid": "u" }));
        assert!(url.starts_with("http://gdplugin.localhost/com.x.sdPlugin/pi/index.html#gd="));
    }
}
