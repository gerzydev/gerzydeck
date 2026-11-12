//! Icon library ("Moje ikony") and imported Stream Deck icon packs, served via `gdicons://`.

use crate::plugins::scheme::Served;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

pub const SCHEME: &str = "gdicons";
const MAX_PACK_BYTES: u64 = 200 * 1024 * 1024;
const MAX_PACK_ENTRIES: usize = 50_000;
const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "svg", "webp"];

#[derive(Debug, Clone, Serialize)]
pub struct LibraryIcon {
    pub id: String,
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IconPack {
    pub id: String,
    pub name: String,
    pub author: String,
    pub icon_url: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackIcon {
    pub name: String,
    pub tags: Vec<String>,
    pub url: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct PackManifest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    icon: Option<String>,
}

#[derive(Deserialize)]
struct PackEntry {
    path: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

pub struct IconStore {
    pub root: PathBuf,
}

fn err(m: impl Into<String>) -> String {
    m.into()
}

fn is_library_id(id: &str) -> bool {
    id.len() == 20 && id.ends_with(".png") && id[..16].bytes().all(|b| b.is_ascii_hexdigit())
}

fn url(rel: &str) -> String {
    format!("http://{SCHEME}.localhost/{}", rel.replace('\\', "/"))
}

fn is_image(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

impl IconStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn default_root() -> PathBuf {
        crate::store::data_dir().join("icons")
    }

    fn library_dir(&self) -> PathBuf {
        self.root.join("library")
    }
    fn library_meta(&self) -> PathBuf {
        self.root.join("library.json")
    }
    fn packs_dir(&self) -> PathBuf {
        self.root.join("packs")
    }

    fn read_meta(&self) -> BTreeMap<String, String> {
        std::fs::read_to_string(self.library_meta()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn write_meta(&self, meta: &BTreeMap<String, String>) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let tmp = self.library_meta().with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(meta).unwrap_or_default()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, self.library_meta()).map_err(|e| e.to_string())
    }

    /// Normalizes any image to a 200×200 PNG and keeps it in the library.
    pub fn add_upload(&self, bytes: &[u8], name: &str) -> Result<LibraryIcon, String> {
        let png = crate::render::normalize_upload(bytes).map_err(|e| e.to_string())?;
        let hex: String = Sha256::digest(&png).iter().take(8).map(|b| format!("{b:02x}")).collect();
        let id = format!("{hex}.png");
        std::fs::create_dir_all(self.library_dir()).map_err(|e| e.to_string())?;
        std::fs::write(self.library_dir().join(&id), &png).map_err(|e| e.to_string())?;
        let mut meta = self.read_meta();
        let name: String = name.trim().chars().take(80).collect();
        meta.insert(id.clone(), if name.is_empty() { "Ikona".into() } else { name.clone() });
        self.write_meta(&meta)?;
        Ok(LibraryIcon { url: url(&format!("library/{id}")), name: meta[&id].clone(), id })
    }

    pub fn list_library(&self) -> Vec<LibraryIcon> {
        let meta = self.read_meta();
        let Ok(entries) = std::fs::read_dir(self.library_dir()) else { return vec![] };
        let mut out: Vec<LibraryIcon> = entries
            .flatten()
            .filter_map(|e| {
                let id = e.file_name().to_string_lossy().into_owned();
                is_library_id(&id).then(|| LibraryIcon { url: url(&format!("library/{id}")), name: meta.get(&id).cloned().unwrap_or_else(|| "Ikona".into()), id })
            })
            .collect();
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    }

    pub fn delete_library(&self, id: &str) -> Result<(), String> {
        if !is_library_id(id) {
            return Err(err("Neplatná ikona"));
        }
        let _ = std::fs::remove_file(self.library_dir().join(id));
        let mut meta = self.read_meta();
        meta.remove(id);
        self.write_meta(&meta)
    }

    /// Imports a `.streamDeckIconPack` (zip with `<id>.sdIconPack/manifest.json`).
    pub fn import_pack(&self, bytes: &[u8]) -> Result<IconPack, String> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| err("Soubor není balík ikon (ZIP)"))?;
        if archive.len() > MAX_PACK_ENTRIES {
            return Err(err("Balík obsahuje příliš mnoho souborů"));
        }
        let mut paths = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            let f = archive.by_index(i).map_err(|e| e.to_string())?;
            let name = f.name().map_err(|e| e.to_string())?.to_string();
            let p = crate::plugins::install::zip_path(&name).map_err(|e| e.to_string())?;
            paths.push((p, f.is_dir()));
        }
        let manifest = paths
            .iter()
            .filter(|(p, dir)| !dir && p.file_name().is_some_and(|n| n == "manifest.json"))
            .min_by_key(|(p, _)| p.components().count())
            .map(|(p, _)| p.clone())
            .ok_or_else(|| err("Balík neobsahuje manifest.json"))?;
        let prefix = manifest.parent().map(Path::to_path_buf).unwrap_or_default();
        let folder = prefix.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let id = folder.strip_suffix(".sdIconPack").unwrap_or(folder).to_string();
        let id = if crate::plugins::install::valid_id(&id) { id } else { format!("pack-{}", crate::config::new_id()) };

        std::fs::create_dir_all(self.packs_dir()).map_err(|e| e.to_string())?;
        let stage = self.packs_dir().join(format!(".stage-{}", crate::config::new_id()));
        std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
        let result = (|| -> Result<(), String> {
            let mut total = 0u64;
            for (i, (p, is_dir)) in paths.iter().enumerate() {
                let Ok(rel) = p.strip_prefix(&prefix) else { continue };
                if rel.as_os_str().is_empty() {
                    continue;
                }
                let out = stage.join(rel);
                if *is_dir {
                    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
                    continue;
                }
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let mut f = archive.by_index(i).map_err(|e| e.to_string())?;
                let mut buf = Vec::new();
                (&mut f).take(MAX_PACK_BYTES - total + 1).read_to_end(&mut buf).map_err(|e| e.to_string())?;
                total += buf.len() as u64;
                if total > MAX_PACK_BYTES {
                    return Err(err("Balík je větší než 200 MB"));
                }
                std::fs::write(&out, buf).map_err(|e| e.to_string())?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(e);
        }
        let dest = self.packs_dir().join(&id);
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&stage, &dest).map_err(|e| {
            let _ = std::fs::remove_dir_all(&stage);
            e.to_string()
        })?;
        self.pack_info(&id).ok_or_else(|| err("Balík se nepodařilo načíst"))
    }

    fn pack_info(&self, id: &str) -> Option<IconPack> {
        let dir = self.packs_dir().join(id);
        let m: PackManifest = std::fs::read_to_string(dir.join("manifest.json"))
            .ok()
            .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default();
        let icon_url = m
            .icon
            .as_deref()
            .and_then(|i| crate::plugins::manifest::resolve_image(&dir, i))
            .and_then(|abs| dir.canonicalize().ok().and_then(|root| abs.strip_prefix(root).ok().map(|r| r.to_string_lossy().into_owned())))
            .map(|rel| url(&format!("packs/{id}/{rel}")))
            .unwrap_or_default();
        Some(IconPack {
            id: id.to_string(),
            name: m.name.unwrap_or_else(|| id.to_string()),
            author: m.author.unwrap_or_default(),
            icon_url,
            count: self.pack_icons(id).map(|v| v.len()).unwrap_or(0),
        })
    }

    pub fn list_packs(&self) -> Vec<IconPack> {
        let Ok(entries) = std::fs::read_dir(self.packs_dir()) else { return vec![] };
        let mut out: Vec<IconPack> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| {
                let id = e.file_name().to_string_lossy().into_owned();
                (!id.starts_with('.')).then(|| self.pack_info(&id)).flatten()
            })
            .collect();
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    }

