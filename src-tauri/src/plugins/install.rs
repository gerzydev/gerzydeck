//! Bounded ZIP installation with confined paths and rollback on replacement failure.
use super::manifest::Manifest;
use std::{
    collections::HashSet,
    fs,
    io::{self, Cursor, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_BYTES: u64 = 300 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;
#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    pub id: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
}
#[derive(Debug)]
pub struct InstallError(String);
impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Instalace pluginu selhala: {}", self.0)
    }
}
impl std::error::Error for InstallError {}
impl From<io::Error> for InstallError {
    fn from(e: io::Error) -> Self {
        Self(format!("chyba souboru: {e}"))
    }
}
impl From<zip::result::ZipError> for InstallError {
    fn from(e: zip::result::ZipError) -> Self {
        Self(format!("neplatný archiv ZIP: {e}"))
    }
}
fn invalid(message: &str) -> InstallError {
    InstallError(message.into())
}

fn safe_component(name: &str) -> bool {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || ":<>\"|?*".contains(c))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) && !(stem.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
pub(crate) fn valid_id(id: &str) -> bool {
    id.len() <= 200
        && safe_component(id)
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}
fn zip_path(name: &str) -> Result<PathBuf, InstallError> {
    if name.starts_with(['/', '\\']) {
        return Err(invalid("archiv obsahuje absolutní cestu"));
    }
    let name = name.replace('\\', "/");
    let trimmed = name.trim_end_matches('/');
    if !trimmed.split('/').all(safe_component) {
        return Err(invalid("archiv obsahuje nebezpečnou cestu"));
    }
    Ok(PathBuf::from(trimmed))
}
struct Stage(PathBuf);
impl Stage {
    fn new(root: &Path) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..100 {
            let name = format!(
                ".gd-install-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let dir = root.join(name);
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Nelze vytvořit dočasnou složku",
        ))
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_dir_all(&self.0) {
            log::warn!("Nelze uklidit dočasnou složku {}: {e}", self.0.display());
        }
    }
}
fn contained_existing(root: &Path, path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || !path.canonicalize()?.starts_with(root)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Složka pluginu je mimo úložiště",
        ));
    }
    Ok(())
}
pub fn install_from_zip(
    bytes: &[u8],
    plugins_dir: &Path,
    fallback_id: Option<&str>,
) -> Result<InstalledPlugin, InstallError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    if archive.len() > MAX_ENTRIES {
        return Err(invalid("archiv obsahuje více než 20 000 položek"));
    }
    let mut paths = Vec::with_capacity(archive.len());
    let mut names = HashSet::new();
    let mut total = 0u64;
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        let path = zip_path(&file.name()?)?;
        if !names.insert(path.to_string_lossy().to_lowercase()) {
            return Err(invalid("archiv obsahuje duplicitní cesty"));
        }
        if file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err(invalid("archiv obsahuje symbolický odkaz"));
        }
        total = total
            .checked_add(file.size())
            .ok_or_else(|| invalid("archiv je příliš velký"))?;
        if total > MAX_BYTES {
            return Err(invalid("rozbalený archiv přesahuje 300 MB"));
        }
        paths.push(path);
    }
    let mut manifests = paths
        .iter()
        .enumerate()
        .filter(|(_, p)| p.file_name().is_some_and(|s| s == "manifest.json"));
    let (_, manifest_path) = manifests
        .next()
        .ok_or_else(|| invalid("archiv neobsahuje manifest.json"))?;
    if manifests.next().is_some() {
        return Err(invalid("archiv obsahuje více manifestů pluginu"));
    }
    let prefix = manifest_path.parent().unwrap_or(Path::new(""));
    let wrapped = prefix.components().count() == 1
        && prefix
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.ends_with(".sdPlugin"));
    let id = if wrapped {
        prefix
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_suffix(".sdPlugin"))
    } else {
        fallback_id
    }
    .ok_or_else(|| invalid("archiv nemá složku sdPlugin ani náhradní ID"))?;
    if !valid_id(id) {
        return Err(invalid("neplatné ID pluginu"));
    }
    fs::create_dir_all(plugins_dir)?;
    let root = plugins_dir.canonicalize()?;
    let stage = Stage::new(&root)?;
    let unpacked = stage.0.join("unpacked");
    fs::create_dir(&unpacked)?;
    let mut actual = 0u64;
    for (i, path) in paths.iter().enumerate() {
        let Ok(relative) = path.strip_prefix(prefix) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let mut file = archive.by_index(i)?;
        let output = unpacked.join(relative);
        if file.is_dir() {
            fs::create_dir_all(output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut target = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?;
        let copied = io::copy(&mut (&mut file).take(MAX_BYTES - actual + 1), &mut target)?;
        actual += copied;
        if actual > MAX_BYTES {
            return Err(invalid("rozbalený archiv přesahuje 300 MB"));
        }
    }
    let manifest =
        Manifest::load(&unpacked).map_err(|e| invalid(&format!("neplatný manifest: {e}")))?;
    let dir = root.join(format!("{id}.sdPlugin"));
    let backup = stage.0.join("previous");
    let replacing = match fs::symlink_metadata(&dir) {
        Ok(_) => {
            contained_existing(&root, &dir)?;
            fs::rename(&dir, &backup)?;
            true
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };
    if let Err(error) = fs::rename(&unpacked, &dir) {
        if replacing {
            if let Err(rollback) = fs::rename(&backup, &dir) {
                let preserved = stage.0.clone();
                std::mem::forget(stage);
                return Err(invalid(&format!("nelze dokončit instalaci ({error}) ani obnovit původní plugin ({rollback}); záloha: {}", preserved.display())));
            }
        }
        return Err(error.into());
    }
    Ok(InstalledPlugin {
        id: id.into(),
        dir,
        manifest,
    })
}
pub fn uninstall(plugins_dir: &Path, id: &str) -> io::Result<()> {
    if !valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Neplatné ID pluginu",
        ));
    }
    let root = match plugins_dir.canonicalize() {
        Ok(root) => root,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let target = root.join(format!("{id}.sdPlugin"));
    match contained_existing(&root, &target) {
        Ok(()) => fs::remove_dir_all(target),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
pub fn list_installed(plugins_dir: &Path) -> Vec<InstalledPlugin> {
    let Ok(root) = plugins_dir.canonicalize() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut plugins = Vec::new();
    for item in entries {
        let item = match item {
            Ok(item) => item,
            Err(e) => {
                log::warn!("Nelze číst složku pluginů: {e}");
                continue;
            }
        };
        let name = item.file_name();
        let Some(id) = name.to_str().and_then(|s| s.strip_suffix(".sdPlugin")) else {
            continue;
        };
        if !valid_id(id) {
            log::warn!("Neplatné ID pluginu: {id}");
            continue;
        }
        let dir = item.path();
        let result = contained_existing(&root, &dir)
            .map_err(|e| e.to_string())
            .and_then(|()| Manifest::load(&dir).map_err(|e| e.to_string()));
        match result {
            Ok(manifest) => plugins.push(InstalledPlugin {
                id: id.into(),
                dir,
                manifest,
            }),
            Err(e) => log::warn!("Přeskakuji poškozený plugin {id}: {e}"),
        }
    }
    plugins.sort_by(|a, b| a.id.cmp(&b.id));
    plugins
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    const MANIFEST: &str = r#"{"Name":"Test","Actions":[{"UUID":"test.action","Name":"Test","States":[{"Image":"icon"}]}]}"#;
    fn root() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gd-install-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn archive(files: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, contents) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }
    #[test]
    fn wrapped_install_list_uninstall() {
        let p = root();
        let result = install_from_zip(
            &archive(&[
                ("test.id.sdPlugin/manifest.json", MANIFEST),
                ("test.id.sdPlugin/file.txt", "hello"),
            ]),
            &p,
            None,
        )
        .unwrap();
        assert_eq!(result.id, "test.id");
        assert_eq!(
            fs::read_to_string(result.dir.join("file.txt")).unwrap(),
            "hello"
        );
        assert_eq!(list_installed(&p).len(), 1);
        uninstall(&p, "test.id").unwrap();
        assert!(list_installed(&p).is_empty());
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn root_manifest_fallback_and_nested_root() {
        for prefix in ["", "release/"] {
            let p = root();
            let name = format!("{prefix}manifest.json");
            assert_eq!(
                install_from_zip(&archive(&[(&name, MANIFEST)]), &p, Some("test.id"))
                    .unwrap()
                    .id,
                "test.id"
            );
            fs::remove_dir_all(p).unwrap();
        }
    }
    #[test]
    fn rejects_escaping_paths_without_writes() {
        let p = root();
        let plugins = p.join("plugins");
        for bad in [
            "../evil",
            "/evil",
            "C:/evil",
            "..\\evil",
            "\\\\server\\evil",
            "test.id.sdPlugin/../evil",
            "test.id.sdPlugin/file:stream",
            "test.id.sdPlugin/CON.txt",
        ] {
            assert!(
                install_from_zip(
                    &archive(&[("test.id.sdPlugin/manifest.json", MANIFEST), (bad, "bad")]),
                    &plugins,
                    None
                )
                .is_err(),
                "{bad}"
            );
            assert!(!p.join("evil").exists());
            assert!(!plugins.join("test.id.sdPlugin").exists());
        }
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn rejects_non_plugin_and_invalid_ids() {
        let p = root();
        assert!(install_from_zip(&archive(&[("readme", "hi")]), &p, None).is_err());
        assert!(install_from_zip(
            &archive(&[("manifest.json", MANIFEST)]),
            &p,
            Some("../escape")
        )
        .is_err());
        assert!(uninstall(&p, "../escape").is_err());
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn reinstall_replaces_old_and_invalid_preserves_existing() {
        let p = root();
        install_from_zip(
            &archive(&[
                ("test.id.sdPlugin/manifest.json", MANIFEST),
                ("test.id.sdPlugin/old", "old"),
            ]),
            &p,
            None,
        )
        .unwrap();
        assert!(install_from_zip(
            &archive(&[("test.id.sdPlugin/manifest.json", "{}")]),
            &p,
            None
        )
        .is_err());
        assert!(p.join("test.id.sdPlugin/old").exists());
        install_from_zip(
            &archive(&[
                ("test.id.sdPlugin/manifest.json", MANIFEST),
                ("test.id.sdPlugin/new", "new"),
            ]),
            &p,
            None,
        )
        .unwrap();
        assert!(!p.join("test.id.sdPlugin/old").exists());
        assert!(p.join("test.id.sdPlugin/new").exists());
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn rejects_symlink_and_advertised_size_limit() {
        let p = root();
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.add_symlink(
            "link",
            "../outside",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        assert!(
            install_from_zip(&zip.finish().unwrap().into_inner(), &p, Some("test.id")).is_err()
        );
        let mut bytes = archive(&[("test.id.sdPlugin/manifest.json", MANIFEST)]);
        let offset = bytes.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
        bytes[offset + 24..offset + 28].copy_from_slice(&((MAX_BYTES + 1) as u32).to_le_bytes());
        assert!(install_from_zip(&bytes, &p, None)
            .unwrap_err()
            .to_string()
            .contains("300 MB"));
        assert!(!p.join("test.id.sdPlugin").exists());
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn rejects_entry_count_limit() {
        let p = root();
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for n in 0..=MAX_ENTRIES {
            zip.start_file(format!("file{n}"), zip::write::SimpleFileOptions::default())
                .unwrap();
        }
        assert!(
            install_from_zip(&zip.finish().unwrap().into_inner(), &p, Some("test.id"))
                .unwrap_err()
                .to_string()
                .contains("20 000")
        );
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn listing_skips_broken_and_rejects_multiple_manifests() {
        let p = root();
        fs::create_dir(p.join("broken.sdPlugin")).unwrap();
        assert!(list_installed(&p).is_empty());
        assert!(install_from_zip(
            &archive(&[
                ("one.sdPlugin/manifest.json", MANIFEST),
                ("two.sdPlugin/manifest.json", MANIFEST)
            ]),
            &p,
            None
        )
        .is_err());
        fs::remove_dir_all(p).unwrap();
    }
}
