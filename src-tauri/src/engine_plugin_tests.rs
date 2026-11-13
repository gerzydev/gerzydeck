//! Engine ↔ plugin host behaviour, with a fake host channel.

use crate::config::{new_id, normalize_config, Action};
use crate::device::{DeviceCmd, DeviceEvent};
use crate::engine::*;
use crate::keyboard::Keyboard;
use crate::plugins::link::{HostCmd, PluginEvent, PluginStatus};
use crate::render::KeySpec;
use serde_json::json;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default, Clone)]
struct Rec(Arc<Mutex<Vec<String>>>);
struct Kb(Rec);
impl Keyboard for Kb {
    fn down(&self, c: &str) {
        self.0 .0.lock().unwrap().push(format!("down {c}"))
    }
    fn up(&self, c: &str) {
        self.0 .0.lock().unwrap().push(format!("up {c}"))
    }
    fn tap_vk(&self, _: u16) {}
    fn type_text(&self, _: &str) {}
}
struct Sink;
impl EventSink for Sink {
    fn state(&self, _: &State) {}
    fn config(&self, _: &Config) {}
    fn key(&self, _: u8, _: bool) {}
    fn raw(&self, _: u8, _: bool) {}
}
fn render(spec: &KeySpec) -> Vec<u8> {
    format!("{}|{}", spec.label, spec.image.is_some()).into_bytes()
}
fn render_plugin(tile: &PluginTile) -> Vec<u8> {
    let style = if tile.align.is_some() || tile.size.is_some() { format!("|{:?}|{:?}|{}", tile.align, tile.size, tile.show_title) } else { String::new() };
    format!("P|{:?}|{:?}|{:?}|{}|{}{style}", tile.label, tile.title, tile.overlay, tile.image.is_some(), tile.missing).into_bytes()
}

fn render_usage(t: &UsageTile) -> Vec<u8> {
    format!("U|{}|{:?}|{}|{:?}", t.name, t.percent, t.reset.is_some(), t.error).into_bytes()
}

fn cfg() -> Config {
    normalize_config(&json!({
        "defaultProfileId": "A", "activeProfileId": "A",
        "profiles": [
            { "id": "A", "name": "A", "keys": {
                "1": { "action": { "type": "keys", "codes": ["KeyQ"] } },
                "3": { "action": { "type": "plugin", "plugin": "com.x", "action": "com.x.act", "settings": { "n": 1 } } },
                "4": { "action": { "type": "profile", "profileId": "B" } },
                "6": { "action": { "type": "usage", "provider": "claude", "window": "session" } }
            } },
            { "id": "B", "name": "B", "keys": {} }
        ]
    }))
}

struct P {
    tx: Sender<Msg>,
    dev: Receiver<DeviceCmd>,
    host: Receiver<HostCmd>,
    kb: Rec,
    store: Store,
    wake: Receiver<()>,
}

impl P {
    fn new() -> P {
        let store = Store::new(std::env::temp_dir().join(format!("deck-plugin-{}", new_id())));
        store.save(&cfg()).unwrap();
        let (htx, hrx) = channel();
        let (dtx, drx) = channel();
        let kb = Rec::default();
        let (wtx, wrx) = channel();
        let (tx, _h) = spawn_engine(EngineDeps {
            store: store.clone(),
            device_tx: dtx,
            keyboard: Arc::new(Kb(kb.clone())),
            launch: Arc::new(|_: &str, _: &[String]| {}),
            system: Arc::new(|_: &str| {}),
            sink: Box::new(Sink),
            render,
            render_plugin,
            render_usage,
            usage_wake: Some(wtx),
            sound_tx: None,
            host_tx: Some(htx),
        });
        let p = P { tx, dev: drx, host: hrx, kb, store, wake: wrx };
        p.sync();
        p
    }
    fn sync(&self) -> State {
        let (r, rx) = channel();
        self.tx.send(Msg::GetState(r)).unwrap();
        rx.recv_timeout(Duration::from_secs(2)).unwrap()
    }
    fn host_cmds(&self) -> Vec<String> {
        self.sync();
        let mut out = vec![];
        while let Ok(c) = self.host.try_recv() {
            out.push(match c {
                HostCmd::DeviceConnected => "connected".to_string(),
                HostCmd::DeviceDisconnected => "disconnected".into(),
                HostCmd::WillAppear(k) => format!("appear {} {} {}", k.plugin, k.ctx.context, k.ctx.settings),
                HostCmd::WillDisappear(k) => format!("disappear {}", k.ctx.context),
                HostCmd::KeyDown(k) => format!("down {}", k.ctx.context),
                HostCmd::KeyUp(k) => format!("up {}", k.ctx.context),
                HostCmd::TitleParams { key, title } => format!("title {} {title}", key.ctx.context),
                other => format!("{other:?}"),
            });
        }
        out
    }
    fn images(&self) -> Vec<(u8, String)> {
        self.sync();
        let mut out = vec![];
        while let Ok(c) = self.dev.try_recv() {
            if let DeviceCmd::SetImages(v) = c {
                out.extend(v.into_iter().map(|(k, b)| (k, String::from_utf8(b).unwrap())));
            }
        }
        out
    }
    fn plugin(&self, e: PluginEvent) {
        self.tx.send(Msg::Plugin(e)).unwrap();
    }
    fn connect(&self) {
        self.tx.send(Msg::Device(DeviceEvent::Connected)).unwrap();
    }
    fn key(&self, key: u8, pressed: bool) {
        self.tx.send(Msg::Device(DeviceEvent::Key { key, pressed })).unwrap();
    }
}

