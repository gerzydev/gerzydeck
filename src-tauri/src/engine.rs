//! Engine actor: owns config + active profile, renders the deck, routes presses to actions.

pub use crate::config::Config;
use crate::config::{decide_profile, normalize_config, Action};
use crate::plugins::link::{context_for, parse_context, HostCmd, KeyCtx, PluginEvent, PluginStatus};
use crate::plugins::protocol::ActionCtx;
use image::RgbaImage;
use std::collections::HashSet;
use std::time::{Duration, Instant};
use crate::device::{DeviceCmd, DeviceEvent};
use crate::keyboard::Keyboard;
pub use crate::os::Foreground;
use crate::render::{normalize_upload, parse_hex, KeySpec};
use crate::runner::{ActionRunner, CommandFn, LaunchFn};
pub use crate::store::Store;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
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
    pub playing_sounds: std::collections::BTreeSet<String>,
    pub active_profile_id: String,
    pub manual_profile_id: String,
    pub active_profile_name: String,
    pub foreground_exe: Option<String>,
    pub foreground_elevated: bool,
    pub conflicts: Vec<String>,
    pub calibrating: bool,
    /// AI usage per provider, for the editor's key previews
    pub usage: std::collections::BTreeMap<String, crate::usage::UsageView>,
}

/// Device id reported to plugins.
pub const DEVICE_ID: &str = "D6-main";
/// Minimum gap between two plugin-driven frames of one key (≤ 10 fps).
const PLUGIN_FRAME_GAP: Duration = Duration::from_millis(100);
const OVERLAY_TIME: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overlay {
    #[default]
    None,
    Ok,
    Alert,
}

/// Everything needed to draw a key driven by a plugin.
#[derive(Debug, Clone, Default)]
pub struct PluginTile {
    pub image: Option<Arc<RgbaImage>>,
    /// live title from the plugin (shown at the bottom)
    pub title: Option<String>,
    /// user label from the editor (shown at the top)
    pub label: Option<String>,
    pub bg: [u8; 3],
    pub overlay: Overlay,
    /// plugin not installed: draw the label with a warning marker
    pub missing: bool,
    /// where the live title goes: editor choice, else manifest hint ("top" | "middle" | "bottom")
    pub align: Option<String>,
    /// manifest font size (SDK points)
    pub size: Option<u32>,
    pub show_title: bool,
}

struct PluginImage {
    original: Arc<RgbaImage>,
    skin: Option<Arc<RgbaImage>>,
    themed: OnceLock<Arc<RgbaImage>>,
}

/// Latest images/titles a plugin set for one action instance (None key = all states).
#[derive(Default)]
struct PluginView {
    styles: Vec<crate::plugins::link::TitleStyle>,
    images: HashMap<Option<u8>, Arc<PluginImage>>,
    titles: HashMap<Option<u8>, String>,
    overlay: Option<(Overlay, Instant)>,
    gen: u64,
}

impl PluginView {
    fn pick<T: Clone>(map: &HashMap<Option<u8>, T>, state: u8) -> Option<T> {
        map.get(&Some(state)).or_else(|| map.get(&None)).cloned()
    }
    fn set<T>(map: &mut HashMap<Option<u8>, T>, state: Option<u8>, value: Option<T>) {
        if state.is_none() {
            map.clear();
        }
        match value {
            Some(v) => {
                map.insert(state, v);
            }
            None => {
                map.remove(&state);
            }
        }
    }
}

pub enum Msg {
    Device(DeviceEvent),
    /// boot logo JPEG (already 800×480, rotated) for the deck's flash; the device thread answers
    SetBootLogo { jpeg: Vec<u8>, reply: Sender<Result<(), String>> },
    /// fresh numbers from the usage poller
    Usage { provider: String, result: Result<crate::usage::Usage, String> },
    /// which usage providers have a key on the active profile
    UsageWanted(Sender<Vec<String>>),
    Plugin(PluginEvent),
    SoundPlaying { sound: String, playing: bool },
    /// drag & drop in the editor: swap two keys of a profile (content, action, plugin settings)
    SwapKeys { profile_id: String, a: u8, b: u8, reply: Sender<Result<Config, String>> },
    /// property inspector for a key: forwarded to the host with the key's current action instance
    OpenPi { profile_id: String, key: u8, reply: Sender<Option<crate::plugins::link::PiInfo>> },
    /// periodic wake-up for throttled frames and overlay expiry
    Tick,
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

/// What a usage key shows; drawn by `EngineDeps::render_usage`.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageTile {
    pub name: String,
    pub percent: Option<f32>,
    /// countdown to the reset
    pub reset: Option<String>,
    pub error: Option<String>,
    pub bg: [u8; 3],
}

pub struct EngineDeps {
    pub store: Store,
    pub device_tx: Sender<DeviceCmd>,
    pub keyboard: Arc<dyn Keyboard>,
    pub launch: LaunchFn,
    pub system: CommandFn,
    pub sink: Box<dyn EventSink>,
    pub render: fn(&KeySpec) -> Vec<u8>,
    pub render_plugin: fn(&PluginTile) -> Vec<u8>,
    pub render_usage: fn(&UsageTile) -> Vec<u8>,
    /// wakes the usage poller (a usage key was pressed)
    pub usage_wake: Option<Sender<()>>,
    pub host_tx: Option<Sender<HostCmd>>,
    /// soundboard playback thread
    pub sound_tx: Option<Sender<crate::sound::SoundCmd>>,
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
    render_plugin: fn(&PluginTile) -> Vec<u8>,
    render_usage: fn(&UsageTile) -> Vec<u8>,
    usage_wake: Option<Sender<()>>,
    sound_tx: Option<Sender<crate::sound::SoundCmd>>,
    held_sounds: HashMap<u8, String>,
    usage: HashMap<String, Result<crate::usage::Usage, String>>,
    host_tx: Option<Sender<HostCmd>>,
    /// action instances the host was told are visible (context → instance)
    visible: HashMap<String, KeyCtx>,
    /// user titles already sent as titleParametersDidChange
    sent_titles: HashMap<String, String>,
    views: HashMap<String, PluginView>,
    missing_plugins: HashSet<String>,
    last_frame: HashMap<u8, Instant>,
    frame_pending: bool,
    /// decoded user images drawn under plugin titles (image id → pixels)
    user_images: HashMap<String, Arc<RgbaImage>>,
    themed_images: HashMap<[u8; 32], Arc<RgbaImage>>,
}

