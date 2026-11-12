//! Config shape, normalization (never fails) and the auto-switch decision.

use crate::scancodes::{media_vk, scancode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

pub const CONFIG_VERSION: u32 = 1;
pub const KEY_COUNT: u8 = 15;
pub const DEFAULT_BG: &str = "#141618";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub version: u32,
    pub brightness: u8,
    pub key_map: Option<Vec<u8>>,
    pub default_profile_id: String,
    pub active_profile_id: String,
    pub profiles: Vec<Profile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub processes: Vec<String>,
    /// "1".."15"
    pub keys: BTreeMap<String, KeyConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyConfig {
    pub image: Option<String>,
    pub label: String,
    pub bg: String,
    pub action: Action,
    /// plugin keys: where the live value goes ("top" | "middle" | "bottom"); None = as the plugin says
    #[serde(default)]
    pub title_align: Option<String>,
}

impl Default for KeyConfig {
    fn default() -> Self {
        Self { image: None, label: String::new(), bg: DEFAULT_BG.into(), action: Action::None, title_align: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    None,
    Keys { codes: Vec<String> },
    Macro { steps: Vec<Step> },
    Launch { target: String, args: Vec<String> },
    Media { command: String },
    System { command: String },
    Profile {
        #[serde(rename = "profileId")]
        profile_id: String,
    },
    /// Action of a Stream Deck SDK plugin. `settings`/`state` are owned by the core (plugins write them).
    Plugin { plugin: String, action: String, settings: Value, state: u8 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Step {
    Down { code: String },
    Up { code: String },
    Tap { code: String },
    Wait { ms: u32 },
    Text { text: String },
}

impl Profile {
    pub fn key(&self, k: u8) -> &KeyConfig {
        static EMPTY: std::sync::OnceLock<KeyConfig> = std::sync::OnceLock::new();
        self.keys.get(&k.to_string()).unwrap_or_else(|| EMPTY.get_or_init(KeyConfig::default))
    }
}

/// Short random id like "p1a2b3c4d5e".
pub fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    format!("p{:010x}", h.finish() & 0xFF_FFFF_FFFF)
}

fn empty_keys() -> BTreeMap<String, KeyConfig> {
    (1..=KEY_COUNT).map(|k| (k.to_string(), KeyConfig::default())).collect()
}

pub fn create_profile(name: &str) -> Profile {
    Profile { id: new_id(), name: name.into(), processes: vec![], keys: empty_keys() }
}

pub fn create_default_config() -> Config {
    let p = create_profile("Výchozí");
    Config {
        version: CONFIG_VERSION,
        brightness: 80,
        key_map: None,
        default_profile_id: p.id.clone(),
        active_profile_id: p.id.clone(),
        profiles: vec![p],
    }
}

fn s(v: &Value, max: usize) -> String {
    v.as_str().map(|x| x.chars().take(max).collect()).unwrap_or_default()
}

fn valid_code(v: &Value) -> Option<String> {
    v.as_str().filter(|c| scancode(c).is_some()).map(String::from)
}

fn normalize_step(v: &Value) -> Option<Step> {
    let code = || valid_code(&v["code"]);
    Some(match v["type"].as_str()? {
        "down" => Step::Down { code: code()? },
        "up" => Step::Up { code: code()? },
        "tap" => Step::Tap { code: code()? },
        "wait" => {
            let ms = v["ms"].as_u64().filter(|&m| m <= 60_000)?;
            Step::Wait { ms: ms as u32 }
        }
        "text" => {
            let text = s(&v["text"], 1000);
            if text.is_empty() {
                return None;
            }
            Step::Text { text }
        }
        _ => return None,
    })
}

pub fn normalize_action(v: &Value) -> Action {
    let parsed = (|| {
        Some(match v["type"].as_str()? {
            "keys" => {
                let arr = v["codes"].as_array()?;
                if arr.is_empty() || arr.len() > 6 {
                    return None;
                }
                let mut codes: Vec<String> = Vec::new();
                for c in arr {
                    let c = valid_code(c)?;
                    if !codes.contains(&c) {
                        codes.push(c);
                    }
                }
                Action::Keys { codes }
            }
            "macro" => {
                let arr = v["steps"].as_array()?;
                if arr.is_empty() || arr.len() > 200 {
                    return None;
                }
                // drop invalid steps one by one (e.g. an empty text step while the user is typing)
                let steps: Vec<Step> = arr.iter().filter_map(normalize_step).collect();
                if steps.is_empty() {
                    return None;
                }
                Action::Macro { steps }
            }
            "launch" => {
                let target = s(&v["target"], 1000).trim().to_string();
                if target.is_empty() {
                    return None;
                }
                let args = v["args"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).take(50).collect()).unwrap_or_default();
                Action::Launch { target, args }
            }
            "media" => {
                let command = s(&v["command"], 20);
                media_vk(&command)?;
                Action::Media { command }
            }
            "system" => {
                let command = s(&v["command"], 20);
                if !matches!(command.as_str(), "lock" | "sleep") {
                    return None;
                }
                Action::System { command }
            }
            "plugin" => {
                let plugin = s(&v["plugin"], 200);
                let action = s(&v["action"], 200);
                if plugin.is_empty() || action.is_empty() {
                    return None;
                }
                let settings = if v["settings"].is_object() { v["settings"].clone() } else { serde_json::json!({}) };
                let state = v["state"].as_u64().filter(|&n| n < 16).unwrap_or(0) as u8;
                Action::Plugin { plugin, action, settings, state }
            }
            "profile" => {
                let profile_id = s(&v["profileId"], 64);
                if profile_id.is_empty() {
                    return None;
                }
                Action::Profile { profile_id }
            }
            _ => return None,
        })
    })();
    parsed.unwrap_or(Action::None)
}

fn is_image_id(s: &str) -> bool {
    s.len() == 20 && s.ends_with(".png") && s[..16].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn normalize_key(v: &Value) -> KeyConfig {
    if !v.is_object() {
        return KeyConfig::default();
    }
    let bg = v["bg"].as_str().filter(|b| b.len() == 7 && b.starts_with('#') && b[1..].chars().all(|c| c.is_ascii_hexdigit()));
    KeyConfig {
        image: v["image"].as_str().filter(|i| is_image_id(i)).map(String::from),
        label: s(&v["label"], 40),
        bg: bg.map(|b| b.to_ascii_lowercase()).unwrap_or_else(|| DEFAULT_BG.into()),
        action: normalize_action(&v["action"]),
        title_align: v["titleAlign"].as_str().filter(|a| matches!(*a, "top" | "middle" | "bottom")).map(String::from),
    }
}

fn normalize_profile(v: &Value, used: &mut HashSet<String>) -> Profile {
    let mut id = s(&v["id"], 64);
    if id.is_empty() || used.contains(&id) {
        id = new_id();
    }
    used.insert(id.clone());
    let keys = (1..=KEY_COUNT).map(|k| (k.to_string(), normalize_key(&v["keys"][k.to_string()]))).collect();
    let mut processes: Vec<String> = Vec::new();
    for p in v["processes"].as_array().into_iter().flatten() {
        if let Some(p) = p.as_str() {
            let p = p.trim().to_lowercase();
            if !p.is_empty() && !processes.contains(&p) {
                processes.push(p);
            }
        }
    }
    let name = s(&v["name"], 60).trim().to_string();
    Profile { id, name: if name.is_empty() { "Profil".into() } else { name }, processes, keys }
}

fn is_permutation(v: &Value) -> Option<Vec<u8>> {
    let arr = v.as_array()?;
    let mut nums: Vec<u8> = arr.iter().map(|x| x.as_u64().filter(|&n| n <= 255).map(|n| n as u8)).collect::<Option<_>>()?;
    let original = nums.clone();
    nums.sort_unstable();
    (nums == (1..=KEY_COUNT).collect::<Vec<u8>>()).then_some(original)
}

/// Never fails: anything invalid degrades to defaults.
pub fn normalize_config(raw: &Value) -> Config {
    let Some(list) = raw.get("profiles").and_then(|p| p.as_array()) else {
        return create_default_config();
    };
    let mut used = HashSet::new();
    let profiles: Vec<Profile> = list.iter().filter(|p| p.is_object()).map(|p| normalize_profile(p, &mut used)).collect();
    if profiles.is_empty() {
        return create_default_config();
    }
    let exists = |id: &str| profiles.iter().any(|p| p.id == id);
    let default_profile_id = raw["defaultProfileId"].as_str().filter(|id| exists(id)).map(String::from).unwrap_or_else(|| profiles[0].id.clone());
    let active_profile_id = raw["activeProfileId"].as_str().filter(|id| exists(id)).map(String::from).unwrap_or_else(|| default_profile_id.clone());
    let brightness = raw["brightness"].as_f64().map(|b| b.round().clamp(0.0, 100.0) as u8).unwrap_or(80);
    Config {
        version: CONFIG_VERSION,
        brightness,
        key_map: is_permutation(&raw["keyMap"]),
        default_profile_id,
        active_profile_id,
        profiles,
    }
}

/// Which profile should be active for the given foreground exe.
pub fn decide_profile(config: &Config, exe: Option<&str>, manual: &str) -> String {
    if let Some(exe) = exe {
        let exe = exe.to_lowercase();
        if let Some(p) = config.profiles.iter().find(|p| p.processes.contains(&exe)) {
            return p.id.clone();
        }
    }
    if config.profiles.iter().any(|p| p.id == manual) {
        manual.to_string()
    } else {
        config.default_profile_id.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(id: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut p = json!({ "id": id, "name": id, "processes": [], "keys": {} });
        if let (Some(p), Some(e)) = (p.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                p.insert(k.clone(), v.clone());
            }
        }
        p
    }

    #[test]
    fn default_config_is_valid() {
        let c = create_default_config();
        assert_eq!(c.version, 1);
        assert_eq!(c.profiles.len(), 1);
        assert_eq!(c.profiles[0].name, "Výchozí");
        assert_eq!(c.profiles[0].keys.len(), 15);
        assert_eq!(c.profiles[0].key(7), &KeyConfig::default());
        assert_eq!(c.profiles[0].key(7).bg, "#141618");
        assert_eq!(c.active_profile_id, c.profiles[0].id);
        assert_eq!(normalize_config(&serde_json::to_value(&c).unwrap()), c);
    }

    #[test]
    fn json_wire_format_is_camel_case() {
        let c = create_default_config();
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("activeProfileId").is_some());
        assert!(v.get("keyMap").is_some());
        assert_eq!(v["profiles"][0]["keys"]["1"]["action"], json!({ "type": "none" }));
        let p = Action::Profile { profile_id: "x".into() };
        assert_eq!(serde_json::to_value(&p).unwrap(), json!({ "type": "profile", "profileId": "x" }));
    }

    #[test]
    fn garbage_becomes_default() {
        for raw in [json!(null), json!(42), json!("x"), json!({ "profiles": "nope" }), json!({ "profiles": [] })] {
            let c = normalize_config(&raw);
            assert_eq!(c.profiles.len(), 1);
            assert_eq!(c.default_profile_id, c.profiles[0].id);
        }
    }

    #[test]
    fn invalid_actions_become_none() {
        let keys = json!({
            "1": { "action": { "type": "explode" } },
            "2": { "action": { "type": "keys", "codes": ["KeyQ", "Bogus"] } },
            "3": { "action": { "type": "keys", "codes": ["ShiftLeft", "KeyQ"] } },
            "4": { "action": { "type": "macro", "steps": [{ "type": "wait", "ms": 999999 }] } },
            "5": { "action": { "type": "macro", "steps": [{ "type": "tap", "code": "KeyQ" }, { "type": "wait", "ms": 50 }, { "type": "text", "text": "ahoj" }] } },
            "6": { "action": { "type": "media", "command": "volup" } },
            "7": { "action": { "type": "system", "command": "format-c" } },
            "8": { "action": { "type": "system", "command": "lock" } },
            "9": { "action": { "type": "launch", "target": "notepad.exe" } },
            "10": { "action": { "type": "keys", "codes": [] } }
        });
        let c = normalize_config(&json!({ "profiles": [profile("a", json!({ "keys": keys }))] }));
        let p = &c.profiles[0];
        assert_eq!(p.key(1).action, Action::None);
        assert_eq!(p.key(2).action, Action::None);
        assert_eq!(p.key(3).action, Action::Keys { codes: vec!["ShiftLeft".into(), "KeyQ".into()] });
        assert_eq!(p.key(4).action, Action::None);
        assert!(matches!(&p.key(5).action, Action::Macro { steps } if steps.len() == 3));
        assert_eq!(p.key(6).action, Action::Media { command: "volup".into() });
        assert_eq!(p.key(7).action, Action::None);
        assert_eq!(p.key(8).action, Action::System { command: "lock".into() });
        assert_eq!(p.key(9).action, Action::Launch { target: "notepad.exe".into(), args: vec![] });
        assert_eq!(p.key(10).action, Action::None);
        assert_eq!(p.key(11).action, Action::None);
    }

    /// The editor creates an empty "Napsat text" step while typing; it must not wipe the whole macro.
    #[test]
    fn invalid_macro_steps_are_dropped_individually() {
        let a = normalize_action(&json!({ "type": "macro", "steps": [
            { "type": "tap", "code": "KeyQ" }, { "type": "text", "text": "" }, { "type": "wait", "ms": 50 }
        ] }));
        assert_eq!(a, Action::Macro { steps: vec![Step::Tap { code: "KeyQ".into() }, Step::Wait { ms: 50 }] });
    }

    #[test]
    fn plugin_actions_normalize() {
        let ok = normalize_action(&json!({ "type": "plugin", "plugin": "com.elgato.obsstudio", "action": "com.elgato.obsstudio.record", "settings": { "a": 1 }, "state": 1 }));
        assert_eq!(ok, Action::Plugin { plugin: "com.elgato.obsstudio".into(), action: "com.elgato.obsstudio.record".into(), settings: json!({ "a": 1 }), state: 1 });
        let defaults = normalize_action(&json!({ "type": "plugin", "plugin": "p", "action": "a" }));
        assert_eq!(defaults, Action::Plugin { plugin: "p".into(), action: "a".into(), settings: json!({}), state: 0 });
        let bad_settings = normalize_action(&json!({ "type": "plugin", "plugin": "p", "action": "a", "settings": [1, 2], "state": 900 }));
        assert_eq!(bad_settings, Action::Plugin { plugin: "p".into(), action: "a".into(), settings: json!({}), state: 0 });
        assert_eq!(normalize_action(&json!({ "type": "plugin", "plugin": "", "action": "a" })), Action::None);
        assert_eq!(normalize_action(&json!({ "type": "plugin", "plugin": "p" })), Action::None);
        let v = serde_json::to_value(&ok).unwrap();
        assert_eq!(v["type"], "plugin");
        assert_eq!(v["settings"]["a"], 1);
    }

    #[test]
    fn title_align_is_optional_and_validated() {
        let c = normalize_config(&json!({ "profiles": [profile("a", json!({ "keys": {
            "1": { "titleAlign": "middle" }, "2": { "titleAlign": "sideways" }, "3": {}
        } }))] }));
        assert_eq!(c.profiles[0].key(1).title_align.as_deref(), Some("middle"));
        assert_eq!(c.profiles[0].key(2).title_align, None);
        assert_eq!(c.profiles[0].key(3).title_align, None);
        assert_eq!(serde_json::to_value(c.profiles[0].key(1)).unwrap()["titleAlign"], "middle");
    }

    #[test]
    fn repairs_ids_processes_brightness_keymap() {
        let c = normalize_config(&json!({
            "activeProfileId": "gone", "defaultProfileId": "gone", "brightness": 300, "keyMap": [1, 1, 1],
            "profiles": [profile("a", json!({ "processes": ["Wow.exe", " WOW.EXE "] })), profile("b", json!({}))]
        }));
        assert_eq!(c.active_profile_id, "a");
        assert_eq!(c.default_profile_id, "a");
        assert_eq!(c.profiles[0].processes, vec!["wow.exe".to_string()]);
        assert_eq!(c.brightness, 100);
        assert_eq!(c.key_map, None);
        let ok: Vec<u8> = (1..=15).rev().collect();
        assert_eq!(normalize_config(&json!({ "keyMap": ok, "profiles": [profile("a", json!({}))] })).key_map, Some(ok));
    }

    #[test]
    fn duplicate_ids_made_unique_and_dangling_target_kept() {
        let c = normalize_config(&json!({ "profiles": [profile("a", json!({})), profile("a", json!({}))] }));
        assert_ne!(c.profiles[0].id, c.profiles[1].id);
        let c = normalize_config(&json!({ "profiles": [profile("a", json!({ "keys": { "1": { "action": { "type": "profile", "profileId": "gone" } } } }))] }));
        assert_eq!(c.profiles[0].key(1).action, Action::Profile { profile_id: "gone".into() });
    }

    #[test]
    fn label_bg_image_validation() {
        let c = normalize_config(&json!({ "profiles": [profile("a", json!({ "keys": {
            "1": { "label": "x".repeat(100), "bg": "#ABCDEF", "image": "0123456789abcdef.png" },
            "2": { "bg": "red", "image": "../evil.png" }
        } }))] }));
        assert_eq!(c.profiles[0].key(1).label.chars().count(), 40);
        assert_eq!(c.profiles[0].key(1).bg, "#abcdef");
        assert_eq!(c.profiles[0].key(1).image.as_deref(), Some("0123456789abcdef.png"));
        assert_eq!(c.profiles[0].key(2).bg, "#141618");
        assert_eq!(c.profiles[0].key(2).image, None);
    }

    #[test]
    fn decide_profile_rules() {
        let c = normalize_config(&json!({
            "defaultProfileId": "def",
            "profiles": [profile("def", json!({})), profile("wow1", json!({ "processes": ["wow.exe"] })),
                         profile("wow2", json!({ "processes": ["wow.exe"] })), profile("manual", json!({}))]
        }));
        assert_eq!(decide_profile(&c, Some("Wow.exe"), "manual"), "wow1");
        assert_eq!(decide_profile(&c, Some("chrome.exe"), "manual"), "manual");
        assert_eq!(decide_profile(&c, None, "manual"), "manual");
        assert_eq!(decide_profile(&c, Some("chrome.exe"), "deleted"), "def");
    }
}
