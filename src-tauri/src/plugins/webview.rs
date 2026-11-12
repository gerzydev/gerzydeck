//! Launchers: HTML plugins run in hidden webview windows; everything else is delegated
//! (exe / Node.js via the process supervisor).

use super::host::{Launcher, LoadedPlugin};
use super::manifest::Runtime;
use super::scheme;
use serde_json::json;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

fn window_label(id: &str) -> String {
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("plugin_{safe}")
}

/// Runs `CodePath` HTML plugins in hidden windows served from `gdplugin://`.
pub struct HtmlLauncher {
    app: AppHandle,
    labels: Vec<String>,
}

impl HtmlLauncher {
    pub fn new(app: AppHandle) -> Self {
        Self { app, labels: vec![] }
    }

    fn start_html(&mut self, plugin: &LoadedPlugin, html: &std::path::Path, port: u16, uuid: &str, info: &str) {
        let rel = match html.strip_prefix(plugin.dir.canonicalize().unwrap_or_else(|_| plugin.dir.clone())) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => {
                log::error!("html plugin {} outside its folder", plugin.id);
                return;
            }
        };
        let info: serde_json::Value = serde_json::from_str(info).unwrap_or(json!({}));
        let url = scheme::page_url(&plugin.id, &rel, &json!({ "port": port, "uuid": uuid, "registerEvent": "registerPlugin", "info": info }));
        let label = window_label(&plugin.id);
        if let Some(w) = self.app.get_webview_window(&label) {
            let _ = w.close();
        }
        let Ok(parsed) = url.parse() else {
            log::error!("bad plugin url {url}");
            return;
        };
        match WebviewWindowBuilder::new(&self.app, &label, WebviewUrl::External(parsed)).visible(false).skip_taskbar(true).title(format!("plugin {}", plugin.id)).build() {
            Ok(_) => {
                log::info!("html plugin {} started", plugin.id);
                self.labels.push(label);
            }
            Err(e) => log::error!("html plugin {} failed: {e}", plugin.id),
        }
    }
}

/// HTML plugins → hidden webviews, others → `procs` (process supervisor).
pub struct CompositeLauncher {
    pub html: HtmlLauncher,
    pub procs: Box<dyn Launcher>,
}

impl Launcher for CompositeLauncher {
    fn start(&mut self, plugin: &LoadedPlugin, port: u16, uuid: &str, info: &str) {
        match plugin.manifest.runtime(&plugin.dir) {
            Ok(Runtime::Html(path)) => self.html.start_html(plugin, &path, port, uuid, info),
            Ok(_) => self.procs.start(plugin, port, uuid, info),
            Err(e) => log::error!("plugin {} cannot start: {e}", plugin.id),
        }
    }

    fn stop(&mut self, id: &str) {
        let label = window_label(id);
        if let Some(pos) = self.html.labels.iter().position(|l| *l == label) {
            self.html.labels.remove(pos);
            if let Some(w) = self.html.app.get_webview_window(&label) {
                let _ = w.close();
            }
        } else {
            self.procs.stop(id);
        }
    }

    fn stop_all(&mut self) {
        for label in self.html.labels.drain(..) {
            if let Some(w) = self.html.app.get_webview_window(&label) {
                let _ = w.close();
            }
        }
        self.procs.stop_all();
    }
}
