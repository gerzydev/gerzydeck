//! Manifest parsing and confined plugin asset resolution.
use serde::{Deserialize, Deserializer};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Manifest {
    pub name: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "UUID")]
    pub uuid: Option<String>,
    #[serde(default)]
    pub icon: String,
    pub category: Option<String>,
    pub category_icon: Option<String>,
    pub code_path: Option<String>,
    pub code_path_win: Option<String>,
    pub nodejs: Option<NodeSpec>,
    pub property_inspector_path: Option<String>,
    pub actions: Vec<ActionDef>,
    #[serde(
        default,
        rename = "ApplicationsToMonitor",
        deserialize_with = "windows_apps"
    )]
    pub applications_to_monitor_windows: Vec<String>,
    #[serde(default, rename = "OS")]
    pub os: Vec<OsSpec>,
    #[serde(default, rename = "SDKVersion")]
    pub sdk_version: u32,
    #[serde(default, rename = "Software", deserialize_with = "software_version")]
    pub software_min: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NodeSpec {
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct OsSpec {
    pub platform: String,
    #[serde(default)]
    pub minimum_version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ActionDef {
    #[serde(rename = "UUID")]
    pub uuid: String,
    pub name: String,
    #[serde(default)]
    pub tooltip: String,
    #[serde(default)]
    pub icon: String,
    pub property_inspector_path: Option<String>,
    pub states: Vec<StateDef>,
    #[serde(default = "keypad")]
    pub controllers: Vec<String>,
    #[serde(default = "yes")]
    pub visible_in_action_list: bool,
    #[serde(default)]
    pub supported_in_multi_actions: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StateDef {
    #[serde(default)]
    pub image: String,
    pub title: Option<String>,
    pub show_title: Option<bool>,
    pub title_alignment: Option<String>,
    #[serde(default, deserialize_with = "font_size")]
    pub font_size: Option<u32>,
    pub title_color: Option<String>,
    pub multi_action_image: Option<String>,
}
fn keypad() -> Vec<String> {
    vec!["Keypad".into()]
}
fn yes() -> bool {
    true
}
fn font_size<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Size {
        Number(u32),
        Text(String),
    }
    Option::<Size>::deserialize(d)?
        .map(|s| match s {
            Size::Number(n) => Ok(n),
            Size::Text(s) => s.parse().map_err(serde::de::Error::custom),
        })
        .transpose()
}
fn windows_apps<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    struct Apps {
        #[serde(default)]
        windows: Vec<String>,
    }
    Ok(Apps::deserialize(d)?.windows)
}
fn software_version<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    struct Software {
        #[serde(rename = "MinimumVersion")]
        minimum_version: Option<String>,
    }
    Ok(Software::deserialize(d)?.minimum_version)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Runtime {
    Exe(PathBuf),
    Node { script: PathBuf, version: String },
    Html(PathBuf),
}

#[derive(Debug)]
pub struct ManifestError(String);
impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ManifestError {}

impl Manifest {
    pub fn load(plugin_dir: &Path) -> Result<Self, ManifestError> {
        let file = confined_file(plugin_dir, "manifest.json")
            .ok_or_else(|| ManifestError("Manifest chybí nebo je mimo složku pluginu".into()))?;
        let json = fs::read_to_string(file)
            .map_err(|e| ManifestError(format!("Nelze přečíst manifest: {e}")))?;
        let m: Self = serde_json::from_str(json.trim_start_matches('\u{feff}'))
            .map_err(|e| ManifestError(format!("Neplatný manifest: {e}")))?;
        if m.actions.is_empty()
            || m.actions
                .iter()
                .any(|a| a.states.is_empty() || a.uuid.is_empty() || a.name.is_empty())
        {
            return Err(ManifestError(
                "Manifest musí obsahovat akce s názvem, UUID a alespoň jedním stavem".into(),
            ));
        }
        Ok(m)
    }

    pub fn runtime(&self, plugin_dir: &Path) -> Result<Runtime, ManifestError> {
        let path = self
            .code_path_win
            .as_deref()
            .or(self.code_path.as_deref())
            .ok_or_else(|| ManifestError("Manifest neobsahuje cestu k programu pluginu".into()))?;
        let resolved = confined_file(plugin_dir, path).ok_or_else(|| {
            ManifestError("Program pluginu chybí nebo je mimo jeho složku".into())
        })?;
        let ext = resolved
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if self.nodejs.is_some() || matches!(ext.as_str(), "js" | "mjs" | "cjs") {
            Ok(Runtime::Node {
                script: resolved,
                version: self
                    .nodejs
                    .as_ref()
                    .map(|n| n.version.clone())
                    .unwrap_or_default(),
            })
        } else if ext == "html" {
            Ok(Runtime::Html(resolved))
        } else {
            Ok(Runtime::Exe(resolved))
        }
    }

    pub fn keypad_actions(&self) -> impl Iterator<Item = &ActionDef> {
        self.actions
            .iter()
            .filter(|a| a.visible_in_action_list && a.controllers.iter().any(|c| c == "Keypad"))
    }
}

/// Resolve only regular files whose canonical target remains within the plugin.
fn confined_file(plugin_dir: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty() || relative.contains(':') || relative.starts_with(['/', '\\']) {
        return None;
    }
    let normalized = relative.replace('\\', "/");
    let path = Path::new(&normalized);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    let root = plugin_dir.canonicalize().ok()?;
    let resolved = root.join(path).canonicalize().ok()?;
    (resolved.starts_with(&root) && resolved.is_file()).then_some(resolved)
}

pub fn resolve_image(plugin_dir: &Path, base: &str) -> Option<PathBuf> {
    [
        format!("{base}@2x.png"),
        format!("{base}.png"),
        format!("{base}.svg"),
        base.to_owned(),
    ]
    .iter()
    .find_map(|p| confined_file(plugin_dir, p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    fn dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gd-manifest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
    const MINIMAL: &str = r#"{"Name":"Test","Actions":[{"UUID":"test.action","Name":"Action","States":[{"Image":"icon"}]}]}"#;
    fn load_json(json: &str) -> (Manifest, PathBuf) {
        let d = dir();
        fs::write(d.join("manifest.json"), json).unwrap();
        (Manifest::load(&d).unwrap(), d)
    }
    #[test]
    fn bom_unknown_fields_and_defaults() {
        let (m, d) = load_json(&format!("\u{feff}{}", MINIMAL.replace("\"Name\":\"Test\"", "\"Name\":\"Test\",\"FutureField\":true,\"ApplicationsToMonitor\":{\"windows\":[\"obs.exe\"]},\"OS\":[{\"Platform\":\"windows\",\"MinimumVersion\":\"10\"}]")));
        assert_eq!(m.name.as_deref(), Some("Test"));
        assert_eq!(m.applications_to_monitor_windows, ["obs.exe"]);
        assert_eq!(m.os[0].minimum_version, "10");
        assert_eq!(m.keypad_actions().count(), 1);
        assert!(m.actions[0].visible_in_action_list);
        fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn rejects_missing_actions_and_empty_states() {
        for json in [
            r#"{"Name":"No actions"}"#,
            r#"{"Actions":[{"UUID":"a","Name":"a","States":[]}]}"#,
        ] {
            let d = dir();
            fs::write(d.join("manifest.json"), json).unwrap();
            assert!(Manifest::load(&d).is_err());
            fs::remove_dir_all(d).unwrap();
        }
    }
    #[test]
    fn runtime_precedence_and_types() {
        for (extra, file, kind) in [
            (
                r#", "CodePath":"other.html", "CodePathWin":"win.exe""#,
                "win.exe",
                "exe",
            ),
            (
                r#", "CodePath":"main.mjs", "Nodejs":{"Version":"20"}"#,
                "main.mjs",
                "node",
            ),
            (r#", "CodePath":"main.html""#, "main.html", "html"),
            (r#", "CodePath":"main.cjs""#, "main.cjs", "node"),
        ] {
            let (m, d) = load_json(&format!("{}{} }}", &MINIMAL[..MINIMAL.len() - 1], extra));
            fs::write(d.join(file), "").unwrap();
            let r = m.runtime(&d).unwrap();
            assert!(matches!(
                (&r, kind),
                (Runtime::Exe(_), "exe")
                    | (Runtime::Node { .. }, "node")
                    | (Runtime::Html(_), "html")
            ));
            if let Runtime::Node { version, .. } = r {
                assert_eq!(version, if file == "main.mjs" { "20" } else { "" });
            }
            fs::remove_dir_all(d).unwrap();
        }
    }
    #[test]
    fn image_order_and_traversal() {
        let d = dir();
        fs::write(d.join("icon.png"), "").unwrap();
        fs::write(d.join("icon@2x.png"), "").unwrap();
        assert_eq!(
            resolve_image(&d, "icon"),
            Some(d.join("icon@2x.png").canonicalize().unwrap())
        );
        for path in ["../../x", "..\\..\\x", "C:\\Windows\\x", "/x"] {
            assert!(resolve_image(&d, path).is_none());
        }
        fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn runtime_rejects_escape_and_missing_file() {
        let (mut m, d) = load_json(MINIMAL);
        m.code_path = Some("../evil.exe".into());
        assert!(m.runtime(&d).is_err());
        m.code_path = Some("missing.exe".into());
        assert!(m.runtime(&d).is_err());
        fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn every_real_fixture_parses_has_keypad_and_expected_runtime() {
        for (id, kind, version) in [
            ("com.elgato.obsstudio", "html", ""),
            ("com.elgato.spotify", "exe", ""),
            ("com.elgato.discord", "exe", ""),
            ("com.elgato.cpu", "exe", ""),
            ("me.amankhanna.oadiscord", "exe", ""),
            ("me.amankhanna.oasystem", "exe", ""),
            ("com.ntanis.essentials-for-spotify", "node", "20"),
            ("de.felitendo.obs-tools", "exe", ""),
        ] {
            let d = dir();
            fs::copy(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("tests/fixtures/plugins/{id}.json")),
                d.join("manifest.json"),
            )
            .unwrap();
            let m = Manifest::load(&d).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(m.keypad_actions().count() > 0, "{id}");
            let code = m.code_path_win.as_ref().or(m.code_path.as_ref()).unwrap();
            let code_path = d.join(code);
            fs::create_dir_all(code_path.parent().unwrap()).unwrap();
            fs::write(code_path, "fixture runtime placeholder").unwrap();
            let r = m.runtime(&d).unwrap();
            assert!(
                matches!(
                    (&r, kind),
                    (Runtime::Exe(_), "exe")
                        | (Runtime::Html(_), "html")
                        | (Runtime::Node { .. }, "node")
                ),
                "{id}: {r:?}"
            );
            if let Runtime::Node { version: v, .. } = r {
                assert_eq!(v, version, "{id}");
            }
            fs::remove_dir_all(d).unwrap();
        }
    }
}
