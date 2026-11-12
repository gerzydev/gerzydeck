//! Messages between the engine actor and the plugin host actor.

use crate::plugins::protocol::ActionCtx;
use image::RgbaImage;
use serde_json::Value;
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// An action instance plus the plugin it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyCtx {
    pub plugin: String,
    pub ctx: ActionCtx,
}

/// What the editor needs to open a property inspector.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiInfo {
    /// e.g. `gdplugin://localhost/<plugin>/<pi path>`
    pub url: String,
    pub port: u16,
    pub uuid: String,
    pub info: Value,
    pub action_info: Value,
}

/// Engine → host.
#[derive(Debug)]
pub enum HostCmd {
    DeviceConnected,
    DeviceDisconnected,
    WillAppear(KeyCtx),
    WillDisappear(KeyCtx),
    KeyDown(KeyCtx),
    KeyUp(KeyCtx),
    /// User title (editor label) for an action, sent as titleParametersDidChange.
    TitleParams { key: KeyCtx, title: String },
    PiOpen { key: KeyCtx, reply: Sender<Option<PiInfo>> },
    PiClose { context: String },
    Shutdown,
}

/// Status of an installed plugin, shown in the editor.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PluginStatus {
    Running,
    Starting,
    Crashed { restarts: u32 },
    MissingNode,
    NotInstalled,
}

/// Host → engine (wrapped in `engine::Msg::Plugin`).
#[derive(Debug, Clone)]
pub enum PluginEvent {
    SetImage { context: String, image: Option<Arc<RgbaImage>>, state: Option<u8> },
    SetTitle { context: String, title: Option<String>, state: Option<u8> },
    SetState { context: String, state: u8 },
    SetSettings { context: String, settings: Value },
    ShowOk { context: String },
    ShowAlert { context: String },
    SwitchProfile { profile: String },
    Status { plugin: String, status: PluginStatus },
}

/// Context string of a key in a profile; stable across restarts so plugins can cache by it.
pub fn context_for(profile_id: &str, key: u8) -> String {
    format!("{profile_id}:{key}")
}

pub fn parse_context(context: &str) -> Option<(&str, u8)> {
    let (p, k) = context.rsplit_once(':')?;
    let k: u8 = k.parse().ok()?;
    (1..=15).contains(&k).then_some((p, k))
}
