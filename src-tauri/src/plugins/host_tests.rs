//! Host actor tests with an in-process fake plugin speaking the SDK protocol.

use super::host::*;
use super::link::{HostCmd, KeyCtx, PluginEvent};
use super::protocol::ActionCtx;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const MANIFEST: &str = r#"{"Name":"Fake","Version":"1.0","Author":"t","Icon":"icon","CodePath":"fake.exe","PropertyInspectorPath":"pi.html",
  "Actions":[{"UUID":"com.fake.act","Name":"Act","Icon":"icon","States":[{"Image":"icon"},{"Image":"icon2"}]}]}"#;

fn plugins_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("gd-host-{}", crate::config::new_id()));
    let p = d.join("com.fake.sdPlugin");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("manifest.json"), MANIFEST).unwrap();
    std::fs::write(p.join("fake.exe"), "x").unwrap();
    std::fs::write(p.join("pi.html"), "<html></html>").unwrap();
    std::fs::write(p.join("icon.png"), "png").unwrap();
    d
}

/// Launcher that runs a fake plugin as a tokio task: it registers, records every message,
/// answers keyDown with showOk, and sends whatever the test pushes into `say`.
#[derive(Clone, Default)]
struct FakeLauncher {
    got: Arc<Mutex<Vec<Value>>>,
    launches: Arc<Mutex<u32>>,
    /// test → fake plugin: raw messages the plugin should send to the host
    say: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>>>,
    stopped: Arc<Mutex<Vec<String>>>,
}

impl Launcher for FakeLauncher {
    fn start(&mut self, _plugin: &LoadedPlugin, port: u16, uuid: &str, _info: &str) {
        *self.launches.lock().unwrap() += 1;
        let got = self.got.clone();
        let uuid = uuid.to_string();
        let (say_tx, mut say_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        *self.say.lock().unwrap() = Some(say_tx);
        tokio::spawn(async move {
            let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}")).await.unwrap();
            let (mut sink, mut source) = ws.split();
            sink.send(Message::text(json!({ "event": "registerPlugin", "uuid": uuid }).to_string())).await.unwrap();
            let sink = Arc::new(tokio::sync::Mutex::new(sink));
            let s2 = sink.clone();
            tokio::spawn(async move {
                while let Some(m) = say_rx.recv().await {
                    let _ = s2.lock().await.send(Message::text(m)).await;
                }
            });
            while let Some(Ok(Message::Text(t))) = source.next().await {
                let v: Value = serde_json::from_str(t.as_str()).unwrap();
                got.lock().unwrap().push(v.clone());
                if v["event"] == "keyDown" {
                    let _ = sink.lock().await.send(Message::text(json!({ "event": "showOk", "context": v["context"] }).to_string())).await;
                }
            }
        });
    }
    fn stop(&mut self, id: &str) {
        self.stopped.lock().unwrap().push(id.to_string());
    }
    fn stop_all(&mut self) {}
}

fn decode(src: &str, _dir: Option<&Path>) -> Option<image::RgbaImage> {
    (src.starts_with("data:") || src.ends_with(".png")).then(|| image::RgbaImage::new(2, 2))
}

struct H {
    tx: std::sync::mpsc::Sender<HostCmd>,
    events: Receiver<PluginEvent>,
    fake: FakeLauncher,
}

fn ctx(settings: Value) -> KeyCtx {
    KeyCtx {
        plugin: "com.fake".into(),
        ctx: ActionCtx { action: "com.fake.act".into(), context: "A:3".into(), device: "D6-main".into(), key: 3, settings, state: 0 },
    }
}

fn start() -> H {
    let dir = plugins_dir();
    let (etx, erx) = channel();
    let fake = FakeLauncher::default();
    let tx = spawn_host(HostDeps {
        plugins_dir: dir.clone(),
        settings_file: dir.join("plugin-settings.json"),
        events: Box::new(move |e| {
            let _ = etx.send(e);
        }),
        launcher: Box::new(fake.clone()),
        decode_image: decode,
        open_url: |_| {},
        list_processes: Vec::new,
    });
    H { tx, events: erx, fake }
}