    pub fn pack_icons(&self, id: &str) -> Result<Vec<PackIcon>, String> {
        if !crate::plugins::install::valid_id(id) && !id.starts_with("pack-") {
            return Err(err("Neplatný balík"));
        }
        let dir = self.packs_dir().join(id);
        let root = dir.canonicalize().map_err(|_| err("Balík neexistuje"))?;
        let rel_of = |p: &Path| p.strip_prefix(&root).ok().map(|r| r.to_string_lossy().replace('\\', "/"));
        let mut out = vec![];
        if let Some(entries) = std::fs::read_to_string(dir.join("icons.json")).ok().and_then(|t| serde_json::from_str::<Vec<PackEntry>>(t.trim_start_matches('\u{feff}')).ok()) {
            for e in entries {
                let candidates = [root.join("icons").join(&e.path), root.join(&e.path)];
                let Some(file) = candidates.iter().find_map(|c| c.canonicalize().ok()).filter(|f| f.starts_with(&root) && f.is_file() && is_image(f)) else { continue };
                let Some(rel) = rel_of(&file) else { continue };
                let name = e.name.unwrap_or_else(|| file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
                out.push(PackIcon { name, tags: e.tags, url: url(&format!("packs/{id}/{rel}")) });
            }
        } else {
            let base = if root.join("icons").is_dir() { root.join("icons") } else { root.clone() };
            let mut stack = vec![base];
            while let Some(d) = stack.pop() {
                for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if is_image(&p) && p.file_name().is_some_and(|n| n != "icon.png") {
                        if let Some(rel) = rel_of(&p) {
                            let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                            out.push(PackIcon { name, tags: vec![], url: url(&format!("packs/{id}/{rel}")) });
                        }
                    }
                }
            }
            out.sort_by(|a, b| a.name.cmp(&b.name));
        }
        Ok(out)
    }

    pub fn delete_pack(&self, id: &str) -> Result<(), String> {
        if !crate::plugins::install::valid_id(id) && !id.starts_with("pack-") {
            return Err(err("Neplatný balík"));
        }
        std::fs::remove_dir_all(self.packs_dir().join(id)).map_err(|e| e.to_string())
    }