impl Engine {
    fn new(d: EngineDeps) -> Self {
        let config = d.store.load();
        crate::i18n::set_language(&config.language);
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
            render_plugin: d.render_plugin,
            render_usage: d.render_usage,
            usage_wake: d.usage_wake,
            sound_tx: d.sound_tx,
            held_sounds: HashMap::new(),
            usage: HashMap::new(),
            host_tx: d.host_tx,
            visible: HashMap::new(),
            sent_titles: HashMap::new(),
            views: HashMap::new(),
            missing_plugins: HashSet::new(),
            last_frame: HashMap::new(),
            frame_pending: false,
            user_images: HashMap::new(),
            themed_images: HashMap::new(),
        };
        e.refresh_name();
        e.send_sound_devices();
        e
    }

    fn active(&self) -> &crate::config::Profile {
        self.config.profiles.iter().find(|p| p.id == self.state.active_profile_id).unwrap_or(&self.config.profiles[0])
    }

    fn send_sound_devices(&self) {
        if let Some(tx) = &self.sound_tx {
            let _ = tx.send(crate::sound::SoundCmd::SetDevices { me: self.config.sound_me_device.clone(), mic: self.config.sound_mic_device.clone() });
        }
    }

    fn play_sound(&self, action: &Action) {
        let (Some(tx), Action::Sound { sound, me, mic, mode, .. }) = (&self.sound_tx, action) else { return };
        use crate::sound::{Mode, SoundCmd};
        let cmd = match mode.as_str() {
            "stopAll" => SoundCmd::StopAll,
            _ if sound.is_empty() => return,
            m => SoundCmd::Press { sound: sound.clone(), me: *me, mic: *mic, mode: match m { "restart" => Mode::Restart, "hold" => Mode::Hold, _ => Mode::Toggle } },
        };
        let _ = tx.send(cmd);
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
        log::info!("profile -> {id}");
        self.previous_profile = Some(std::mem::replace(&mut self.state.active_profile_id, id));
        self.emit_state();
        self.sync_visible();
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
        let target = match id.as_str() {
            "__back" => self.previous_profile.clone(),
            // cycle through the profiles in their list order, wrapping around
            "__next" | "__prev" => {
                let list = &self.config.profiles;
                let i = list.iter().position(|p| p.id == self.state.active_profile_id).unwrap_or(0);
                let j = if id == "__next" { (i + 1) % list.len() } else { (i + list.len() - 1) % list.len() };
                Some(list[j].id.clone())
            }
            _ => Some(id),
        };
        if let Some(t) = target {
            self.set_active(t);
        }
    }

    fn host(&self, cmd: HostCmd) {
        if let Some(tx) = &self.host_tx {
            let _ = tx.send(cmd);
        }
    }

    fn key_ctx(profile_id: &str, key: u8, action: &Action) -> Option<KeyCtx> {
        let Action::Plugin { plugin, action, settings, state } = action else { return None };
        Some(KeyCtx {
            plugin: plugin.clone(),
            ctx: ActionCtx {
                action: action.clone(),
                context: context_for(profile_id, key),
                device: DEVICE_ID.into(),
                key,
                settings: settings.clone(),
                state: *state,
            },
        })
    }

    /// Tells the host which plugin actions are on the deck now (willAppear / willDisappear diff)
    /// and forwards changed user titles.
    fn sync_visible(&mut self) {
        let mut desired: HashMap<String, (KeyCtx, String)> = HashMap::new();
        if self.state.connected {
            let profile = self.active();
            for k in 1..=15u8 {
                let cfg = profile.key(k);
                if let Some(kc) = Self::key_ctx(&profile.id, k, &cfg.action) {
                    desired.insert(kc.ctx.context.clone(), (kc, cfg.label.clone()));
                }
            }
        }
        let gone: Vec<String> = self
            .visible
            .iter()
            .filter(|(c, old)| desired.get(*c).is_none_or(|(new, _)| new.plugin != old.plugin || new.ctx.action != old.ctx.action))
            .map(|(c, _)| c.clone())
            .collect();
        for c in gone {
            if let Some(old) = self.visible.remove(&c) {
                self.sent_titles.remove(&c);
                self.host(HostCmd::WillDisappear(old));
            }
        }
        let mut contexts: Vec<String> = desired.keys().cloned().collect();
        contexts.sort();
        for c in contexts {
            let (kc, label) = desired[&c].clone();
            if !self.visible.contains_key(&c) {
                self.host(HostCmd::WillAppear(kc.clone()));
            }
            self.visible.insert(c.clone(), kc.clone());
            if self.sent_titles.get(&c).map_or(!label.is_empty(), |t| *t != label) {
                self.sent_titles.insert(c.clone(), label.clone());
                self.host(HostCmd::TitleParams { key: kc, title: label });
            }
        }
    }

    fn theme_cached(&mut self, source: &RgbaImage) -> Arc<RgbaImage> {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(source.width().to_le_bytes());
        hash.update(source.height().to_le_bytes());
        hash.update(source.as_raw());
        let key: [u8; 32] = hash.finalize().into();
        if let Some(img) = self.themed_images.get(&key) { return img.clone(); }
        let img = Arc::new(crate::plugins::image::theme_image(source));
        // Bound retained dynamic artwork; live per-state images also retain their own result.
        if self.themed_images.len() >= 128 { self.themed_images.clear(); }
        self.themed_images.insert(key, img.clone());
        img
    }

    fn plugin_image(&mut self, context: &str, state: u8, unified: bool) -> Option<Arc<RgbaImage>> {
        let source = self.views.get(context).and_then(|v| PluginView::pick(&v.images, state));
        if !unified { return source.map(|s| s.original.clone()); }
        Some(match source {
            Some(s) => s.skin.clone().unwrap_or_else(|| s.themed.get_or_init(|| self.theme_cached(&s.original)).clone()),
            None => crate::plugins::image::theme_frame(),
        })
    }

    fn render_active(&mut self) {
        if !self.state.connected {
            return;
        }
        let profile = self.active().clone();
        let now = Instant::now();
        let mut images = Vec::new();
        self.frame_pending = false;
        for k in 1..=15u8 {
            let cfg = profile.key(k);
            let plugin = match &cfg.action {
                Action::Plugin { plugin, state, .. } => Some((plugin.clone(), *state)),
                _ => None,
            };
            let context = context_for(&profile.id, k);
            let overlay = self
                .views
                .get(&context)
                .and_then(|v| v.overlay)
                .filter(|(_, until)| *until > now)
                .map(|(o, _)| o)
                .unwrap_or_default();
            if let Action::Usage { provider, window } = &cfg.action {
                if !self.usage.contains_key(provider) {
                    // no numbers yet (start, profile switch): ask right away instead of waiting a minute
                    if let Some(w) = &self.usage_wake {
                        let _ = w.send(());
                    }
                }
                let tile = self.usage_tile(provider, window, cfg);
                let sig = format!("U|{tile:?}");
                if self.rendered.get(&k) != Some(&sig) {
                    images.push((k, (self.render_usage)(&tile)));
                    self.rendered.insert(k, sig);
                }
                continue;
            }
            let unified = self.config.plugin_look && !cfg.original_look;
            let sound_playing = matches!(&cfg.action, Action::Sound { sound, mode, .. } if mode != "stopAll" && self.state.playing_sounds.contains(sound));
            let sig = match &plugin {
                Some((p, state)) => format!(
                    "P|{}|{:?}|{}|{}|{}|{:?}|{}|{:?}|{}|{}",
                    self.views.get(&context).map_or(0, |v| v.gen),
                    cfg.image,
                    state,
                    cfg.label,
                    cfg.bg,
                    overlay,
                    self.missing_plugins.contains(p),
                    cfg.title_align, self.config.plugin_look, cfg.original_look
                ),
                None => format!("{:?}|{}|{}|{}", cfg.image, cfg.label, cfg.bg, sound_playing),
            };
            if self.rendered.get(&k) == Some(&sig) {
                continue;
            }
            let jpeg = match &plugin {
                Some((p, state)) => {
                    if self.last_frame.get(&k).is_some_and(|t| now.duration_since(*t) < PLUGIN_FRAME_GAP) {
                        self.frame_pending = true;
                        continue;
                    }
                    let own = cfg.image.as_deref().and_then(|id| self.user_image(id));
                    let image = own.or_else(|| self.plugin_image(&context, *state, unified));
                    let view = self.views.get(&context);
                    let style = view.and_then(|v| v.styles.get(*state as usize).or_else(|| v.styles.first())).cloned().unwrap_or_default();
                    let tile = PluginTile {
                        align: cfg.title_align.clone().or(style.align).or_else(|| {
                            if !unified { return None; }
                            let Action::Plugin { action, .. } = &cfg.action else { return None };
                            crate::plugins::skins::title_align(action).map(String::from)
                        }),
                        size: style.size,
                        show_title: style.show.unwrap_or(true),
                        // the user's own image replaces the plugin image, the live title stays
                        image,
                        title: view.and_then(|v| PluginView::pick(&v.titles, *state)),
                        label: (!cfg.label.is_empty()).then(|| cfg.label.clone()),
                        bg: parse_hex(&cfg.bg),
                        overlay,
                        missing: self.missing_plugins.contains(p),
                    };
                    self.last_frame.insert(k, now);
                    (self.render_plugin)(&tile)
                }
                None => {
                    let spec = KeySpec {
                        playing: sound_playing,
                        image: cfg.image.as_deref().and_then(|id| self.store.read_image(id)),
                        label: cfg.label.clone(),
                        bg: parse_hex(&cfg.bg),
                    };
                    (self.render)(&spec)
                }
            };
            images.push((k, jpeg));
            self.rendered.insert(k, sig);
        }
        if !images.is_empty() {
            let _ = self.device_tx.send(DeviceCmd::SetImages(images));
        }
    }

    fn usage_tile(&self, provider: &str, window: &str, cfg: &crate::config::KeyConfig) -> UsageTile {
        let service = if provider == "claude" { "Claude" } else { "GPT" };
        let name = if cfg.label.is_empty() { format!("{service} {}", if window == "weekly" { crate::i18n::tr("týden", "week") } else { "5h" }) } else { cfg.label.clone() };
        let mut tile = UsageTile { name, percent: None, reset: None, error: None, bg: parse_hex(&cfg.bg) };
        match self.usage.get(provider) {
            Some(Ok(u)) => {
                if let Some(w) = if window == "weekly" { u.weekly } else { u.session } {
                    tile.percent = Some(w.percent);
                    tile.reset = Some(crate::usage::reset_text(crate::usage::now_s(), w.resets_at));
                }
            }
            Some(Err(e)) => tile.error = Some(e.clone()),
            None => {}
        }
        tile
    }

    fn wanted_usage(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for k in 1..=15u8 {
            if let Action::Usage { provider, .. } = &self.active().key(k).action {
                if !out.contains(provider) {
                    out.push(provider.clone());
                }
            }
        }
        out
    }

    /// Mutable plugin action at a context, if the context names a plugin key in the config.
    fn plugin_action_mut(&mut self, context: &str) -> Option<(&mut serde_json::Value, &mut u8)> {
        let (profile_id, key) = parse_context(context)?;
        let profile = self.config.profiles.iter_mut().find(|p| p.id == profile_id)?;
        match &mut profile.keys.get_mut(&key.to_string())?.action {
            Action::Plugin { settings, state, .. } => Some((settings, state)),
            _ => None,
        }
    }

    fn is_plugin_context(&self, context: &str) -> bool {
        parse_context(context).is_some_and(|(p, k)| {
            self.config.profiles.iter().any(|pr| pr.id == p && matches!(pr.key(k).action, Action::Plugin { .. }))
        })
    }

    fn touch_view(&mut self, context: &str) -> Option<&mut PluginView> {
        if !self.is_plugin_context(context) {
            return None;
        }
        let v = self.views.entry(context.to_string()).or_default();
        v.gen += 1;
        Some(v)
    }

    fn save_config(&mut self) {
        if let Err(e) = self.store.save(&self.config) {
            log::error!("saving config failed: {e}");
        }
        self.sink.config(&self.config);
    }

    fn on_plugin(&mut self, ev: PluginEvent) {
        match ev {
            PluginEvent::SetImage { context, image, skin, state } => {
                if let Some(v) = self.touch_view(&context) {
                    PluginView::set(&mut v.images, state, image.map(|original| Arc::new(PluginImage { original, skin, themed: OnceLock::new() })));
                    self.render_active();
                }
            }
            PluginEvent::SetTitle { context, title, state } => {
                if let Some(v) = self.touch_view(&context) {
                    PluginView::set(&mut v.titles, state, title);
                    self.render_active();
                }
            }
            PluginEvent::SetState { context, state } => {
                let state = state.min(15);
                if let Some((_, st)) = self.plugin_action_mut(&context) {
                    *st = state;
                    if let Some(kc) = self.visible.get_mut(&context) {
                        kc.ctx.state = state;
                    }
                    self.touch_view(&context);
                    self.save_config();
                    self.render_active();
                }
            }
            PluginEvent::SetSettings { context, settings } => {
                let settings = if settings.is_object() { settings } else { serde_json::json!({}) };
                if let Some((s, _)) = self.plugin_action_mut(&context) {
                    *s = settings.clone();
                    if let Some(kc) = self.visible.get_mut(&context) {
                        kc.ctx.settings = settings;
                    }
                    self.save_config();
                }
            }
            PluginEvent::TitleStyle { context, styles } => {
                if let Some(v) = self.touch_view(&context) {
                    v.styles = styles;
                    self.render_active();
                }
            }
            PluginEvent::ShowOk { context } => self.overlay(&context, Overlay::Ok),
            PluginEvent::ShowAlert { context } => self.overlay(&context, Overlay::Alert),
            PluginEvent::SwitchProfile { profile } => {
                let id = self.config.profiles.iter().find(|p| p.name == profile || p.id == profile).map(|p| p.id.clone());
                if let Some(id) = id {
                    self.set_active(id);
                }
            }
            PluginEvent::Status { plugin, status } => {
                let changed = if status == PluginStatus::NotInstalled {
                    self.missing_plugins.insert(plugin)
                } else {
                    self.missing_plugins.remove(&plugin)
                };
                if changed {
                    self.render_active();
                }
            }
        }
    }

    fn overlay(&mut self, context: &str, o: Overlay) {
        if let Some(v) = self.touch_view(context) {
            v.overlay = Some((o, Instant::now() + OVERLAY_TIME));
            if let Some((_, k)) = parse_context(context) {
                self.last_frame.remove(&k); // feedback shows right away
            }
            self.render_active();
        }
    }

    fn on_tick(&mut self) {
        let now = Instant::now();
        let mut expired = false;
        for v in self.views.values_mut() {
            if v.overlay.is_some_and(|(_, until)| until <= now) {
                v.overlay = None;
                v.gen += 1;
                expired = true;
            }
        }
        if expired || self.frame_pending {
            self.render_active();
        }
    }

    fn release_sound(&mut self, key: u8) {
        if let Some(sound) = self.held_sounds.remove(&key) {
            if let Some(tx) = &self.sound_tx {
                let _ = tx.send(crate::sound::SoundCmd::Stop { sound });
            }
        }
    }

    fn release_sounds(&mut self) {
        let keys: Vec<_> = self.held_sounds.keys().copied().collect();
        for key in keys { self.release_sound(key); }
    }

    fn on_key(&mut self, key: u8, pressed: bool) {
        self.sink.key(key, pressed);
        // Release the sound captured at key-down, even after a profile/action change.
        if !pressed { self.release_sound(key); self.runner.release(key); }
        if self.state.calibrating {
            return;
        }
        let context = context_for(&self.active().id, key);
        if let Some(kc) = self.visible.get(&context).cloned() {
            self.host(if pressed { HostCmd::KeyDown(kc) } else { HostCmd::KeyUp(kc) });
            return;
        }
        if pressed {
            let action = self.active().key(key).action.clone();
            if matches!(action, Action::Usage { .. }) {
                if let Some(w) = &self.usage_wake {
                    let _ = w.send(());
                }
            }
            if matches!(action, Action::Sound { .. }) {
                if self.held_sounds.contains_key(&key) { return; }
                if let Action::Sound { sound, mode, .. } = &action {
                    if mode == "hold" && !sound.is_empty() { self.held_sounds.insert(key, sound.clone()); }
                }
                self.play_sound(&action);
            }
            let _ = self.runner.press(key, &action);
            let switches: Vec<String> = std::mem::take(&mut *self.pending_switches.lock().unwrap());
            for id in switches {
                self.switch(id);
            }
        }
    }

    fn set_config(&mut self, raw: &serde_json::Value) -> Result<Config, String> {
        let mut cfg = normalize_config(raw);
        // plugin settings/state are owned by the core: keep what plugins wrote, whatever the editor sent
        for profile in &mut cfg.profiles {
            let Some(old) = self.config.profiles.iter().find(|p| p.id == profile.id) else { continue };
            for (k, kc) in profile.keys.iter_mut() {
                let Some(old_key) = old.keys.get(k) else { continue };
                if let (Action::Plugin { plugin, action, settings, state }, Action::Plugin { plugin: op, action: oa, settings: os, state: ost }) =
                    (&mut kc.action, &old_key.action)
                {
                    if plugin == op && action == oa {
                        *settings = os.clone();
                        *state = *ost;
                    }
                }
            }
        }
        self.store.save(&cfg).map_err(|e| crate::trf!("Uložení selhalo: {e}", "Saving failed: {e}"))?;
        self.store.prune_images(&cfg);
        self.store.prune_sounds(&cfg);
        if cfg.brightness != self.config.brightness {
            let _ = self.device_tx.send(DeviceCmd::SetBrightness(cfg.brightness));
        }
        if cfg.key_map != self.config.key_map {
            let _ = self.device_tx.send(DeviceCmd::SetKeyMap(key_map_array(&cfg.key_map)));
        }
        let devices_changed = cfg.sound_me_device != self.config.sound_me_device || cfg.sound_mic_device != self.config.sound_mic_device;
        // An editor look toggle must take effect immediately, outside plugin frame throttling.
        if cfg.plugin_look != self.config.plugin_look || cfg.profiles.iter().any(|p| {
            self.config.profiles.iter().find(|old| old.id == p.id).is_some_and(|old| {
                p.keys.iter().any(|(k, key)| old.keys.get(k).is_some_and(|v| v.original_look != key.original_look))
            })
        }) { self.last_frame.clear(); }
        self.config = cfg;
        if devices_changed {
            self.send_sound_devices();
        }
        if !self.exists(&self.state.manual_profile_id) {
            self.state.manual_profile_id = self.config.default_profile_id.clone();
        }
        if self.previous_profile.as_deref().is_some_and(|p| !self.exists(p)) {
            self.previous_profile = None;
        }
        Ok(self.config.clone())
    }

    fn swap_keys(&mut self, profile_id: &str, a: u8, b: u8) -> Result<Config, String> {
        if a == b || !(1..=15).contains(&a) || !(1..=15).contains(&b) {
            return Err(crate::i18n::tr("Neplatné tlačítko", "Invalid key").into());
        }
        let profile = self.config.profiles.iter_mut().find(|p| p.id == profile_id).ok_or("Profil neexistuje")?;
        let ka = profile.keys.remove(&a.to_string()).unwrap_or_default();
        let kb = profile.keys.remove(&b.to_string()).unwrap_or_default();
        profile.keys.insert(a.to_string(), kb);
        profile.keys.insert(b.to_string(), ka);
        // live plugin images/titles move with their keys
        let (ca, cb) = (context_for(profile_id, a), context_for(profile_id, b));
        let va = self.views.remove(&ca);
        let vb = self.views.remove(&cb);
        if let Some(v) = va {
            self.views.insert(cb.clone(), v);
        }
        if let Some(v) = vb {
            self.views.insert(ca.clone(), v);
        }
        for k in [a, b] {
            self.rendered.remove(&k);
            self.last_frame.remove(&k);
        }
        self.store.save(&self.config).map_err(|e| crate::trf!("Uložení selhalo: {e}", "Saving failed: {e}"))?;
        Ok(self.config.clone())
    }

    fn user_image(&mut self, id: &str) -> Option<Arc<RgbaImage>> {
        if !self.user_images.contains_key(id) {
            let img = image::load_from_memory(&self.store.read_image(id)?).ok()?.to_rgba8();
            if self.user_images.len() > 64 {
                self.user_images.clear();
            }
            self.user_images.insert(id.to_string(), Arc::new(img));
        }
        self.user_images.get(id).cloned()
    }

    /// Notifications and rendering after a config change. Runs after the caller got its reply,
    /// because sinks may block (Tauri tray calls wait for the main thread).
    fn after_config_change(&mut self) {
        if crate::i18n::is_czech() != (self.config.language == "cs") {
            crate::i18n::set_language(&self.config.language);
            self.rendered.clear(); // key texts (AI usage) follow the language
        }
        self.sink.config(&self.config);
        if self.exists(&self.state.active_profile_id) {
            self.emit_state();
            self.sync_visible();
            self.render_active();
        } else {
            let target = decide_profile(&self.config, self.state.foreground_exe.as_deref(), &self.state.manual_profile_id);
            self.activate(target);
        }
    }

    /// Returns false on shutdown.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Device(DeviceEvent::Connected) => {
                log::info!("deck connected");
                self.state.connected = true;
                self.rendered.clear();
                self.last_frame.clear();
                self.emit_state();
                self.host(HostCmd::DeviceConnected);
                self.sync_visible();
                self.render_active();
            }
            Msg::Device(DeviceEvent::Disconnected) => {
                log::info!("deck disconnected");
                self.runner.release_all();
                self.release_sounds();
                self.state.connected = false;
                self.emit_state();
                self.sync_visible();
                self.host(HostCmd::DeviceDisconnected);
            }
            Msg::Device(DeviceEvent::Raw { device_id, pressed }) => self.sink.raw(device_id, pressed),
            Msg::Device(DeviceEvent::Redraw) => {
                self.rendered.clear();
                self.last_frame.clear();
                self.render_active();
            }
            Msg::Device(DeviceEvent::Key { key, pressed }) => self.on_key(key, pressed),
            Msg::Plugin(ev) => self.on_plugin(ev),
            Msg::SoundPlaying { sound, playing } => {
                let changed = if playing { self.state.playing_sounds.insert(sound) } else { self.state.playing_sounds.remove(&sound) };
                if changed {
                    self.sink.state(&self.state);
                    self.render_active();
                }
            }
            Msg::SwapKeys { profile_id, a, b, reply } => {
                let res = self.swap_keys(&profile_id, a, b);
                let ok = res.is_ok();
                let _ = reply.send(res);
                if ok {
                    self.after_config_change();
                }
            }
            Msg::OpenPi { profile_id, key, reply } => {
                let kc = self.config.profiles.iter().find(|p| p.id == profile_id).and_then(|p| Self::key_ctx(&p.id, key, &p.key(key).action));
                match (kc, &self.host_tx) {
                    (Some(kc), Some(tx)) => {
                        let _ = tx.send(HostCmd::PiOpen { key: kc, reply });
                    }
                    _ => {
                        let _ = reply.send(None);
                    }
                }
            }
            Msg::Tick => self.on_tick(),
            Msg::SetBootLogo { jpeg, reply } => {
                if !self.state.connected {
                    let _ = reply.send(Err(crate::i18n::tr("Deck není připojený", "The deck is not connected").into()));
                } else if let Err(std::sync::mpsc::SendError(DeviceCmd::SetBootLogo { reply, .. })) = self.device_tx.send(DeviceCmd::SetBootLogo { jpeg, reply }) {
                    let _ = reply.send(Err(crate::i18n::tr("Ovladač decku neběží", "The deck driver is not running").into()));
                }
            }
            Msg::Usage { provider, result } => {
                // keep showing the last numbers when a refresh fails for network reasons
                let keep = matches!((&result, self.usage.get(&provider)), (Err(e), Some(Ok(_))) if e.contains("internet") || e.contains("dotaz") || e.contains("requests"));
                if !keep {
                    let view = match &result {
                        Ok(u) => crate::usage::UsageView { usage: u.clone(), error: None },
                        Err(e) => crate::usage::UsageView { usage: Default::default(), error: Some(e.clone()) },
                    };
                    self.state.usage.insert(provider.clone(), view);
                    self.usage.insert(provider, result);
                    self.sink.state(&self.state);
                }
                self.render_active();
            }
            Msg::UsageWanted(reply) => {
                let _ = reply.send(self.wanted_usage());
            }
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
                let ok = res.is_ok();
                let _ = r.send(res);
                if ok {
                    self.after_config_change();
                }
            }
            Msg::SetActiveProfile(id) => self.set_active(id),
            Msg::SwitchProfile(id) => self.switch(id),
            Msg::SetCalibrating(on) => {
                self.state.calibrating = on;
                if on {
                    self.runner.release_all();
                    self.release_sounds();
                }
                self.emit_state();
            }
            Msg::ImportImage(bytes, r) => {
                let res = normalize_upload(&bytes)
                    .map_err(|e| e.to_string())
                    .and_then(|png| self.store.save_image(&png).map_err(|e| crate::trf!("Uložení obrázku selhalo: {e}", "Saving the image failed: {e}")));
                let _ = r.send(res);
            }
            Msg::Shutdown => {
                self.runner.release_all();
                self.release_sounds();
                if let Some(tx) = &self.sound_tx { let _ = tx.send(crate::sound::SoundCmd::StopAll); }
                self.host(HostCmd::Shutdown);
                let _ = self.device_tx.send(DeviceCmd::Stop);
                return false;
            }
        }
        true
    }
}

