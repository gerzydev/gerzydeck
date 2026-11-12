//! Remote catalogue models and asynchronous download discovery.
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const OPENACTION: &str = "https://openactionapi.github.io/plugins/catalogue.json";
const ELGATO: &str = "https://plugins.amankhanna.me/catalogue.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    OpenAction,
    ElgatoArchive,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub author: String,
    pub source: Source,
    pub icon_url: String,
    pub repository: Option<String>,
    pub download_url: Option<String>,
    pub version: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

#[derive(Deserialize)]
struct OpenEntry {
    name: String,
    author: String,
    repository: Option<String>,
    download_url: Option<String>,
}
#[derive(Deserialize)]
struct ArchiveEntry {
    id: String,
    name: String,
    author: String,
    link: Option<String>,
    version: Option<String>,
}

pub fn parse_openaction(json: &str) -> Result<Vec<CatalogEntry>> {
    let entries: BTreeMap<String, OpenEntry> = serde_json::from_str(json)?;
    entries
        .into_iter()
        .map(|(id, e)| {
            if !super::install::valid_id(&id) {
                bail!("{}", crate::trf!("Neplatné ID pluginu: {id}", "Invalid plugin ID: {id}"));
            }
            Ok(CatalogEntry {
                icon_url: format!("https://openactionapi.github.io/plugins/icons/{id}.png"),
                id,
                name: e.name,
                author: e.author,
                source: Source::OpenAction,
                repository: e.repository,
                download_url: e.download_url,
                version: None,
            })
        })
        .collect()
}
pub fn parse_elgato(json: &str) -> Result<Vec<CatalogEntry>> {
    let entries: Vec<ArchiveEntry> = serde_json::from_str(json)?;
    entries
        .into_iter()
        .map(|e| {
            if !super::install::valid_id(&e.id) {
                bail!("{}", crate::trf!("Neplatné ID pluginu: {}", "Invalid plugin ID: {}", e.id));
            }
            Ok(CatalogEntry {
                icon_url: format!("https://plugins.amankhanna.me/icons/{}.png", e.id),
                download_url: Some(format!(
                    "https://plugins.amankhanna.me/rezipped/{}.zip",
                    e.id
                )),
                id: e.id,
                name: e.name,
                author: e.author,
                source: Source::ElgatoArchive,
                repository: e.link,
                version: e.version,
            })
        })
        .collect()
}
async fn fetch(client: &reqwest::Client, url: &str) -> Result<String> {
    Ok(client
        .get(url)
        .header(reqwest::header::USER_AGENT, "GerzysDeck")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}
pub async fn fetch_all(client: &reqwest::Client) -> (Vec<CatalogEntry>, Vec<String>) {
    let (open, archive) = tokio::join!(fetch(client, OPENACTION), fetch(client, ELGATO));
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    for (source, result) in [
        ("OpenAction", open.and_then(|s| parse_openaction(&s))),
        ("Elgato", archive.and_then(|s| parse_elgato(&s))),
    ] {
        match result {
            Ok(mut rows) => entries.append(&mut rows),
            Err(e) => errors.push(format!("{source}: {e}")),
        }
    }
    (entries, errors)
}
#[derive(Deserialize)]
struct Release {
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    assets: Vec<ReleaseAsset>,
}
#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}
fn release_assets(json: &str) -> Result<Vec<Asset>> {
    let releases: Vec<Release> = serde_json::from_str(json)?;
    let release = releases
        .iter()
        .find(|r| !r.draft && !r.prerelease)
        .or_else(|| releases.iter().find(|r| !r.draft))
        .ok_or_else(|| anyhow!(crate::i18n::tr("Repozitář nemá vydání", "The repository has no release")))?;
    Ok(release
        .assets
        .iter()
        .filter(|a| {
            let name = a.name.to_ascii_lowercase();
            name.ends_with(".zip") || name.ends_with(".streamdeckplugin")
        })
        .map(|a| Asset {
            name: a.name.clone(),
            url: a.browser_download_url.clone(),
        })
        .collect())
}
pub async fn resolve_download(
    client: &reqwest::Client,
    entry: &CatalogEntry,
) -> Result<Vec<Asset>> {
    if let Some(url) = &entry.download_url {
        return Ok(vec![Asset {
            name: url.rsplit('/').next().unwrap_or(&entry.id).to_owned(),
            url: url.clone(),
        }]);
    }
    let repository = entry
        .repository
        .as_deref()
        .ok_or_else(|| anyhow!(crate::i18n::tr("Plugin nemá odkaz ke stažení", "The plugin has no download link")))?;
    let url = reqwest::Url::parse(repository)?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        bail!(crate::i18n::tr("Plugin nemá repozitář na GitHubu", "The plugin has no GitHub repository"));
    }
    let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|s| {
            s.is_empty()
                || !s
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        })
    {
        bail!(crate::i18n::tr("Neplatný repozitář GitHub", "Invalid GitHub repository"));
    }
    release_assets(
        &fetch(
            client,
            &format!(
                "https://api.github.com/repos/{}/{}/releases",
                parts[0],
                parts[1].trim_end_matches(".git")
            ),
        )
        .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_catalogues() {
        let open =
            parse_openaction(include_str!("../../tests/fixtures/catalog/openaction.json")).unwrap();
        let elgato =
            parse_elgato(include_str!("../../tests/fixtures/catalog/elgato.json")).unwrap();
        assert_eq!(open.len(), 5);
        assert_eq!(elgato.len(), 5);
        assert_eq!(open[0].source, Source::OpenAction);
        assert!(open
            .iter()
            .all(|e| e.icon_url
                == format!("https://openactionapi.github.io/plugins/icons/{}.png", e.id)));
        assert!(elgato.iter().all(|e| e.download_url
            == Some(format!(
                "https://plugins.amankhanna.me/rezipped/{}.zip",
                e.id
            ))));
    }
    #[test]
    fn malformed_catalogue_fails() {
        assert!(parse_openaction("[]").is_err());
        assert!(parse_elgato("{}").is_err());
    }
    #[test]
    fn stable_release_and_case_insensitive_asset_extensions() {
        let releases = r#"[{"prerelease":true,"assets":[{"name":"preview.zip","browser_download_url":"preview"}]},{"prerelease":false,"assets":[{"name":"plugin.streamDeckPlugin","browser_download_url":"stable"},{"name":"source.tar.gz","browser_download_url":"ignored"}]}]"#;
        let assets = release_assets(releases).unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].url, "stable");
        assert!(release_assets("[]").is_err());
    }
    #[tokio::test]
    async fn direct_url_needs_no_network() {
        let mut entry = parse_elgato(include_str!("../../tests/fixtures/catalog/elgato.json"))
            .unwrap()
            .remove(0);
        entry.download_url = Some("https://example.invalid/plugin.zip".into());
        let assets = resolve_download(&reqwest::Client::new(), &entry)
            .await
            .unwrap();
        assert_eq!(assets[0].url, "https://example.invalid/plugin.zip");
    }
}
