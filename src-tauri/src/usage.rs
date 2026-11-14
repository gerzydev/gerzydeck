//! AI plan usage (Claude, ChatGPT/Codex) read with the logins of Claude Code and Codex.
//!
//! Expired logins are refreshed the same way those tools do it, and the new tokens are written
//! back into their files (all other fields untouched), so they stay logged in.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// refresh a little before the token runs out
const REFRESH_MARGIN_S: i64 = 5 * 60;

pub const PROVIDERS: [&str; 2] = ["claude", "codex"];

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub percent: f32,
    /// unix seconds; None when the window has no active period (nothing used in it lately)
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct Usage {
    /// the 5-hour window
    pub session: Option<Window>,
    pub weekly: Option<Window>,
}

/// What the editor gets for its key previews.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct UsageView {
    #[serde(flatten)]
    pub usage: Usage,
    pub error: Option<String>,
}

pub fn now_s() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

// ---------- time helpers (no date crate in the project) ----------

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

/// "2026-11-13T00:29:59.594+00:00" / "...Z" → unix seconds
fn parse_rfc3339(s: &str) -> Option<i64> {
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let rest = &s[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest {
        "Z" | "z" | "" => 0,
        _ => {
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            let oh: i64 = rest.get(1..3)?.parse().ok()?;
            let om: i64 = rest.get(4..6)?.parse().ok()?;
            sign * (oh * 3600 + om * 60)
        }
    };
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - offset)
}

fn rfc3339(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", t / 3600, t % 3600 / 60, t % 60)
}

/// Countdown until a reset, short enough for a key.
pub fn reset_text(now: i64, at: i64) -> String {
    let s = at - now;
    if s <= 0 {
        crate::i18n::tr("teď", "now").into()
    } else if s < 60 {
        "<1 min".into()
    } else if s < 3600 {
        format!("{} min", s / 60)
    } else if s < 86_400 {
        format!("{} h {:02}", s / 3600, s % 3600 / 60)
    } else {
        format!("{} d {} h", s / 86_400, s % 86_400 / 3600)
    }
}

// ---------- parsing ----------

fn f(v: &Value) -> Option<f32> {
    v.as_f64().map(|x| x as f32)
}

pub fn parse_claude(v: &Value) -> Result<Usage, String> {
    // An idle window (nothing used for a while) comes as null or without "resets_at": 0 %, no countdown.
    let win = |k: &str| -> Option<Window> {
        let w = v.get(k)?;
        if w.is_null() {
            return Some(Window { percent: 0.0, resets_at: None });
        }
        let resets_at = w.get("resets_at").and_then(Value::as_str).and_then(parse_rfc3339);
        Some(Window { percent: w.get("utilization").and_then(f).unwrap_or(0.0), resets_at })
    };
    let u = Usage { session: win("five_hour"), weekly: win("seven_day") };
    if u.session.is_none() && u.weekly.is_none() {
        return Err(crate::i18n::tr("Neznámá odpověď od Claude", "Unknown answer from Claude").into());
    }
    Ok(u)
}

pub fn parse_codex(v: &Value) -> Result<Usage, String> {
    let rl = v.get("rate_limit").ok_or(crate::i18n::tr("Neznámá odpověď od ChatGPT", "Unknown answer from ChatGPT"))?;
    let win = |k: &str| -> Option<Window> {
        let w = rl.get(k)?;
        Some(Window { percent: f(w.get("used_percent")?)?, resets_at: Some(w.get("reset_at")?.as_i64()?) })
    };
    let u = Usage { session: win("primary_window"), weekly: win("secondary_window") };
    if u.session.is_none() && u.weekly.is_none() {
        return Err(crate::i18n::tr("Neznámá odpověď od ChatGPT", "Unknown answer from ChatGPT").into());
    }
    Ok(u)
}

// ---------- credentials ----------

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}

fn claude_file() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".claude")).join(".credentials.json")
}

fn codex_file() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex")).join("auth.json")
}

fn read_json(p: &Path) -> Result<Value, String> {
    let t = std::fs::read_to_string(p).map_err(|_| crate::i18n::tr("Nejsi přihlášený", "Not logged in").to_string())?;
    serde_json::from_str(t.trim_start_matches('\u{feff}')).map_err(|_| crate::i18n::tr("Soubor s přihlášením je poškozený", "The login file is damaged").to_string())
}