pub fn spawn_engine(deps: EngineDeps) -> (Sender<Msg>, JoinHandle<()>) {
    let (tx, rx) = channel::<Msg>();
    let ticker = tx.clone();
    std::thread::Builder::new()
        .name("engine-tick".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(50));
            if ticker.send(Msg::Tick).is_err() {
                break;
            }
        })
        .expect("spawn engine ticker");
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

    fn render_plugin(tile: &PluginTile) -> Vec<u8> {
        format!("P|{:?}|{:?}|{:?}|{}|{}", tile.label, tile.title, tile.overlay, tile.image.is_some(), tile.missing).into_bytes()
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
        _host: Receiver<HostCmd>,
        kb: Rec,
        sink: Rec,
        store: Store,
    }

    impl T {
        fn new() -> T {
            T::with(seed())
        }
        fn with(cfg: Config) -> T {
            let dir = std::env::temp_dir().join(format!("deck-engine-{}", new_id()));
            let store = Store::new(dir);
            store.save(&cfg).unwrap();
            let (htx, hrx) = channel();
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
                render_plugin,
                render_usage: |_| Vec::new(),
                usage_wake: None,
            sound_tx: None,
                host_tx: Some(htx),
            });
            let t = T { tx, dev: drx, _host: hrx, kb, sink, store };
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
            let res = rx.recv_timeout(Duration::from_secs(2)).unwrap();
            self.state(); // notifications/rendering run after the reply
            res
        }
    }

    fn look_engine() -> (Engine, Receiver<DeviceCmd>) {
        let store = Store::new(std::env::temp_dir().join(format!("deck-look-{}", new_id())));
        let mut cfg = seed();
        cfg.profiles[0].keys.get_mut("3").unwrap().action = Action::Plugin {
            plugin: "me.amankhanna.oasystem".into(), action: "me.amankhanna.oasystem.cpu".into(), settings: json!({}), state: 0
        };
        store.save(&cfg).unwrap();
        let (tx, rx) = channel();
        let e = Engine::new(EngineDeps {
            store, device_tx: tx, keyboard: Arc::new(FakeKb(Rec::default())),
            launch: Arc::new(|_, _| {}), system: Arc::new(|_| {}), sink: Box::new(FakeSink(Rec::default())),
            render, render_plugin: |t| serde_json::to_vec(&json!({
                "align":t.align, "title":t.title,
                "image":t.image.as_ref().map(|i| (i.dimensions(), i.get_pixel(i.width()/2, i.height()/2).0))
            })).unwrap(),
            render_usage: |_| vec![], usage_wake: None, sound_tx: None, host_tx: None,
        });
        (e, rx)
    }

    fn look_images(rx: &Receiver<DeviceCmd>) -> Vec<serde_json::Value> {
        rx.try_iter().flat_map(|c| match c { DeviceCmd::SetImages(v) => v, _ => vec![] })
            .filter(|(k, _)| *k == 3).map(|(_, b)| serde_json::from_slice(&b).unwrap()).collect()
    }

    #[test]
    fn plugin_look_selects_state_skin_theme_original_and_reuses_cached_images() {
        use image::Rgba;
        let (mut e, _) = look_engine();
        let original = Arc::new(RgbaImage::from_pixel(4, 4, Rgba([20, 40, 200, 255])));
        let skin = Arc::new(RgbaImage::from_pixel(144, 144, Rgba([200, 40, 20, 255])));
        e.on_plugin(PluginEvent::SetImage { context: "A:3".into(), image: Some(original.clone()), skin: Some(skin.clone()), state: Some(0) });
        assert!(Arc::ptr_eq(&e.plugin_image("A:3", 0, true).unwrap(), &skin));
        assert!(Arc::ptr_eq(&e.plugin_image("A:3", 0, false).unwrap(), &original));
        // The next state has its own original but no skin; never inherit state zero's skin.
        e.on_plugin(PluginEvent::SetImage { context: "A:3".into(), image: Some(original.clone()), skin: None, state: Some(1) });
        let themed = e.plugin_image("A:3", 1, true).unwrap();
        assert_eq!(themed.dimensions(), (144, 144));
        assert_eq!(themed.get_pixel(72, 72).0, [20, 40, 200, 255]);
        assert!(Arc::ptr_eq(&e.plugin_image("A:3", 1, true).unwrap(), &themed), "title-only redraws reuse the theme");
        // A newly decoded copy with the same content also hits the bounded content cache.
        e.on_plugin(PluginEvent::SetImage { context: "A:3".into(), image: Some(Arc::new((*original).clone())), skin: None, state: Some(1) });
        assert!(Arc::ptr_eq(&e.plugin_image("A:3", 1, true).unwrap(), &themed));
        assert_eq!(e.themed_images.len(), 1);
        e.on_plugin(PluginEvent::SetImage { context: "A:3".into(), image: None, skin: None, state: None });
        assert!(e.plugin_image("A:3", 0, false).is_none());
        assert!(Arc::ptr_eq(&e.plugin_image("A:3", 0, true).unwrap(), &crate::plugins::image::theme_frame()));
    }

    #[test]
    fn plugin_look_toggles_redraw_immediately_and_custom_images_win() {
        use image::Rgba;
        let (mut e, rx) = look_engine();
        e.on_plugin(PluginEvent::SetImage {
            context: "A:3".into(), image: Some(Arc::new(RgbaImage::from_pixel(4, 4, Rgba([10, 20, 30, 255])))),
            skin: Some(Arc::new(RgbaImage::from_pixel(144, 144, Rgba([200, 90, 30, 255])))), state: None
        });
        e.state.connected = true;
        e.render_active();
        let rendered = look_images(&rx);
        assert_eq!(rendered[0]["image"][0], json!([144, 144]));
        assert_eq!(rendered[0]["image"][1], json!([200, 90, 30, 255]));
        assert_eq!(rendered[0]["align"], "middle");
        let mut raw = serde_json::to_value(&e.config).unwrap();
        raw["pluginLook"] = json!(false);
        e.set_config(&raw).unwrap(); e.after_config_change();
        let rendered = look_images(&rx);
        assert_eq!(rendered.len(), 1);
        assert_eq!(rendered[0]["image"][0], json!([4, 4]));
        assert!(rendered[0]["align"].is_null());
        raw["pluginLook"] = json!(true);
        raw["profiles"][0]["keys"]["3"]["originalLook"] = json!(true);
        e.set_config(&raw).unwrap(); e.after_config_change();
        assert_eq!(look_images(&rx)[0]["image"][0], json!([4, 4]));
        raw["profiles"][0]["keys"]["3"]["originalLook"] = json!(false);
        e.set_config(&raw).unwrap(); e.after_config_change();
        assert_eq!(look_images(&rx)[0]["image"][0], json!([144, 144]));

        let mut png = Vec::new();
        RgbaImage::from_pixel(12, 12, Rgba([1, 250, 1, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let id = e.store.save_image(&png).unwrap();
        raw["profiles"][0]["keys"]["3"]["image"] = json!(id);
        e.last_frame.clear();
        e.set_config(&raw).unwrap(); e.after_config_change();
        assert_eq!(look_images(&rx)[0]["image"][1], json!([1, 250, 1, 255]));
    }

    #[test]
    fn plugin_look_titles_keep_changing_and_alignment_precedence_is_preserved() {
        use crate::plugins::link::TitleStyle;
        let (mut e, rx) = look_engine();
        // Title-only plugins receive the frame and skin alignment fallback.
        e.state.connected = true; e.render_active();
        assert_eq!(look_images(&rx)[0]["align"], "middle");
        e.last_frame.clear();
        e.on_plugin(PluginEvent::SetTitle { context: "A:3".into(), title: Some("42%".into()), state: None });
        let rendered = look_images(&rx);
        assert_eq!(rendered[0]["title"], "42%");
        assert_eq!(rendered[0]["image"][0], json!([144, 144]));
        e.last_frame.clear();
        e.on_plugin(PluginEvent::TitleStyle { context: "A:3".into(), styles: vec![TitleStyle { align: Some("top".into()), size: None, show: None }] });
        assert_eq!(look_images(&rx)[0]["align"], "top");
        e.config.profiles[0].keys.get_mut("3").unwrap().title_align = Some("bottom".into());
        e.last_frame.clear(); e.render_active();
        assert_eq!(look_images(&rx)[0]["align"], "bottom");
        e.last_frame.clear();
        e.on_plugin(PluginEvent::SetState { context: "A:3".into(), state: 1 });
        e.last_frame.clear();
        e.on_plugin(PluginEvent::SetTitle { context: "A:3".into(), title: Some("89%".into()), state: Some(1) });
        assert_eq!(look_images(&rx).last().unwrap()["title"], "89%");
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
    fn next_and_prev_cycle_through_profiles_with_wraparound() {
        let t = T::new();
        let ids: Vec<String> = t.config().profiles.iter().map(|p| p.id.clone()).collect();
        assert!(ids.len() >= 2);
        assert_eq!(t.state().active_profile_id, ids[0]);
        for expected in ids.iter().skip(1).chain(std::iter::once(&ids[0])) {
            t.send(Msg::SwitchProfile("__next".into()));
            assert_eq!(&t.state().active_profile_id, expected);
        }
        t.send(Msg::SwitchProfile("__prev".into()));
        assert_eq!(&t.state().active_profile_id, ids.last().unwrap(), "prev from the first wraps to the last");
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

    /// A sink that blocks (Tauri tray updates wait for the main thread, which may itself be
    /// waiting for this reply) must not delay the reply: the editor showed "Jádro programu neodpovídá".
    #[test]
    fn set_config_replies_before_notifying_a_blocking_sink() {
        struct BlockingSink(Arc<(Mutex<bool>, std::sync::Condvar)>);
        impl EventSink for BlockingSink {
            fn state(&self, _: &State) {
                let (lock, cv) = &*self.0;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = cv.wait(released).unwrap();
                }
            }
            fn config(&self, _: &Config) {}
            fn key(&self, _: u8, _: bool) {}
            fn raw(&self, _: u8, _: bool) {}
        }
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let dir = std::env::temp_dir().join(format!("deck-engine-{}", new_id()));
        let store = Store::new(dir);
        store.save(&seed()).unwrap();
        let (dtx, _drx) = channel();
        let (tx, _h) = spawn_engine(EngineDeps {
            store,
            device_tx: dtx,
            keyboard: Arc::new(FakeKb(Rec::default())),
            launch: Arc::new(|_: &str, _: &[String]| {}),
            system: Arc::new(|_: &str| {}),
            sink: Box::new(BlockingSink(gate.clone())),
            render,
            render_plugin,
            render_usage: |_| Vec::new(),
            usage_wake: None,
            sound_tx: None,
            host_tx: None,
        });
        let (r, rx) = channel();
        tx.send(Msg::SetConfig(serde_json::to_value(seed()).unwrap(), r)).unwrap();
        let reply = rx.recv_timeout(Duration::from_secs(2));
        let (lock, cv) = &*gate;
        *lock.lock().unwrap() = true;
        cv.notify_all();
        assert!(reply.expect("engine did not reply while the sink was blocked").is_ok());
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

#[cfg(test)]
mod sound_tests {
    use super::*;
    use crate::sound::{Mode, SoundCmd};
    use std::sync::mpsc::Receiver;
    use serde_json::json;

    const A: &str = "0123456789abcdef.wav";
    const B: &str = "fedcba9876543210.mp3";
    struct Kb;
    impl Keyboard for Kb {
        fn down(&self, _: &str) {}
        fn up(&self, _: &str) {}
        fn tap_vk(&self, _: u16) {}
        fn type_text(&self, _: &str) {}
    }
    struct Sink(Arc<Mutex<Vec<State>>>);
    impl EventSink for Sink {
        fn state(&self, s: &State) { self.0.lock().unwrap().push(s.clone()); }
        fn config(&self, _: &Config) {}
        fn key(&self, _: u8, _: bool) {}
        fn raw(&self, _: u8, _: bool) {}
    }
    fn action(sound: &str, mode: &str) -> Action {
        Action::Sound { sound: sound.into(), name: "boom.wav".into(), me: 63, mic: 29, mode: mode.into() }
    }
    struct Rig {
        e: Engine,
        sounds: Receiver<SoundCmd>,
        dev: Receiver<DeviceCmd>,
        states: Arc<Mutex<Vec<State>>>,
    }
    impl Rig {
        fn new() -> Self {
            let store = Store::new(std::env::temp_dir().join(format!("deck-sound-{}", crate::config::new_id())));
            let cfg = normalize_config(&json!({
                "defaultProfileId":"A", "activeProfileId":"A",
                "profiles":[{"id":"A","name":"A","keys":{}},{"id":"B","name":"B","keys":{}}]
            }));
            store.save(&cfg).unwrap();
            let (stx, sounds) = channel();
            let (dtx, dev) = channel();
            let states = Arc::new(Mutex::new(Vec::new()));
            let e = Engine::new(EngineDeps {
                store, device_tx: dtx, keyboard: Arc::new(Kb),
                launch: Arc::new(|_, _| {}), system: Arc::new(|_| {}),
                sink: Box::new(Sink(states.clone())),
                render: |s| vec![u8::from(s.playing)], render_plugin: |_| vec![], render_usage: |_| vec![],
                usage_wake: None, host_tx: None, sound_tx: Some(stx),
            });
            assert_eq!(sounds.try_recv().unwrap(), SoundCmd::SetDevices { me: "".into(), mic: "auto".into() });
            while dev.try_recv().is_ok() {}
            Self { e, sounds, dev, states }
        }
        fn set(&mut self, key: u8, a: Action) { self.e.config.profiles[0].keys.get_mut(&key.to_string()).unwrap().action = a; }
        fn commands(&self) -> Vec<SoundCmd> { self.sounds.try_iter().collect() }
        fn images(&self) -> Vec<(u8, Vec<u8>)> {
            self.dev.try_iter().flat_map(|c| match c { DeviceCmd::SetImages(v) => v, _ => vec![] }).collect()
        }
    }
    #[test]
    fn sound_press_modes_volumes_stop_all_and_empty() {
        let mut r = Rig::new();
        for (mode, expected) in [("toggle", Mode::Toggle), ("restart", Mode::Restart), ("hold", Mode::Hold)] {
            r.set(1, action(A, mode));
            r.e.on_key(1, true);
            assert_eq!(r.commands(), vec![SoundCmd::Press { sound: A.into(), me: 63, mic: 29, mode: expected }]);
            r.e.on_key(1, false);
            assert_eq!(r.commands(), if mode == "hold" { vec![SoundCmd::Stop { sound: A.into() }] } else { vec![] });
        }
        r.set(1, action("", "toggle"));
        r.e.on_key(1, true);
        assert!(r.commands().is_empty());
        r.set(1, action("", "stopAll"));
        r.e.on_key(1, true);
        assert_eq!(r.commands(), vec![SoundCmd::StopAll]);
    }
    #[test]
    fn hold_release_uses_original_sound_across_profile_and_action_changes() {
        let mut r = Rig::new();
        r.set(1, action(A, "hold"));
        // A plugin on the new profile must not intercept release of the original sound.
        r.e.config.profiles[1].keys.get_mut("1").unwrap().action = Action::Plugin {
            plugin: "com.x".into(), action: "com.x.act".into(), settings: json!({}), state: 0
        };
        r.e.on_key(1, true);
        r.e.on_key(1, true); // duplicate key-down does not restart a held sound
        assert_eq!(r.commands().len(), 1);
        r.set(1, action(B, "hold"));
        r.e.set_active("B".into());
        r.e.on_key(1, false);
        assert_eq!(r.commands(), vec![SoundCmd::Stop { sound: A.into() }]);
        r.e.on_key(1, false);
        assert!(r.commands().is_empty());
    }
    #[test]
    fn disconnect_calibration_and_shutdown_release_held_sounds() {
        for msg in [Msg::Device(DeviceEvent::Disconnected), Msg::SetCalibrating(true), Msg::Shutdown] {
            let mut r = Rig::new();
            r.set(1, action(A, "hold"));
            r.e.on_key(1, true);
            r.commands();
            r.e.handle(msg);
            assert!(r.commands().contains(&SoundCmd::Stop { sound: A.into() }));
            assert!(r.e.held_sounds.is_empty());
        }
    }
    #[test]
    fn output_changes_send_devices_only_when_changed() {
        let mut r = Rig::new();
        let mut raw = serde_json::to_value(&r.e.config).unwrap();
        r.e.set_config(&raw).unwrap();
        assert!(r.commands().is_empty());
        raw["soundMeDevice"] = json!("Headphones");
        r.e.set_config(&raw).unwrap();
        assert_eq!(r.commands(), vec![SoundCmd::SetDevices { me: "Headphones".into(), mic: "auto".into() }]);
        raw["soundMicDevice"] = json!("CABLE Input");
        r.e.set_config(&raw).unwrap();
        assert_eq!(r.commands(), vec![SoundCmd::SetDevices { me: "Headphones".into(), mic: "CABLE Input".into() }]);
        r.e.set_config(&raw).unwrap();
        assert!(r.commands().is_empty());
    }
    #[test]
    fn playing_events_update_state_and_only_matching_tiles() {
        let mut r = Rig::new();
        r.set(1, action(A, "toggle"));
        r.set(2, action(A, "hold"));
        r.set(3, action(B, "restart"));
        r.set(4, action(A, "stopAll"));
        r.e.handle(Msg::Device(DeviceEvent::Connected));
        r.images();
        r.e.handle(Msg::SoundPlaying { sound: A.into(), playing: true });
        assert_eq!(r.images(), vec![(1, vec![1]), (2, vec![1])]);
        assert!(r.states.lock().unwrap().last().unwrap().playing_sounds.contains(A));
        r.e.handle(Msg::SoundPlaying { sound: A.into(), playing: true });
        assert!(r.images().is_empty());
        r.e.handle(Msg::SoundPlaying { sound: A.into(), playing: false });
        assert_eq!(r.images(), vec![(1, vec![0]), (2, vec![0])]);
        assert!(!r.states.lock().unwrap().last().unwrap().playing_sounds.contains(A));
    }
}
