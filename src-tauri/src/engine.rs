//! Engine actor: owns config + active profile, renders the deck, routes presses to actions.

pub use crate::config::Config;
use crate::config::{decide_profile, normalize_config};
use crate::device::{DeviceCmd, DeviceEvent};
use crate::keyboard::Keyboard;
pub use crate::os::Foreground;
use crate::render::{normalize_upload, parse_hex, KeySpec};
use crate::runner::{ActionRunner, CommandFn, LaunchFn};
pub use crate::store::Store;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub trait EventSink: Send + 'static {
    fn state(&self, state: &State);
    fn config(&self, config: &Config);
    fn key(&self, key: u8, pressed: bool);
    fn raw(&self, device_id: u8, pressed: bool);
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub connected: bool,
    pub active_profile_id: String,
    pub manual_profile_id: String,
    pub active_profile_name: String,
    pub foreground_exe: Option<String>,
    pub foreground_elevated: bool,
    pub conflicts: Vec<String>,
    pub calibrating: bool,
}

pub enum Msg {
    Device(DeviceEvent),
    Foreground(Foreground),
    Conflicts(Vec<String>),
    GetConfig(Sender<Config>),
    GetState(Sender<State>),
    SetConfig(serde_json::Value, Sender<Result<Config, String>>),
    SetActiveProfile(String),
    SwitchProfile(String),
    SetCalibrating(bool),
    ImportImage(Vec<u8>, Sender<Result<String, String>>),
    Shutdown,
}

pub struct EngineDeps {
    pub store: Store,
    pub device_tx: Sender<DeviceCmd>,
    pub keyboard: Arc<dyn Keyboard>,
    pub launch: LaunchFn,
    pub system: CommandFn,
    pub sink: Box<dyn EventSink>,
    pub render: fn(&KeySpec) -> Vec<u8>,
}

fn key_map_array(m: &Option<Vec<u8>>) -> Option<[u8; 15]> {
    m.as_ref().and_then(|v| v.as_slice().try_into().ok())
}

struct Engine {
    config: Config,
    state: State,
    previous_profile: Option<String>,
    rendered: HashMap<u8, String>,
    runner: ActionRunner,
    /// profile switches requested by deck actions, applied right after the press
    pending_switches: Arc<Mutex<Vec<String>>>,
    store: Store,
    device_tx: Sender<DeviceCmd>,
    sink: Box<dyn EventSink>,
    render: fn(&KeySpec) -> Vec<u8>,
}

impl Engine {
    fn new(d: EngineDeps) -> Self {
        let config = d.store.load();
        let pending: Arc<Mutex<Vec<String>>> = Arc::default();
        let p = pending.clone();
        let runner = ActionRunner::new(d.keyboard, d.launch, d.system, Arc::new(move |id: &str| p.lock().unwrap().push(id.to_string())));
        let _ = d.device_tx.send(DeviceCmd::SetKeyMap(key_map_array(&config.key_map)));
        let _ = d.device_tx.send(DeviceCmd::SetBrightness(config.brightness));
        let mut e = Engine {
            state: State {
                active_profile_id: config.active_profile_id.clone(),
                manual_profile_id: config.active_profile_id.clone(),
                ..State::default()
            },
            config,
            previous_profile: None,
            rendered: HashMap::new(),
            runner,
            pending_switches: pending,
            store: d.store,
            device_tx: d.device_tx,
            sink: d.sink,
            render: d.render,
        };
        e.refresh_name();
        e
    }

    fn active(&self) -> &crate::config::Profile {
        self.config.profiles.iter().find(|p| p.id == self.state.active_profile_id).unwrap_or(&self.config.profiles[0])
    }

    fn refresh_name(&mut self) {
        self.state.active_profile_name = self.active().name.clone();
    }

    fn emit_state(&mut self) {
        self.refresh_name();
        self.sink.state(&self.state);
    }

    fn exists(&self, id: &str) -> bool {
        self.config.profiles.iter().any(|p| p.id == id)
    }

    fn activate(&mut self, id: String) {
        self.previous_profile = Some(std::mem::replace(&mut self.state.active_profile_id, id));
        self.emit_state();
        self.render_active();
    }

    /// Manual switch (editor, deck profile action).
    fn set_active(&mut self, id: String) {
        if !self.exists(&id) {
            return;
        }
        self.state.manual_profile_id = id.clone();
        if self.config.active_profile_id != id {
            self.config.active_profile_id = id.clone();
            if let Err(e) = self.store.save(&self.config) {
                log::error!("saving config failed: {e}");
            }
        }
        if id != self.state.active_profile_id {
            self.activate(id);
        }
    }