/// Write to a temp file next to it, then rename: the file is never half-written.
fn write_json(p: &Path, v: &Value) -> Result<(), String> {
    let tmp = p.with_extension(format!("gd-{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, p).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

fn claude_needs_refresh(file: &Value, now_ms: i64) -> bool {
    file["claudeAiOauth"]["expiresAt"].as_i64().is_none_or(|e| e - REFRESH_MARGIN_S * 1000 <= now_ms)
}

fn apply_claude_refresh(file: &mut Value, resp: &Value, now_ms: i64) -> Result<(), String> {
    let access = resp["access_token"].as_str().ok_or(crate::i18n::tr("Obnovení přihlášení Claude selhalo", "Refreshing the Claude login failed"))?;
    let o = file.get_mut("claudeAiOauth").and_then(Value::as_object_mut).ok_or(crate::i18n::tr("Nejsi přihlášený v Claude Code", "Not logged in to Claude Code"))?;
    o.insert("accessToken".into(), access.into());
    if let Some(r) = resp["refresh_token"].as_str() {
        o.insert("refreshToken".into(), r.into());
    }
    if let Some(e) = resp["expires_in"].as_i64() {
        o.insert("expiresAt".into(), (now_ms + e * 1000).into());
    }
    Ok(())
}

fn apply_codex_refresh(file: &mut Value, resp: &Value, now_iso: &str) -> Result<(), String> {
    let access = resp["access_token"].as_str().ok_or(crate::i18n::tr("Obnovení přihlášení ChatGPT selhalo", "Refreshing the ChatGPT login failed"))?;
    let t = file.get_mut("tokens").and_then(Value::as_object_mut).ok_or(crate::i18n::tr("Nejsi přihlášený v Codexu", "Not logged in to Codex"))?;
    t.insert("access_token".into(), access.into());
    for k in ["refresh_token", "id_token"] {
        if let Some(v) = resp[k].as_str() {
            t.insert(k.into(), v.into());
        }
    }
    file["last_refresh"] = now_iso.into();
    Ok(())
}

fn jwt_exp(token: &str) -> Option<i64> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()?["exp"].as_i64()
}

// ---------- fetching ----------

/// A failed fetch; `retry_after` (seconds) is set when the service asked us to slow down.
#[derive(Debug, Clone, PartialEq)]
pub struct FetchError {
    pub message: String,
    pub rate_limited: bool,
    pub retry_after: Option<u64>,
}

impl From<String> for FetchError {
    fn from(message: String) -> Self {
        Self { message, rate_limited: false, retry_after: None }
    }
}

impl From<&str> for FetchError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

async fn post_json(client: &reqwest::Client, url: &str, body: &Value) -> Result<Value, String> {
    let res = client.post(url).json(body).send().await.map_err(|_| crate::i18n::tr("Bez připojení k internetu", "No internet connection").to_string())?;
    let status = res.status();
    let v: Value = res.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(crate::i18n::tr("Přihlášení vypršelo – přihlas se", "Login expired, log in again").into());
    }
    Ok(v)
}

async fn get_json(client: &reqwest::Client, url: &str, headers: &[(&str, &str)]) -> Result<Value, (reqwest::StatusCode, Option<u64>)> {
    let mut req = client.get(url).header(reqwest::header::USER_AGENT, "GerzysDeck");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let res = req.send().await.map_err(|_| (reqwest::StatusCode::SERVICE_UNAVAILABLE, None))?;
    if !res.status().is_success() {
        // Retry-After in seconds (the HTTP-date form is not used by these services)
        let retry = res.headers().get(reqwest::header::RETRY_AFTER).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok());
        return Err((res.status(), retry));
    }
    res.json().await.map_err(|_| (reqwest::StatusCode::BAD_GATEWAY, None))
}

fn http_err((s, retry_after): (reqwest::StatusCode, Option<u64>)) -> FetchError {
    let message = match s.as_u16() {
        401 | 403 => crate::i18n::tr("Přihlášení vypršelo – přihlas se", "Login expired, log in again").into(),
        429 => crate::i18n::tr("Moc dotazů, zkusím později", "Too many requests, retrying later").into(),
        503 => crate::i18n::tr("Bez připojení k internetu", "No internet connection").into(),
        c => format!("Chyba serveru ({c})"),
    };
    FetchError { message, rate_limited: s.as_u16() == 429, retry_after }
}