const APPEAR_3: &str = r#"appear com.x A:3 {"n":1}"#;

#[test]
fn connect_announces_device_then_visible_plugin_actions() {
    let p = P::new();
    p.connect();
    let cmds = p.host_cmds();
    assert_eq!(cmds.first().map(String::as_str), Some("connected"));
    assert!(cmds.iter().any(|c| c == APPEAR_3), "{cmds:?}");
    let key3 = p.images().into_iter().find(|(k, _)| *k == 3).unwrap().1;
    assert!(key3.starts_with("P|"), "{key3}");
}

#[test]
fn profile_switch_sends_disappear_and_back_sends_appear() {
    let p = P::new();
    p.connect();
    p.host_cmds();
    p.key(4, true);
    assert_eq!(p.host_cmds(), vec!["disappear A:3"]);
    p.tx.send(Msg::SwitchProfile("__back".into())).unwrap();
    assert_eq!(p.host_cmds(), vec![APPEAR_3]);
}

#[test]
fn disconnect_sends_disappear_and_device_disconnected() {
    let p = P::new();
    p.connect();
    p.host_cmds();
    p.tx.send(Msg::Device(DeviceEvent::Disconnected)).unwrap();
    assert_eq!(p.host_cmds(), vec!["disappear A:3", "disconnected"]);
}

#[test]
fn plugin_key_presses_go_to_the_host_not_the_keyboard() {
    let p = P::new();
    p.connect();
    p.host_cmds();
    p.key(3, true);
    p.key(3, false);
    assert_eq!(p.host_cmds(), vec!["down A:3", "up A:3"]);
    assert!(p.kb.0.lock().unwrap().is_empty());
}

#[test]
fn plugin_settings_survive_an_editor_save() {
    let p = P::new();
    let (r, rx) = channel();
    p.tx.send(Msg::GetConfig(r)).unwrap();
    let editor_copy = rx.recv().unwrap(); // editor loaded config before the plugin wrote
    p.plugin(PluginEvent::SetSettings { context: "A:3".into(), settings: json!({ "n": 2 }) });
    p.plugin(PluginEvent::SetState { context: "A:3".into(), state: 1 });
    p.sync();
    let (r, rx) = channel();
    p.tx.send(Msg::SetConfig(serde_json::to_value(&editor_copy).unwrap(), r)).unwrap();
    rx.recv().unwrap().unwrap();
    let a = p.store.load().profiles[0].key(3).action.clone();
    assert_eq!(a, Action::Plugin { plugin: "com.x".into(), action: "com.x.act".into(), settings: json!({ "n": 2 }), state: 1 });
}