    fn switch(&mut self, id: String) {
        let target = if id == "__back" { self.previous_profile.clone() } else { Some(id) };
        if let Some(t) = target {
            self.set_active(t);
        }
    }

    fn render_active(&mut self) {
        if !self.state.connected {
            return;
        }
        let profile = self.active().clone();
        let mut images = Vec::new();
        for k in 1..=15u8 {
            let cfg = profile.key(k);
            let sig = format!("{:?}|{}|{}", cfg.image, cfg.label, cfg.bg);
            if self.rendered.get(&k) == Some(&sig) {
                continue;
            }
            let spec = KeySpec {
                image: cfg.image.as_deref().and_then(|id| self.store.read_image(id)),
                label: cfg.label.clone(),
                bg: parse_hex(&cfg.bg),
            };
            images.push((k, (self.render)(&spec)));
            self.rendered.insert(k, sig);
        }
        if !images.is_empty() {
            let _ = self.device_tx.send(DeviceCmd::SetImages(images));
        }
    }

    fn on_key(&mut self, key: u8, pressed: bool) {
        self.sink.key(key, pressed);
        if self.state.calibrating {
            return;
        }
        if pressed {
            let action = self.active().key(key).action.clone();
            let _ = self.runner.press(key, &action);
            let switches: Vec<String> = std::mem::take(&mut *self.pending_switches.lock().unwrap());
            for id in switches {
                self.switch(id);
            }
        } else {
            self.runner.release(key);
        }
    }

    fn set_config(&mut self, raw: &serde_json::Value) -> Result<Config, String> {
        let cfg = normalize_config(raw);
        self.store.save(&cfg).map_err(|e| format!("Uložení selhalo: {e}"))?;
        self.store.prune_images(&cfg);
        if cfg.brightness != self.config.brightness {
            let _ = self.device_tx.send(DeviceCmd::SetBrightness(cfg.brightness));
        }
        if cfg.key_map != self.config.key_map {
            let _ = self.device_tx.send(DeviceCmd::SetKeyMap(key_map_array(&cfg.key_map)));
        }
        self.config = cfg;
        if !self.exists(&self.state.manual_profile_id) {
            self.state.manual_profile_id = self.config.default_profile_id.clone();
        }
        if self.previous_profile.as_deref().is_some_and(|p| !self.exists(p)) {
            self.previous_profile = None;
        }
        self.sink.config(&self.config);
        if self.exists(&self.state.active_profile_id) {
            self.emit_state();
            self.render_active();
        } else {
            let target = decide_profile(&self.config, self.state.foreground_exe.as_deref(), &self.state.manual_profile_id);
            self.activate(target);
        }
        Ok(self.config.clone())
    }

    /// Returns false on shutdown.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Device(DeviceEvent::Connected) => {
                self.state.connected = true;
                self.rendered.clear();
                self.emit_state();
                self.render_active();
            }
            Msg::Device(DeviceEvent::Disconnected) => {
                self.runner.release_all();
                self.state.connected = false;
                self.emit_state();
            }
            Msg::Device(DeviceEvent::Raw { device_id, pressed }) => self.sink.raw(device_id, pressed),
            Msg::Device(DeviceEvent::Key { key, pressed }) => self.on_key(key, pressed),
            Msg::Foreground(fg) => {
                self.state.foreground_exe = fg.exe;
                self.state.foreground_elevated = fg.elevated;
                let target = decide_profile(&self.config, self.state.foreground_exe.as_deref(), &self.state.manual_profile_id);
                if target != self.state.active_profile_id {
                    self.activate(target);
                } else {
                    self.emit_state();
                }
            }
            Msg::Conflicts(c) => {
                if c != self.state.conflicts {
                    self.state.conflicts = c;
                    self.emit_state();
                }
            }
            Msg::GetConfig(r) => {
                let _ = r.send(self.config.clone());
            }
            Msg::GetState(r) => {
                self.refresh_name();
                let _ = r.send(self.state.clone());
            }
            Msg::SetConfig(raw, r) => {
                let res = self.set_config(&raw);
                let _ = r.send(res);
            }
            Msg::SetActiveProfile(id) => self.set_active(id),
            Msg::SwitchProfile(id) => self.switch(id),
            Msg::SetCalibrating(on) => {
                self.state.calibrating = on;
                if on {
                    self.runner.release_all();
                }
                self.emit_state();
            }
            Msg::ImportImage(bytes, r) => {
                let res = normalize_upload(&bytes)
                    .map_err(|e| e.to_string())
                    .and_then(|png| self.store.save_image(&png).map_err(|e| format!("Uložení obrázku selhalo: {e}")));
                let _ = r.send(res);
            }
            Msg::Shutdown => {
                self.runner.release_all();
                let _ = self.device_tx.send(DeviceCmd::Stop);
                return false;
            }
        }
        true
    }
}