    /// `gdicons://` handler: `/library/<id>` and `/packs/<pack>/<path>`, confined to the store.
    pub fn serve(&self, url_path: &str) -> Served {
        let not_found = || Served { status: 404, mime: "text/plain".into(), body: b"not found".to_vec() };
        let path = url_path.split(['?', '#']).next().unwrap_or("");
        if path.contains('%') || path.contains('\\') {
            return not_found();
        }
        let rel = Path::new(path.trim_start_matches('/'));
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return not_found();
        }
        let first = rel.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
        if first != "library" && first != "packs" {
            return not_found();
        }
        let (Ok(root), Ok(file)) = (self.root.join(&first).canonicalize(), self.root.join(rel).canonicalize()) else { return not_found() };
        if !file.starts_with(&root) || !file.is_file() || !is_image(&file) {
            return not_found();
        }
        let Ok(body) = std::fs::read(&file) else { return not_found() };
        let mime = match file.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("svg") => "image/svg+xml",
            Some("webp") => "image/webp",
            _ => "application/octet-stream",
        };
        Served { status: 200, mime: mime.into(), body }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("gd-icons-{}", crate::config::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn png() -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255])).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    #[test]
    fn library_add_list_delete() {
        let lib = IconStore::new(tmp());
        let item = lib.add_upload(&png(), "Můj mikrofon").unwrap();
        assert!(item.id.ends_with(".png"));
        let list = lib.list_library();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Můj mikrofon");
        assert!(lib.add_upload(b"junk", "x").is_err());
        lib.delete_library(&item.id).unwrap();
        assert!(lib.list_library().is_empty());
        assert!(lib.delete_library("../../etc").is_err());
    }

    #[test]
    fn icon_pack_import_with_icons_json() {
        let s = IconStore::new(tmp());
        let manifest = br#"{"Name":"Gaming Pack","Author":"Someone","Version":"1.0","Icon":"icon.png"}"#;
        let icons = br#"[{"path":"mic.png","name":"Mic","tags":["voice","audio"]},{"path":"missing.png","name":"Gone"}]"#;
        let bytes = zip(&[
            ("com.someone.gaming.sdIconPack/manifest.json", manifest),
            ("com.someone.gaming.sdIconPack/icons.json", icons),
            ("com.someone.gaming.sdIconPack/icon.png", &png()),
            ("com.someone.gaming.sdIconPack/icons/mic.png", &png()),
        ]);
        let pack = s.import_pack(&bytes).unwrap();
        assert_eq!(pack.id, "com.someone.gaming");
        assert_eq!(pack.name, "Gaming Pack");
        let icons = s.pack_icons(&pack.id).unwrap();
        assert_eq!(icons.len(), 1, "missing files are skipped");
        assert_eq!(icons[0].name, "Mic");
        assert_eq!(icons[0].tags, vec!["voice", "audio"]);
        assert_eq!(icons[0].url, "http://gdicons.localhost/packs/com.someone.gaming/icons/mic.png");
        assert_eq!(s.list_packs().len(), 1);
        s.delete_pack(&pack.id).unwrap();
        assert!(s.list_packs().is_empty());
    }

    #[test]
    fn icon_pack_without_icons_json_lists_images() {
        let s = IconStore::new(tmp());
        let bytes = zip(&[
            ("my.sdIconPack/manifest.json", br#"{"Name":"Loose"}"#),
            ("my.sdIconPack/icons/a.png", &png()),
            ("my.sdIconPack/icons/sub/b.svg", b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
            ("my.sdIconPack/icons/readme.txt", b"hi"),
        ]);
        let pack = s.import_pack(&bytes).unwrap();
        let mut names: Vec<String> = s.pack_icons(&pack.id).unwrap().into_iter().map(|i| i.name).collect();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn malicious_or_wrong_zips_are_rejected() {
        let s = IconStore::new(tmp());
        assert!(s.import_pack(b"not a zip").is_err());
        assert!(s.import_pack(&zip(&[("../evil.png", &png())])).is_err());
        assert!(s.import_pack(&zip(&[("x.sdIconPack/../../evil.png", &png())])).is_err());
        assert!(s.import_pack(&zip(&[("readme.txt", b"no manifest")])).is_err());
        assert!(!s.root.parent().unwrap().join("evil.png").exists());
    }

    #[test]
    fn scheme_serves_only_inside_the_store() {
        let s = IconStore::new(tmp());
        let item = s.add_upload(&png(), "a").unwrap();
        let ok = s.serve(&format!("/library/{}", item.id));
        assert_eq!((ok.status, ok.mime.as_str()), (200, "image/png"));
        for bad in ["/library/../library.json", "/packs/../../x", "/library/%2e%2e/x", "/other/x", ""] {
            assert_eq!(s.serve(bad).status, 404, "{bad}");
        }
    }
}