#[test]
fn image_flood_is_throttled_but_builtin_keys_stay_instant() {
    let p = P::new();
    p.connect();
    p.images();
    std::thread::sleep(Duration::from_millis(120));
    let img = Arc::new(image::RgbaImage::new(4, 4));
    let t0 = Instant::now();
    for i in 0..50 {
        p.plugin(PluginEvent::SetTitle { context: "A:3".into(), title: Some(format!("{i}")), state: None });
        p.plugin(PluginEvent::SetImage { skin: None, context: "A:3".into(), image: Some(img.clone()), state: None });
    }
    p.key(1, true);
    p.sync();
    assert_eq!(p.kb.0.lock().unwrap().as_slice(), ["down KeyQ"]);
    std::thread::sleep(Duration::from_millis(150).saturating_sub(t0.elapsed()));
    let key3: Vec<String> = p.images().into_iter().filter(|(k, _)| *k == 3).map(|(_, s)| s).collect();
    assert!(!key3.is_empty() && key3.len() <= 2, "{} frames: {key3:?}", key3.len());
    std::thread::sleep(Duration::from_millis(250));
    let mut all: Vec<String> = key3;
    all.extend(p.images().into_iter().filter(|(k, _)| *k == 3).map(|(_, s)| s));
    assert_eq!(all.last().map(String::as_str), Some(r#"P|None|Some("49")|None|true|false"#), "latest frame wins");
}

#[test]
fn show_ok_overlays_and_missing_plugin_renders() {
    let p = P::new();
    p.connect();
    p.images();
    std::thread::sleep(Duration::from_millis(120));
    p.plugin(PluginEvent::ShowOk { context: "A:3".into() });
    std::thread::sleep(Duration::from_millis(150));
    assert!(p.images().iter().any(|(k, s)| *k == 3 && s.contains("|Ok|")));
    p.plugin(PluginEvent::Status { plugin: "com.x".into(), status: PluginStatus::NotInstalled });
    std::thread::sleep(Duration::from_millis(150));
    assert!(p.images().iter().any(|(k, s)| *k == 3 && s.ends_with("|true")));
}

#[test]
fn user_label_becomes_title_params() {
    let p = P::new();
    p.connect();
    p.host_cmds();
    let (r, rx) = channel();
    p.tx.send(Msg::GetConfig(r)).unwrap();
    let mut c = rx.recv().unwrap();
    c.profiles[0].keys.get_mut("3").unwrap().label = "Rec".into();
    let (r, rx) = channel();
    p.tx.send(Msg::SetConfig(serde_json::to_value(&c).unwrap(), r)).unwrap();
    rx.recv().unwrap().unwrap();
    assert!(p.host_cmds().contains(&"title A:3 Rec".to_string()));
}

#[test]
fn unknown_contexts_are_ignored() {
    let p = P::new();
    p.plugin(PluginEvent::SetSettings { context: "nope".into(), settings: json!({}) });
    p.plugin(PluginEvent::SetState { context: "A:1".into(), state: 1 }); // not a plugin key
    p.plugin(PluginEvent::SetImage { skin: None, context: "Z:3".into(), image: None, state: None });
    p.sync();
    assert_eq!(p.store.load().profiles[0].key(1).action, Action::Keys { codes: vec!["KeyQ".into()] });
}

#[test]
fn user_label_and_live_plugin_title_are_both_rendered() {
    let p = P::new();
    p.connect();
    p.images();
    let (r, rx) = channel();
    p.tx.send(Msg::GetConfig(r)).unwrap();
    let mut c = rx.recv().unwrap();
    c.profiles[0].keys.get_mut("3").unwrap().label = "CPU".into();
    let (r, rx) = channel();
    p.tx.send(Msg::SetConfig(serde_json::to_value(&c).unwrap(), r)).unwrap();
    rx.recv().unwrap().unwrap();
    std::thread::sleep(Duration::from_millis(120));
    p.plugin(PluginEvent::SetTitle { context: "A:3".into(), title: Some("8%".into()), state: None });
    std::thread::sleep(Duration::from_millis(150));
    let last = p.images().into_iter().filter(|(k, _)| *k == 3).last().map(|(_, s)| s);
    assert_eq!(last.as_deref(), Some(r#"P|Some("CPU")|Some("8%")|None|true|false"#));
}

#[test]
fn manifest_title_style_applies_and_editor_alignment_wins() {
    use crate::plugins::link::TitleStyle;
    let p = P::new();
    p.connect();
    p.images();
    std::thread::sleep(Duration::from_millis(120));
    p.plugin(PluginEvent::TitleStyle { context: "A:3".into(), styles: vec![TitleStyle { align: Some("middle".into()), size: Some(17), show: None }] });
    std::thread::sleep(Duration::from_millis(150));
    let last = p.images().into_iter().filter(|(k, _)| *k == 3).last().map(|(_, s)| s).unwrap();
    assert!(last.ends_with(r#"|Some("middle")|Some(17)|true"#), "{last}");
    let (r, rx) = channel();
    p.tx.send(Msg::GetConfig(r)).unwrap();
    let mut c = rx.recv().unwrap();
    c.profiles[0].keys.get_mut("3").unwrap().title_align = Some("top".into());
    let (r, rx) = channel();
    p.tx.send(Msg::SetConfig(serde_json::to_value(&c).unwrap(), r)).unwrap();
    rx.recv().unwrap().unwrap();
    std::thread::sleep(Duration::from_millis(150));
    let last = p.images().into_iter().filter(|(k, _)| *k == 3).last().map(|(_, s)| s).unwrap();
    assert!(last.ends_with(r#"|Some("top")|Some(17)|true"#), "{last}");
}

#[test]
fn swapping_keys_moves_everything_including_plugin_settings() {
    let p = P::new();
    p.connect();
    p.host_cmds();
    p.plugin(PluginEvent::SetSettings { context: "A:3".into(), settings: json!({ "n": 7 }) });
    let (r, rx) = channel();
    p.tx.send(Msg::SwapKeys { profile_id: "A".into(), a: 3, b: 5, reply: r }).unwrap();
    let cfg = rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    assert_eq!(cfg.profiles[0].key(3).action, Action::None);
    assert_eq!(
        cfg.profiles[0].key(5).action,
        Action::Plugin { plugin: "com.x".into(), action: "com.x.act".into(), settings: json!({ "n": 7 }), state: 0 }
    );
    assert_eq!(p.store.load().profiles[0].key(5).action, cfg.profiles[0].key(5).action);
    let cmds = p.host_cmds();
    assert!(cmds.contains(&"disappear A:3".to_string()), "{cmds:?}");
    assert!(cmds.contains(&r#"appear com.x A:5 {"n":7}"#.to_string()), "{cmds:?}");
    let (r, rx) = channel();
    p.tx.send(Msg::SwapKeys { profile_id: "A".into(), a: 3, b: 99, reply: r }).unwrap();
    assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap().is_err());
}

#[test]
fn custom_image_on_a_plugin_key_keeps_the_live_title() {
    let p = P::new();
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 255, 0, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let id = p.store.save_image(&png).unwrap();
    p.connect();
    p.images();
    let (r, rx) = channel();
    p.tx.send(Msg::GetConfig(r)).unwrap();
    let mut c = rx.recv().unwrap();
    c.profiles[0].keys.get_mut("3").unwrap().image = Some(id);
    let (r, rx) = channel();
    p.tx.send(Msg::SetConfig(serde_json::to_value(&c).unwrap(), r)).unwrap();
    rx.recv().unwrap().unwrap();
    std::thread::sleep(Duration::from_millis(120));
    p.plugin(PluginEvent::SetTitle { context: "A:3".into(), title: Some("8%".into()), state: None });
    std::thread::sleep(Duration::from_millis(150));
    let last = p.images().into_iter().filter(|(k, _)| *k == 3).last().map(|(_, s)| s);
    assert_eq!(last.as_deref(), Some(r#"P|None|Some("8%")|None|true|false"#));
}

#[test]
fn usage_key_shows_the_window_and_refreshes_on_press() {
    use crate::usage::{Usage, Window};
    let p = P::new();
    p.connect();
    let first = p.images().into_iter().find(|(k, _)| *k == 6).map(|(_, s)| s);
    assert_eq!(first.as_deref(), Some("U|Claude 5h|None|false|None"));
    let (r, rx) = channel();
    p.tx.send(Msg::UsageWanted(r)).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), vec!["claude".to_string()]);
    let at = crate::usage::now_s() + 3600;
    p.tx.send(Msg::Usage { provider: "claude".into(), result: Ok(Usage { session: Some(Window { percent: 21.0, resets_at: at }), weekly: None }) }).unwrap();
    let k6 = p.images().into_iter().filter(|(k, _)| *k == 6).last().map(|(_, s)| s);
    assert_eq!(k6.as_deref(), Some("U|Claude 5h|Some(21.0)|true|None"));
    p.tx.send(Msg::Usage { provider: "claude".into(), result: Err("Přihlášení vypršelo".into()) }).unwrap();
    let k6 = p.images().into_iter().filter(|(k, _)| *k == 6).last().map(|(_, s)| s);
    assert_eq!(k6.as_deref(), Some(r#"U|Claude 5h|None|false|Some("Přihlášení vypršelo")"#));
    while p.wake.try_recv().is_ok() {}
    p.key(6, true);
    p.key(6, false);
    p.sync();
    assert!(p.wake.try_recv().is_ok(), "a press asks for fresh numbers");
}