pub fn spawn_engine(deps: EngineDeps) -> (Sender<Msg>, JoinHandle<()>) {
    let (tx, rx) = channel::<Msg>();
    let handle = std::thread::Builder::new()
        .name("engine".into())
        .spawn(move || {
            let mut engine = Engine::new(deps);
            for msg in rx {
                if !engine.handle(msg) {
                    break;
                }
            }
        })
        .expect("spawn engine thread");
    (tx, handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{new_id, normalize_config};
    use crate::keyboard::Keyboard;
    use serde_json::json;
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Default, Clone)]
    struct Rec(Arc<Mutex<Vec<String>>>);
    impl Rec {
        fn push(&self, s: String) {
            self.0.lock().unwrap().push(s);
        }
        fn get(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }
    struct FakeKb(Rec);
    impl Keyboard for FakeKb {
        fn down(&self, c: &str) { self.0.push(format!("down {c}")) }
        fn up(&self, c: &str) { self.0.push(format!("up {c}")) }
        fn tap_vk(&self, _: u16) {}
        fn type_text(&self, _: &str) {}
    }
    struct FakeSink(Rec);
    impl EventSink for FakeSink {
        fn state(&self, _: &State) {}
        fn config(&self, _: &Config) {}
        fn key(&self, k: u8, p: bool) { self.0.push(format!("key {k} {p}")) }
        fn raw(&self, d: u8, p: bool) { self.0.push(format!("raw {d} {p}")) }
    }

    fn render(spec: &KeySpec) -> Vec<u8> {
        format!("{}|{:?}|{}", spec.label, spec.bg, spec.image.is_some()).into_bytes()
    }

    fn seed() -> Config {
        normalize_config(&json!({
            "defaultProfileId": "A", "activeProfileId": "A",
            "profiles": [
                { "id": "A", "name": "A", "keys": {
                    "1": { "action": { "type": "keys", "codes": ["KeyQ"] } },
                    "2": { "action": { "type": "profile", "profileId": "B" } },
                    "3": { "action": { "type": "profile", "profileId": "gone" } },
                    "4": { "label": "four", "action": { "type": "none" } }
                } },
                { "id": "B", "name": "B", "processes": ["wow.exe"], "keys": {
                    "1": { "action": { "type": "keys", "codes": ["KeyW"] } }
                } }
            ]
        }))
    }

    struct T {
        tx: Sender<Msg>,
        dev: Receiver<DeviceCmd>,
        kb: Rec,
        sink: Rec,
        store: Store,
    }

    impl T {
        fn new() -> T {
            let dir = std::env::temp_dir().join(format!("deck-engine-{}", new_id()));
            let store = Store::new(dir);
            store.save(&seed()).unwrap();
            let (dtx, drx) = channel();
            let kb = Rec::default();
            let sink = Rec::default();
            let (tx, _h) = spawn_engine(EngineDeps {
                store: store.clone(),
                device_tx: dtx,
                keyboard: Arc::new(FakeKb(kb.clone())),
                launch: Arc::new(|_: &str, _: &[String]| {}),
                system: Arc::new(|_: &str| {}),
                sink: Box::new(FakeSink(sink.clone())),
                render,
            });
            let t = T { tx, dev: drx, kb, sink, store };
            t.state(); // wait for startup
            while t.dev.try_recv().is_ok() {} // drop startup SetKeyMap/SetBrightness
            t
        }
        fn send(&self, m: Msg) {
            self.tx.send(m).unwrap();
        }
        fn state(&self) -> State {
            let (r, rx) = channel();
            self.send(Msg::GetState(r));
            rx.recv_timeout(Duration::from_secs(2)).unwrap()
        }
        fn config(&self) -> Config {
            let (r, rx) = channel();
            self.send(Msg::GetConfig(r));
            rx.recv_timeout(Duration::from_secs(2)).unwrap()
        }
        fn last_images(&self) -> Vec<u8> {
            let mut last = None;
            while let Ok(cmd) = self.dev.try_recv() {
                if let DeviceCmd::SetImages(v) = cmd {
                    last = Some(v.into_iter().map(|(k, _)| k).collect());
                }
            }
            last.expect("no SetImages sent")
        }
        fn key(&self, key: u8, pressed: bool) {
            self.send(Msg::Device(DeviceEvent::Key { key, pressed }));
        }
        fn set_config(&self, c: &Config) -> Result<Config, String> {
            let (r, rx) = channel();
            self.send(Msg::SetConfig(serde_json::to_value(c).unwrap(), r));
            rx.recv_timeout(Duration::from_secs(2)).unwrap()
        }
    }

    #[test]
    fn connect_renders_all_15_keys() {
        let t = T::new();
        t.send(Msg::Device(DeviceEvent::Connected));
        assert!(t.state().connected);
        assert_eq!(t.last_images(), (1..=15).collect::<Vec<u8>>());
    }

    #[test]
    fn deck_key_runs_active_profile_action() {
        let t = T::new();
        t.key(1, true);
        t.key(1, false);
        t.state();
        assert_eq!(t.kb.get(), vec!["down KeyQ", "up KeyQ"]);
        assert_eq!(t.sink.get(), vec!["key 1 true", "key 1 false"]);
    }

    #[test]
    fn foreground_game_switches_profile_and_back() {
        let t = T::new();
        t.send(Msg::Device(DeviceEvent::Connected));
        t.state();
        t.last_images();
        t.send(Msg::Foreground(Foreground { exe: Some("Wow.exe".into()), elevated: false }));
        assert_eq!(t.state().active_profile_id, "B");
        assert_eq!(t.last_images(), vec![4]); // only tiles that differ from profile A
        t.send(Msg::Foreground(Foreground { exe: Some("chrome.exe".into()), elevated: false }));
        let s = t.state();
        assert_eq!(s.active_profile_id, "A");
        assert_eq!(s.foreground_exe.as_deref(), Some("chrome.exe"));
    }

    #[test]
    fn disconnect_releases_held_keys() {
        let t = T::new();
        t.key(1, true);
        t.send(Msg::Device(DeviceEvent::Disconnected));
        assert!(!t.state().connected);
        assert_eq!(t.kb.get(), vec!["down KeyQ", "up KeyQ"]);
    }

    #[test]
    fn set_config_rerenders_changed_keys_and_persists() {
        let t = T::new();
        t.send(Msg::Device(DeviceEvent::Connected));
        t.state();
        t.last_images();
        let mut c = t.config();
        c.profiles[0].keys.get_mut("4").unwrap().label = "changed".into();
        t.set_config(&c).unwrap();
        assert_eq!(t.last_images(), vec![4]);
        assert_eq!(t.store.load().profiles[0].key(4).label, "changed");
    }

    #[test]
    fn profile_action_switches_and_dangling_target_is_noop() {
        let t = T::new();
        t.key(3, true);
        assert_eq!(t.state().active_profile_id, "A");
        t.key(2, true);
        let s = t.state(); // SwitchProfile is queued behind the key event
        let s = if s.active_profile_id == "B" { s } else { t.state() };
        assert_eq!(s.active_profile_id, "B");
        assert_eq!(s.manual_profile_id, "B");
        assert_eq!(t.store.load().active_profile_id, "B");
    }

    #[test]
    fn back_returns_to_previous_profile() {
        let t = T::new();
        t.send(Msg::SetActiveProfile("B".into()));
        t.send(Msg::SwitchProfile("__back".into()));
        assert_eq!(t.state().active_profile_id, "A");
    }

    #[test]
    fn deleting_active_profile_falls_back_to_default() {
        let t = T::new();
        t.send(Msg::SetActiveProfile("B".into()));
        t.state();
        let mut c = t.config();
        c.profiles.retain(|p| p.id != "B");
        t.set_config(&c).unwrap();
        assert_eq!(t.state().active_profile_id, "A");
    }

    #[test]
    fn calibration_emits_raw_but_runs_nothing() {
        let t = T::new();
        t.send(Msg::SetCalibrating(true));
        t.send(Msg::Device(DeviceEvent::Raw { device_id: 1, pressed: true }));
        t.key(1, true);
        t.state();
        assert!(t.kb.get().is_empty());
        assert!(t.sink.get().contains(&"raw 1 true".to_string()));
    }

    #[test]
    fn import_image_validates() {
        let t = T::new();
        let (r, rx) = channel();
        t.send(Msg::ImportImage(b"hello".to_vec(), r));
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap().is_err());
        let mut png = Vec::new();
        image::DynamicImage::new_rgb8(30, 30).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let (r, rx) = channel();
        t.send(Msg::ImportImage(png, r));
        let id = rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        assert!(t.store.read_image(&id).is_some());
    }
}
