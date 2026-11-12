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

    /// Never fails: missing → default (saved), corrupt → backed up + default.
    pub fn load(&self) -> Config {
        let path = self.config_file();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => {
                let cfg = create_default_config();
                if let Err(e) = self.save(&cfg) {
                    log::error!("saving default config failed: {e}");
                }
                return cfg;
            }
        };
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => normalize_config(&v),
            Err(e) => {
                let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
                let backup = self.dir.join(format!("config.broken-{ms}.json"));
                log::error!("config.json is corrupt ({e}), moved to {}", backup.display());
                let _ = std::fs::rename(&path, backup);
                let cfg = create_default_config();
                let _ = self.save(&cfg);
                cfg
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