impl H {
    fn wait_got(&self, pred: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..200 {
            if let Some(v) = self.fake.got.lock().unwrap().iter().find(|v| pred(v)) {
                return v.clone();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("plugin never got it; got {:?}", self.fake.got.lock().unwrap());
    }
    fn wait_event(&self, pred: impl Fn(&PluginEvent) -> bool) -> PluginEvent {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.events.recv_timeout(left) {
                Ok(e) if pred(&e) => return e,
                Ok(_) => continue,
                Err(_) => panic!("no matching event"),
            }
        }
    }
}

#[test]
fn launches_installed_plugins_and_queues_until_registered() {
    let h = start();
    h.tx.send(HostCmd::DeviceConnected).unwrap();
    h.tx.send(HostCmd::WillAppear(ctx(json!({"a": 1})))).unwrap();
    let appear = h.wait_got(|v| v["event"] == "willAppear");
    assert_eq!(appear["context"], "A:3");
    assert_eq!(appear["payload"]["settings"]["a"], 1);
    assert_eq!(appear["payload"]["coordinates"], json!({"column": 2, "row": 0}));
    h.wait_got(|v| v["event"] == "deviceDidConnect");
    assert_eq!(*h.fake.launches.lock().unwrap(), 1);
}

#[test]
fn will_appear_sends_the_manifest_state_image_as_default() {
    let h = start();
    h.tx.send(HostCmd::WillAppear(ctx(json!({})))).unwrap();
    h.wait_event(|e| matches!(e, PluginEvent::SetImage { context, image: Some(_), state: None } if context == "A:3"));
}

#[test]
fn key_down_round_trip_reaches_the_engine() {
    let h = start();
    h.tx.send(HostCmd::WillAppear(ctx(json!({})))).unwrap();
    h.tx.send(HostCmd::KeyDown(ctx(json!({})))).unwrap();
    h.wait_got(|v| v["event"] == "keyDown");
    h.wait_event(|e| matches!(e, PluginEvent::ShowOk { context } if context == "A:3"));
}

#[test]
fn plugin_messages_become_engine_events() {
    let h = start();
    h.tx.send(HostCmd::WillAppear(ctx(json!({})))).unwrap();
    h.wait_got(|v| v["event"] == "willAppear");
    let say = |m: Value| h.fake.say.lock().unwrap().as_ref().unwrap().send(m.to_string()).unwrap();
    say(json!({ "event": "setTitle", "context": "A:3", "payload": { "title": "42%" } }));
    h.wait_event(|e| matches!(e, PluginEvent::SetTitle { title: Some(t), .. } if t == "42%"));
    say(json!({ "event": "setImage", "context": "A:3", "payload": { "image": "data:image/png;base64,AA" } }));
    h.wait_event(|e| matches!(e, PluginEvent::SetImage { image: Some(_), .. }));
    say(json!({ "event": "setSettings", "context": "A:3", "payload": { "scene": "Game" } }));
    h.wait_event(|e| matches!(e, PluginEvent::SetSettings { settings, .. } if settings["scene"] == "Game"));
    say(json!({ "event": "setState", "context": "A:3", "payload": { "state": 1 } }));
    h.wait_event(|e| matches!(e, PluginEvent::SetState { state: 1, .. }));
    say(json!({ "event": "getSettings", "context": "A:3" }));
    let got = h.wait_got(|v| v["event"] == "didReceiveSettings");
    assert_eq!(got["payload"]["settings"]["scene"], "Game");
    say(json!({ "event": "setGlobalSettings", "context": "x", "payload": { "token": "t" } }));
    say(json!({ "event": "getGlobalSettings", "context": "x" }));
    let g = h.wait_got(|v| v["event"] == "didReceiveGlobalSettings");
    assert_eq!(g["payload"]["settings"]["token"], "t");
    say(json!({ "event": "switchToProfile", "context": "x", "device": "D6-main", "payload": { "profile": "Gaming" } }));
    h.wait_event(|e| matches!(e, PluginEvent::SwitchProfile { profile } if profile == "Gaming"));
}

#[test]
fn property_inspector_open_issues_a_token_and_url() {
    let h = start();
    h.tx.send(HostCmd::WillAppear(ctx(json!({"a": 1})))).unwrap();
    let (r, rx) = channel();
    h.tx.send(HostCmd::PiOpen { key: ctx(json!({"a": 1})), reply: r }).unwrap();
    let pi = rx.recv_timeout(Duration::from_secs(2)).unwrap().expect("pi info");
    assert!(pi.url.starts_with("http://gdplugin.localhost/com.fake/pi.html#gd="));
    assert_eq!(pi.uuid.len(), 32);
    assert_eq!(pi.action_info["context"], "A:3");
    assert_eq!(pi.action_info["payload"]["settings"]["a"], 1);
    h.wait_got(|v| v["event"] == "propertyInspectorDidAppear");
}

#[test]
fn global_settings_persist_across_hosts() {
    let dir = plugins_dir();
    let file = dir.join("plugin-settings.json");
    std::fs::write(&file, r#"{"com.fake": {"token": "saved"}}"#).unwrap();
    let store = GlobalSettings::load(&file);
    assert_eq!(store.get("com.fake")["token"], "saved");
    assert_eq!(store.get("other"), json!({}));
}

#[test]
fn list_reports_plugins_with_keypad_actions_and_status() {
    let h = start();
    h.wait_event(|e| matches!(e, PluginEvent::Status { status: super::link::PluginStatus::Running, .. }));
    let (r, rx) = channel();
    h.tx.send(HostCmd::ListPlugins(r)).unwrap();
    let list = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(list.len(), 1);
    let p = &list[0];
    assert_eq!(p.id, "com.fake");
    assert_eq!(p.name, "Fake");
    assert_eq!(p.status, super::link::PluginStatus::Running);
    assert_eq!(p.actions[0].uuid, "com.fake.act");
    assert!(p.icon.starts_with("http://gdplugin.localhost/com.fake/icon"), "{}", p.icon);
}

#[test]
fn installing_a_missing_plugin_replays_its_actions() {
    let h = start();
    let mut missing = ctx(json!({ "x": 1 }));
    missing.plugin = "com.later".into();
    missing.ctx.action = "com.later.act".into();
    missing.ctx.context = "A:5".into();
    h.tx.send(HostCmd::WillAppear(missing)).unwrap();
    h.wait_event(|e| matches!(e, PluginEvent::Status { plugin, status: super::link::PluginStatus::NotInstalled } if plugin == "com.later"));
    // "install" it: copy the fake plugin under the new id and ask the host to (re)load it
    let (r, rx) = channel();
    h.tx.send(HostCmd::ListPlugins(r)).unwrap();
    let dir = std::path::PathBuf::from(&rx.recv().unwrap()[0].dir).parent().unwrap().join("com.later.sdPlugin");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), MANIFEST.replace("com.fake.act", "com.later.act")).unwrap();
    std::fs::write(dir.join("fake.exe"), "x").unwrap();
    h.tx.send(HostCmd::Reload { id: "com.later".into() }).unwrap();
    let appear = h.wait_got(|v| v["event"] == "willAppear" && v["context"] == "A:5");
    assert_eq!(appear["action"], "com.later.act");
    assert_eq!(*h.fake.launches.lock().unwrap(), 2);
}

#[test]
fn removing_a_plugin_stops_it_and_marks_actions_missing() {
    let h = start();
    h.tx.send(HostCmd::WillAppear(ctx(json!({})))).unwrap();
    h.tx.send(HostCmd::Remove { id: "com.fake".into() }).unwrap();
    h.wait_event(|e| matches!(e, PluginEvent::Status { status: super::link::PluginStatus::NotInstalled, .. }));
    assert!(h.fake.stopped.lock().unwrap().contains(&"com.fake".to_string()));
}