async fn claude(client: &reqwest::Client) -> Result<Usage, FetchError> {
    let path = claude_file();
    let mut file = read_json(&path).map_err(|e| format!("{e} (Claude Code)"))?;
    if claude_needs_refresh(&file, now_s() * 1000) {
        let o = &file["claudeAiOauth"];
        let refresh = o["refreshToken"].as_str().ok_or(crate::i18n::tr("Nejsi přihlášený v Claude Code", "Not logged in to Claude Code"))?;
        let scope = o["scopes"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        let resp = post_json(client, CLAUDE_TOKEN_URL, &serde_json::json!({
            "grant_type": "refresh_token", "refresh_token": refresh, "client_id": CLAUDE_CLIENT_ID, "scope": scope,
        }))
        .await?;
        // re-read right before writing, in case Claude Code changed it meanwhile
        let mut fresh = read_json(&path)?;
        apply_claude_refresh(&mut fresh, &resp, now_s() * 1000)?;
        write_json(&path, &fresh)?;
        log::info!("usage: Claude login refreshed");
        file = fresh;
    }
    let token = file["claudeAiOauth"]["accessToken"].as_str().ok_or(crate::i18n::tr("Nejsi přihlášený v Claude Code", "Not logged in to Claude Code"))?;
    let auth = format!("Bearer {token}");
    let v = get_json(client, CLAUDE_USAGE_URL, &[("Authorization", &auth), ("anthropic-beta", "oauth-2025-04-20")]).await.map_err(http_err)?;
    Ok(parse_claude(&v)?)
}

async fn codex(client: &reqwest::Client) -> Result<Usage, FetchError> {
    let path = codex_file();
    let mut file = read_json(&path).map_err(|e| format!("{e} (Codex)"))?;
    let access = file["tokens"]["access_token"].as_str().ok_or(crate::i18n::tr("Codex není přihlášený přes ChatGPT", "Codex is not logged in with ChatGPT"))?;
    if jwt_exp(access).is_none_or(|e| e - REFRESH_MARGIN_S <= now_s()) {
        let refresh = file["tokens"]["refresh_token"].as_str().ok_or(crate::i18n::tr("Codex není přihlášený přes ChatGPT", "Codex is not logged in with ChatGPT"))?;
        let resp = post_json(client, CODEX_TOKEN_URL, &serde_json::json!({
            "client_id": CODEX_CLIENT_ID, "grant_type": "refresh_token", "refresh_token": refresh, "scope": "openid profile email",
        }))
        .await?;
        let mut fresh = read_json(&path)?;
        apply_codex_refresh(&mut fresh, &resp, &rfc3339(now_s()))?;
        write_json(&path, &fresh)?;
        log::info!("usage: ChatGPT login refreshed");
        file = fresh;
    }
    let token = file["tokens"]["access_token"].as_str().unwrap_or_default();
    let account = file["tokens"]["account_id"].as_str().unwrap_or_default();
    let auth = format!("Bearer {token}");
    let v = get_json(client, CODEX_USAGE_URL, &[("Authorization", &auth), ("ChatGPT-Account-Id", account)]).await.map_err(http_err)?;
    Ok(parse_codex(&v)?)
}

pub async fn fetch(client: &reqwest::Client, provider: &str) -> Result<Usage, FetchError> {
    match provider {
        "claude" => claude(client).await,
        "codex" => codex(client).await,
        _ => Err(crate::i18n::tr("Neznámá služba", "Unknown service").into()),
    }
}

/// Seconds between regular refreshes: Claude's usage endpoint is strict about request rates.
pub fn interval_s(provider: &str) -> u64 {
    if provider == "claude" { 300 } else { 60 }
}

/// When to ask a service again: on schedule, sooner after a key press, much later after "too many requests".
#[derive(Debug, Default)]
pub struct Schedule {
    next: std::collections::HashMap<String, u64>,
    backoff: std::collections::HashMap<String, u64>,
    limited_until: std::collections::HashMap<String, u64>,
    last_try: std::collections::HashMap<String, u64>,
}

impl Schedule {
    /// Whether `provider` should be fetched at `now` (seconds); `pressed` = a key asked for fresh numbers.
    pub fn due(&self, provider: &str, now: u64, pressed: bool) -> bool {
        if self.limited_until.get(provider).is_some_and(|&t| now < t) {
            return false;
        }
        let recent = self.last_try.get(provider).is_some_and(|&t| now < t + 10);
        self.next.get(provider).is_none_or(|&t| now >= t) || (pressed && !recent)
    }

    pub fn record(&mut self, provider: &str, now: u64, result: Result<(), &FetchError>) {
        self.last_try.insert(provider.into(), now);
        match result {
            Err(e) if e.rate_limited => {
                // 5 → 10 → 20 → 30 minutes, or longer if the service says so
                let b = self.backoff.get(provider).map_or(300, |b| (b * 2).min(1800));
                self.backoff.insert(provider.into(), b);
                let wait = e.retry_after.unwrap_or(0).max(b);
                self.limited_until.insert(provider.into(), now + wait);
                self.next.insert(provider.into(), now + wait);
            }
            _ => {
                if result.is_ok() {
                    self.backoff.remove(provider);
                }
                self.limited_until.remove(provider);
                self.next.insert(provider.into(), now + interval_s(provider));
            }
        }
    }
}

/// Polls the providers the engine asks for: every minute, or right away when woken (a key press).
pub fn spawn_poller(
    wake: std::sync::mpsc::Receiver<()>,
    wanted: impl Fn() -> Vec<String> + Send + 'static,
    publish: impl Fn(String, Result<Usage, String>) + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("usage".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => return log::error!("usage poller: {e}"),
            };
            let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().unwrap_or_default();
            let mut schedule = Schedule::default();
            loop {
                let pressed = match wake.recv_timeout(Duration::from_secs(20)) {
                    Ok(()) => true,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                };
                let now = now_s().max(0) as u64;
                for p in wanted() {
                    if !schedule.due(&p, now, pressed) {
                        continue;
                    }
                    let res = rt.block_on(fetch(&client, &p));
                    schedule.record(&p, now, res.as_ref().map(|_| ()));
                    if let Err(e) = &res {
                        log::warn!("usage {p}: {}{}", e.message, e.retry_after.map(|s| format!(" (retry after {s} s)")).unwrap_or_default());
                    }
                    publish(p, res.map_err(|e| e.message));
                }
            }
        })
        .expect("spawn usage poller")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_claude_usage() {
        let v = json!({
            "five_hour": { "utilization": 21, "resets_at": "2026-11-13T00:29:59.594433+00:00" },
            "seven_day": { "utilization": 3.5, "resets_at": "2026-11-15T02:59:59+00:00" },
            "seven_day_opus": null
        });
        let u = parse_claude(&v).unwrap();
        assert_eq!(u.session, Some(Window { percent: 21.0, resets_at: Some(1_794_529_799) }));
        assert_eq!(u.weekly, Some(Window { percent: 3.5, resets_at: Some(1_794_711_599) }));
        // nothing used for hours: the 5-hour window is idle, shown as 0 % without a countdown
        let idle = parse_claude(&json!({ "five_hour": null, "seven_day": { "utilization": 12, "resets_at": "2026-11-15T02:59:59+00:00" } })).unwrap();
        assert_eq!(idle.session, Some(Window { percent: 0.0, resets_at: None }));
        let no_reset = parse_claude(&json!({ "five_hour": { "utilization": 0, "resets_at": null }, "seven_day": null })).unwrap();
        assert_eq!(no_reset.session, Some(Window { percent: 0.0, resets_at: None }));
        assert!(parse_claude(&json!({ "error": "x" })).is_err());
    }

    #[test]
    fn parses_codex_usage() {
        let v = json!({ "rate_limit": {
            "primary_window": { "used_percent": 100, "reset_at": 1794523637 },
            "secondary_window": { "used_percent": 26, "reset_at": 1794682925 }
        } });
        let u = parse_codex(&v).unwrap();
        assert_eq!(u.session, Some(Window { percent: 100.0, resets_at: Some(1794523637) }));
        assert_eq!(u.weekly, Some(Window { percent: 26.0, resets_at: Some(1794682925) }));
        assert!(parse_codex(&json!({})).is_err());
    }

    #[test]
    fn claude_refresh_updates_only_the_tokens() {
        let mut file = json!({ "claudeAiOauth": {
            "accessToken": "old", "refreshToken": "r-old", "expiresAt": 1, "scopes": ["a", "b"],
            "subscriptionType": "max" }, "other": { "keep": true } });
        assert!(claude_needs_refresh(&file, 10_000));
        apply_claude_refresh(&mut file, &json!({ "access_token": "new", "refresh_token": "r-new", "expires_in": 3600 }), 10_000).unwrap();
        assert_eq!(file["claudeAiOauth"]["accessToken"], "new");
        assert_eq!(file["claudeAiOauth"]["refreshToken"], "r-new");
        assert_eq!(file["claudeAiOauth"]["expiresAt"], 10_000 + 3_600_000);
        assert_eq!(file["claudeAiOauth"]["subscriptionType"], "max");
        assert_eq!(file["other"]["keep"], true);
        assert!(!claude_needs_refresh(&file, 10_000));
        // a response without tokens must not damage the file
        assert!(apply_claude_refresh(&mut file, &json!({ "error": "invalid_grant" }), 0).is_err());
        assert_eq!(file["claudeAiOauth"]["accessToken"], "new");
    }

    #[test]
    fn codex_refresh_updates_only_the_tokens() {
        let mut file = json!({ "auth_mode": "chatgpt", "OPENAI_API_KEY": null,
            "tokens": { "id_token": "i", "access_token": "a", "refresh_token": "r", "account_id": "acc" },
            "last_refresh": "2026-01-01T00:00:00Z" });
        apply_codex_refresh(&mut file, &json!({ "id_token": "i2", "access_token": "a2", "refresh_token": "r2" }), "2026-11-12T20:00:00Z").unwrap();
        assert_eq!(file["tokens"]["access_token"], "a2");
        assert_eq!(file["tokens"]["refresh_token"], "r2");
        assert_eq!(file["tokens"]["id_token"], "i2");
        assert_eq!(file["tokens"]["account_id"], "acc");
        assert_eq!(file["last_refresh"], "2026-11-12T20:00:00Z");
        assert_eq!(file["auth_mode"], "chatgpt");
        assert!(apply_codex_refresh(&mut file, &json!({}), "x").is_err());
        assert_eq!(file["tokens"]["access_token"], "a2");
    }

    #[test]
    fn jwt_expiry_is_read_from_the_payload() {
        use base64::Engine as _;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"exp":1794600000}"#);
        assert_eq!(jwt_exp(&format!("h.{payload}.s")), Some(1_794_600_000));
        assert_eq!(jwt_exp("garbage"), None);
    }

    #[test]
    fn reset_countdown_text() {
        assert_eq!(reset_text(1000, 1000 + 59), "<1 min");
        assert_eq!(reset_text(1000, 1000 + 25 * 60), "25 min");
        assert_eq!(reset_text(1000, 1000 + 3 * 3600 + 25 * 60), "3 h 25");
        assert_eq!(reset_text(1000, 1000 + 2 * 86400 + 5 * 3600), "2 d 5 h");
        assert_eq!(reset_text(1000, 900), "now");
    }

    #[test]
    fn atomic_write_keeps_the_file_valid() {
        let dir = std::env::temp_dir().join(format!("gd-usage-{}", crate::config::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("auth.json");
        std::fs::write(&f, "{}").unwrap();
        write_json(&f, &json!({ "a": 1 })).unwrap();
        assert_eq!(read_json(&f).unwrap(), json!({ "a": 1 }));
    }

    #[test]
    fn schedule_spaces_requests_and_backs_off_on_rate_limits() {
        let limited = FetchError { message: "x".into(), rate_limited: true, retry_after: None };
        let mut s = Schedule::default();
        assert!(s.due("claude", 1000, false), "first time right away");
        s.record("claude", 1000, Ok(()));
        assert!(!s.due("claude", 1100, false));
        assert!(s.due("claude", 1300, false), "every 5 minutes for Claude");
        assert!(s.due("claude", 1100, true), "a key press asks sooner");
        assert!(!s.due("claude", 1005, true), "but not twice within 10 s");
        // too many requests: 5, 10, 20, 30, 30 minutes
        let mut t = 2000;
        for wait in [300, 600, 1200, 1800, 1800] {
            s.record("claude", t, Err(&limited));
            assert!(!s.due("claude", t + wait - 1, true), "a press does not break the back-off");
            assert!(s.due("claude", t + wait, false));
            t += wait;
        }
        s.record("claude", t, Ok(()));
        s.record("claude", t + 300, Err(&limited));
        assert!(s.due("claude", t + 600, false), "success resets the back-off");
        // the service's own Retry-After wins when it is longer
        s.record("codex", 0, Err(&FetchError { message: "x".into(), rate_limited: true, retry_after: Some(3600) }));
        assert!(!s.due("codex", 3599, false) && s.due("codex", 3600, false));
        // other errors keep the normal pace
        s.record("codex", 5000, Err(&FetchError::from("offline")));
        assert!(s.due("codex", 5060, false));
    }
}
