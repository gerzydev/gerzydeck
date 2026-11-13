//! Persistence: config.json (atomic writes) + content-addressed key images.

use crate::config::{create_default_config, normalize_config, Config};
use sha2::{Digest, Sha256};
use std::io;
use std::path::{Path, PathBuf};

/// `%APPDATA%\GerzysDeck`, overridable with `GERZYS_DECK_DIR`.
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("GERZYS_DECK_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("GerzysDeck")
}

fn is_image_id(id: &str) -> bool {
    id.len() == 20 && id.ends_with(".png") && id[..16].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn config_file(&self) -> PathBuf {
        self.dir.join("config.json")
    }

    fn images_dir(&self) -> PathBuf {
        self.dir.join("images")
    }

    fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, path)
    }

    pub fn save(&self, cfg: &Config) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(cfg).map_err(io::Error::other)?;
        Self::write_atomic(&self.config_file(), &json)
    }

    /// Never fails: missing → default (saved). Anything unreadable or unusable is first copied
    /// to `config.broken-<ms>.json`, so the user's profiles are never silently overwritten.
    pub fn load(&self) -> Config {
        let path = self.config_file();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let cfg = create_default_config();
                if let Err(e) = self.save(&cfg) {
                    log::error!("saving default config failed: {e}");
                }
                return cfg;
            }
            Err(e) => return self.recover(&format!("cannot read config.json: {e}")),
        };
        let value = match std::str::from_utf8(&bytes).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<serde_json::Value>(t).map_err(|e| e.to_string())) {
            Ok(v) => v,
            Err(e) => return self.recover(&format!("config.json is corrupt: {e}")),
        };
        if !value.get("profiles").is_some_and(|p| p.as_array().is_some_and(|a| !a.is_empty())) {
            return self.recover("config.json has no profiles");
        }
        normalize_config(&value)
    }

    fn recover(&self, why: &str) -> Config {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let backup = self.dir.join(format!("config.broken-{ms}.json"));
        match std::fs::copy(self.config_file(), &backup) {
            Ok(_) => {
                log::error!("{why}; backed up to {}", backup.display());
                let cfg = create_default_config();
                if let Err(e) = self.save(&cfg) {
                    log::error!("saving default config failed: {e}");
                }
                cfg
            }
            Err(e) => {
                // cannot even back it up: use defaults in memory, leave the file alone
                log::error!("{why}; backup failed ({e}), not touching config.json");
                create_default_config()
            }
        }
    }

    pub fn save_image(&self, png: &[u8]) -> io::Result<String> {
        let hash = Sha256::digest(png);
        let hex: String = hash.iter().take(8).map(|b| format!("{b:02x}")).collect();
        let id = format!("{hex}.png");
        Self::write_atomic(&self.images_dir().join(&id), png)?;
        Ok(id)
    }

    pub fn read_image(&self, id: &str) -> Option<Vec<u8>> {
        if !is_image_id(id) {
            return None;
        }
        std::fs::read(self.images_dir().join(id)).ok()
    }

    pub fn prune_images(&self, cfg: &Config) {
        let used: std::collections::HashSet<&str> = cfg
            .profiles
            .iter()
            .flat_map(|p| p.keys.values().filter_map(|k| k.image.as_deref()))
            .collect();
        let Ok(entries) = std::fs::read_dir(self.images_dir()) else { return };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if is_image_id(&name) && !used.contains(name.as_str()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    /// Deletes stored sounds no key uses any more.
    pub fn prune_sounds(&self, cfg: &Config) {
        let used: std::collections::HashSet<&str> = cfg
            .profiles
            .iter()
            .flat_map(|p| p.keys.values())
            .filter_map(|k| match &k.action {
                crate::config::Action::Sound { sound, .. } => Some(sound.as_str()),
                _ => None,
            })
            .collect();
        let Ok(entries) = std::fs::read_dir(crate::sound::sounds_dir(&self.dir)) else { return };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if crate::sound::is_sound_id(&name) && !used.contains(name.as_str()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::new_id;

    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("deck-test-{}", new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_config_creates_default_on_disk() {
        let dir = tmp();
        let c = Store::new(dir.clone()).load();
        assert_eq!(c.profiles.len(), 1);
        let on_disk: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
        assert_eq!(on_disk["version"], 1);
    }

    #[test]
    fn round_trip() {
        let s = Store::new(tmp());
        let mut c = s.load();
        c.profiles[0].name = "WoW".into();
        s.save(&c).unwrap();
        assert_eq!(s.load().profiles[0].name, "WoW");
    }

    #[test]
    fn corrupt_config_is_backed_up() {
        let dir = tmp();
        std::fs::write(dir.join("config.json"), "{ nope").unwrap();
        let c = Store::new(dir.clone()).load();
        assert_eq!(c.profiles.len(), 1);
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().any(|n| n.starts_with("config.broken-") && n.ends_with(".json")), "{names:?}");
    }

    fn has_backup(dir: &std::path::Path) -> bool {
        std::fs::read_dir(dir).unwrap().any(|e| {
            let n = e.unwrap().file_name().to_string_lossy().into_owned();
            n.starts_with("config.broken-") && n.ends_with(".json")
        })
    }

    #[test]
    fn non_utf8_config_is_backed_up_not_silently_replaced() {
        let dir = tmp();
        std::fs::write(dir.join("config.json"), [0x7B, 0xFF, 0xFE, 0x7D]).unwrap();
        let _ = Store::new(dir.clone()).load();
        assert!(has_backup(&dir));
    }

    #[test]
    fn json_without_profiles_is_backed_up() {
        let dir = tmp();
        std::fs::write(dir.join("config.json"), r#"{"hello": 1}"#).unwrap();
        let _ = Store::new(dir.clone()).load();
        assert!(has_backup(&dir));
    }

    #[test]
    fn images_save_read_guard_prune() {
        let s = Store::new(tmp());
        let mut c = s.load();
        let keep = s.save_image(b"png-a").unwrap();
        let drop = s.save_image(b"png-b").unwrap();
        assert_eq!(keep.len(), 20);
        assert!(keep.ends_with(".png"));
        assert_eq!(s.read_image(&keep).as_deref(), Some(&b"png-a"[..]));
        assert_eq!(s.read_image("../../x"), None);
        assert_eq!(s.read_image("0123456789abcdef.png"), None);
        c.profiles[0].keys.get_mut("1").unwrap().image = Some(keep.clone());
        s.prune_images(&c);
        assert!(s.read_image(&keep).is_some());
        assert_eq!(s.read_image(&drop), None);
    }
}
