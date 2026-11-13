//! Built-in look for popular plugins, in the soundboard style (dark key, coloured badge with a
//! gold rim, white glyph; the badge colour shows the state). Skins replace a plugin's *own image files* — manifest state images and the
//! files a plugin later picks with setImage — so keys still change with the plugin's state
//! (a muted mic turns red, a running screen share green). Images a plugin draws itself
//! (data URLs) are not skinned here; the engine gives those the generic badge instead.
//!
//! Pure data: SVG strings, rasterised by `plugins::image::decode_plugin_image`.

use base64::{engine::general_purpose::STANDARD, Engine};

pub const ACCENT: &str = "#ff9a3c";
pub const RED: &str = "#ff6b6b";
pub const GREEN: &str = "#5dde89";
pub const GREY: &str = "#6b7078";

/// Normalises a plugin image reference: "./actions/mute_0@2x.png" → "actions/mute_0".
fn base(image: &str) -> String {
    let s = image.trim().trim_start_matches("./").replace('\\', "/");
    let s = s.rsplit_once('.').filter(|(_, ext)| matches!(ext.to_ascii_lowercase().as_str(), "png" | "svg" | "jpg" | "jpeg" | "gif")).map_or(s.as_str(), |(b, _)| b).to_string();
    s.strip_suffix("@2x").unwrap_or(&s).to_string()
}

/// Badge colours (gradient top, gradient bottom, rim) for a state colour.
fn badge_colors(state: &str) -> (&'static str, &'static str, &'static str) {
    match state {
        RED => ("#8a2020", "#430b0b", "#d0504a"),
        GREEN => ("#1f6b3a", "#0c3119", "#4cc97a"),
        GREY => ("#3b3e43", "#1d1f22", "#6b7078"),
        _ => ("#8f5a19", "#4a2a08", "#d9a040"),
    }
}

/// Badge geometry: key badge in the upper part (the bottom stays free for plugin titles and
/// labels), or one big centred badge for the editor lists.
#[derive(Clone, Copy, PartialEq)]
enum Size {
    Key,
    List,
}

/// Dark key with a coloured badge (gold rim) and a white glyph — the soundboard look.
/// `under` is drawn on the key below the badge (Discord logo), `over` on top (captions, marks).
fn badge_tile(state: &str, g: &str, size: Size, under: &str, over: &str) -> String {
    let (top, bottom, rim) = badge_colors(state);
    let (x, y, w, r, gx, gy, gs) = match size {
        Size::Key => (38.0, 22.0, 68.0, 11.0, 48.0, 32.0, 2.0),
        Size::List => (18.0, 18.0, 108.0, 16.0, 36.0, 36.0, 3.0),
    };
    let glyph = |color: &str, width: f32, dy: f32| {
        format!(r#"<g transform="translate({gx} {}) scale({gs})" fill="none" stroke="{color}" stroke-width="{width}" stroke-linecap="round" stroke-linejoin="round">{g}</g>"#, gy + dy)
    };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144">
<defs>
<linearGradient id="k" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#1b1d21"/><stop offset="1" stop-color="#0b0c0e"/></linearGradient>
<linearGradient id="b" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{top}"/><stop offset="1" stop-color="{bottom}"/></linearGradient>
<radialGradient id="h" cx="0.5" cy="0.38" r="0.55"><stop offset="0" stop-color="{rim}" stop-opacity="0.16"/><stop offset="1" stop-color="{rim}" stop-opacity="0"/></radialGradient>
<filter id="s" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur stdDeviation="4"/></filter>
<filter id="t" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="1.2"/></filter>
</defs>
<rect width="144" height="144" fill="url(#k)"/>
<rect width="144" height="144" fill="url(#h)"/>
{under}
<rect x="{x}" y="{}" width="{w}" height="{w}" rx="{r}" fill="#000" opacity="0.55" filter="url(#s)"/>
<rect x="{x}" y="{y}" width="{w}" height="{w}" rx="{r}" fill="url(#b)"/>
<rect x="{x}" y="{y}" width="{w}" height="{w}" rx="{r}" fill="none" stroke="{rim}" stroke-width="2.5"/>
<path d="M{} {}h{}" stroke="#fff" stroke-opacity="0.22" stroke-width="1.5" stroke-linecap="round"/>
<g filter="url(#t)" opacity="0.45">{}</g>
{}
{over}
</svg>"##,
        y + 4.0,
        x + 6.0,
        y + 3.5,
        w - 12.0,
        glyph("#000", 2.4, 1.0),
        glyph("#ffffff", 2.0, 0.0),
    )
}

/// Base of the generic look for images plugins draw themselves: dark key and a big amber badge;
/// the plugin's picture covers the badge's face and keeps the gold rim around it.
pub fn frame_svg() -> String {
    let (top, bottom, rim) = badge_colors(ACCENT);
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144">
<defs>
<linearGradient id="k" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#1b1d21"/><stop offset="1" stop-color="#0b0c0e"/></linearGradient>
<linearGradient id="b" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{top}"/><stop offset="1" stop-color="{bottom}"/></linearGradient>
</defs>
<rect width="144" height="144" fill="url(#k)"/>
<rect x="18" y="18" width="108" height="108" rx="15" fill="url(#b)"/>
<rect x="18" y="18" width="108" height="108" rx="15" fill="none" stroke="{rim}" stroke-width="2.5"/>
</svg>"##
    )
}

fn caption(text: &str) -> String {
    format!(r##"<text x="72" y="118" text-anchor="middle" font-family="Segoe UI, DM Sans, sans-serif" font-size="14" font-weight="700" letter-spacing="2" fill="#9aa0a8">{text}</text>"##)
}

// ---------- glyphs (24×24, stroke) ----------
const SLASH: &str = r#"<path d="M3.5 3.5l17 17"/>"#;
const MIC: &str = r#"<rect x="9" y="2.5" width="6" height="11" rx="3"/><path d="M5.5 10.5a6.5 6.5 0 0 0 13 0M12 17v4.5M8.5 21.5h7"/>"#;
const HEADPHONES: &str = r#"<path d="M3.5 15v-3a8.5 8.5 0 0 1 17 0v3"/><rect x="2.5" y="14" width="5" height="7" rx="1.5"/><rect x="16.5" y="14" width="5" height="7" rx="1.5"/>"#;
const HEADSET: &str = r#"<path d="M3.5 14v-2a8.5 8.5 0 0 1 17 0v2"/><rect x="2.5" y="13" width="4.5" height="6.5" rx="1.5"/><rect x="17" y="13" width="4.5" height="6.5" rx="1.5"/><path d="M19.5 19.5v.5a2.5 2.5 0 0 1-2.5 2.5h-3.5"/>"#;
const MIC_WAVES: &str = r#"<rect x="9" y="4.5" width="6" height="9.5" rx="3"/><path d="M6.5 11.5a5.5 5.5 0 0 0 11 0M12 17v4M4 6.5a8 8 0 0 0 0 7M20 6.5a8 8 0 0 1 0 7"/>"#;
const CAMERA: &str = r#"<rect x="2.5" y="6.5" width="13" height="11" rx="2"/><path d="M15.5 10.5l6-3.5v10l-6-3.5"/>"#;
const SCREEN: &str = r#"<rect x="2.5" y="4" width="19" height="13" rx="2"/><path d="M8 21h8M12 17v4"/>"#;
const SHARE_ARROW: &str = r#"<path d="M9 13.5v-1.5a3 3 0 0 1 3-3h3.5M13.5 6.5l2.5 2.5-2.5 2.5"/>"#;
const SHARE_STOP: &str = r#"<path d="M9.5 8l5 5M14.5 8l-5 5"/>"#;
const SPEAKER: &str = r#"<path d="M11 4.5 6 8.5H3v7h3l5 4z"/><path d="M15 9a4.5 4.5 0 0 1 0 6M18 6a8.5 8.5 0 0 1 0 12"/>"#;
const MIXER: &str = r#"<path d="M5 3.5v17M12 3.5v17M19 3.5v17"/><path d="M3 15h4M10 8h4M17 12.5h4"/>"#;
const USER_VOLUME: &str = r#"<circle cx="9" cy="7.5" r="3.5"/><path d="M2.5 20.5a6.5 6.5 0 0 1 13 0"/><path d="M18 9.5a3.5 3.5 0 0 1 0 5M20.5 7a7 7 0 0 1 0 10"/>"#;
const HASH: &str = r#"<path d="M4.5 9h15.5M4 15h15.5M10 3.5 8 20.5M16 3.5l-2 17"/>"#;
const HANG_UP: &str = r#"<path d="M2.5 14.5c5.5-5 13.5-5 19 0l-2.2 3.2-3.8-1.6v-2.6a10 10 0 0 0-7 0v2.6l-3.8 1.6z"/>"#;
const MEGAPHONE: &str = r#"<path d="M3.5 9.5v5h3l7.5 5v-15l-7.5 5z"/><path d="M6.5 14.5l1 5h2.5l-1-4M17.5 8.5l2-2M18.5 12h3M17.5 15.5l2 2"/>"#;
const BELL: &str = r#"<path d="M6 16.5V11a6 6 0 0 1 12 0v5.5l1.5 2h-15z"/><path d="M10 21a2 2 0 0 0 4 0"/>"#;
const CHART: &str = r#"<path d="M5 20V11M10.5 20V5M16 20v-6M2.5 20.5h19"/>"#;
const CPU: &str = r#"<rect x="5" y="5" width="14" height="14" rx="2"/><rect x="9" y="9" width="6" height="6" rx="1"/><path d="M9 2v3M15 2v3M9 19v3M15 19v3M2 9h3M2 15h3M19 9h3M19 15h3"/>"#;
const RAM: &str = r#"<path d="M3 7h18a1 1 0 0 1 1 1v3a1.5 1.5 0 0 0 0 3v3a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1v-3a1.5 1.5 0 0 0 0-3V8a1 1 0 0 1 1-1z"/><path d="M7 11v3M12 11v3M17 11v3M6 18v3M10 18v3M14 18v3M18 18v3"/>"#;
const CLOCK: &str = r#"<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3.5 2"/>"#;
const MONITOR: &str = r#"<rect x="2.5" y="4" width="19" height="13" rx="2"/><path d="M8 21h8M12 17v4"/>"#;
const PULSE: &str = r#"<path d="M2.5 12h4l3-7 5 14 3-7h4"/>"#;

fn with_slash(g: &str) -> String {
    format!("{g}{SLASH}")
}

// ---------- System Information (me.amankhanna.oasystem) ----------
// Every action uses the same placeholder "icon" file, so these go by action, not by file.

fn system_tile(action: &str, list: bool) -> Option<String> {
    let g = match action {
        "me.amankhanna.oasystem.cpu" => CPU,
        "me.amankhanna.oasystem.ram" => RAM,
        "me.amankhanna.oasystem.uptime" => CLOCK,
        "me.amankhanna.oasystem.os" => MONITOR,
        _ => return None,
    };
    // the live value goes under the badge (title_align "bottom")
    Some(badge_tile(ACCENT, g, if list { Size::List } else { Size::Key }, "", ""))
}

// ---------- Discord (me.amankhanna.oadiscord) ----------
// State 0 = idle (accent), state 1 = active: red when it cuts you off, green when you're live.
// "_off" files (and headphonesAndMicrophone_1) are sent while Discord isn't connected / in a call.

/// Discord logo (path data from Simple Icons, CC0; the logo is a trademark of Discord Inc.),
/// large and faint in the lower left of the key face, tinted with the state colour.
fn discord_logo(color: &str) -> String {
    const CLYDE: &str = "M20.317 4.3698a19.7913 19.7913 0 00-4.8851-1.5152.0741.0741 0 00-.0785.0371c-.211.3753-.4447.8648-.6083 1.2495-1.8447-.2762-3.68-.2762-5.4868 0-.1636-.3933-.4058-.8742-.6177-1.2495a.077.077 0 00-.0785-.037 19.7363 19.7363 0 00-4.8852 1.515.0699.0699 0 00-.0321.0277C.5334 9.0458-.319 13.5799.0992 18.0578a.0824.0824 0 00.0312.0561c2.0528 1.5076 4.0413 2.4228 5.9929 3.0294a.0777.0777 0 00.0842-.0276c.4616-.6304.8731-1.2952 1.226-1.9942a.076.076 0 00-.0416-.1057c-.6528-.2476-1.2743-.5495-1.8722-.8923a.077.077 0 01-.0076-.1277c.1258-.0943.2517-.1923.3718-.2914a.0743.0743 0 01.0776-.0105c3.9278 1.7933 8.18 1.7933 12.0614 0a.0739.0739 0 01.0785.0095c.1202.099.246.1981.3728.2924a.077.077 0 01-.0066.1276 12.2986 12.2986 0 01-1.873.8914.0766.0766 0 00-.0407.1067c.3604.698.7719 1.3628 1.225 1.9932a.076.076 0 00.0842.0286c1.961-.6067 3.9495-1.5219 6.0023-3.0294a.077.077 0 00.0313-.0552c.5004-5.177-.8382-9.6739-3.5485-13.6604a.061.061 0 00-.0312-.0286zM8.02 15.3312c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9555-2.4189 2.157-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.9555 2.4189-2.1569 2.4189zm7.9748 0c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9554-2.4189 2.1569-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.946 2.4189-2.1568 2.4189Z";
    format!(r#"<path transform="translate(-16 50) scale(4.4)" d="{CLYDE}" fill="{color}" fill-opacity="0.14"/>"#)
}

fn discord_file(file: &str, size: Size) -> Option<String> {
    let name = file.strip_prefix("actions/")?;
    let mic_off = with_slash(MIC);
    let headphones_off = with_slash(HEADPHONES);
    let camera_off = with_slash(CAMERA);
    let screen_share = format!("{SCREEN}{SHARE_ARROW}");
    let screen_stop = format!("{SCREEN}{SHARE_STOP}");
    let (g, color, cap): (&str, &str, Option<&str>) = match name {
        "mute_0" => (MIC, ACCENT, None),
        "mute_1" => (&mic_off, RED, None),
        "deafen_0" => (HEADPHONES, ACCENT, None),
        "deafen_1" => (&headphones_off, RED, None),
        "pushtotalk_0" => (MIC, ACCENT, Some("PTT")),
        "pushtotalk_1" => (MIC, GREEN, Some("PTT")),
        "pushtomute_0" => (&mic_off, ACCENT, Some("PTM")),
        "pushtomute_1" => (&mic_off, GREEN, Some("PTM")),
        "ptttoggle_0" => (MIC_WAVES, ACCENT, Some("VOICE")),
        "ptttoggle_1" => (MIC, ACCENT, Some("PTT")),
        "video_0" => (&camera_off, ACCENT, None),
        "video_0_off" => (&camera_off, GREY, None),
        "video_1" => (CAMERA, GREEN, None),
        "stream_0" => (&screen_share, ACCENT, None),
        "stream_0_off" => (&screen_share, GREY, None),
        "stream_1" => (&screen_stop, GREEN, None),
        "volumeControl" => (MIXER, ACCENT, None),
        "userVolumeControl" => (USER_VOLUME, ACCENT, None),
        "headphonesAndMicrophone" => (HEADSET, ACCENT, None),
        "headphonesAndMicrophone_1" => (HEADSET, GREY, None),
        "textchannel" => (HASH, ACCENT, None),
        "voicechannel_0" => (SPEAKER, ACCENT, None),
        "voicechannel_1" => (HANG_UP, RED, None),
        "soundboard" => (MEGAPHONE, ACCENT, None),
        "notifications" => (BELL, ACCENT, None),
        "serverStats" => (CHART, ACCENT, None),
        _ => return None,
    };
    let rim = badge_colors(color).2;
    // lists show just the badge; the key adds the faint logo and the caption
    if size == Size::List {
        return Some(badge_tile(color, g, size, "", ""));
    }
    Some(badge_tile(color, g, size, &discord_logo(rim), &cap.map(caption).unwrap_or_default()))
}

// ---------- Essentials for Spotify (com.ntanis.essentials-for-spotify) ----------
// The plugin switches between its own files in images/states/. A file is named after what a
// press does ("like" = not liked yet, "unlike" = liked), "-unknown" = state not known yet.
// Active toggles are green, available ones accent, errors red, waiting/unknown grey.

const PLAY: &str = r#"<path d="M7.5 4.5v15l12-7.5z"/>"#;
const PAUSE: &str = r#"<path d="M8 5v14M16 5v14"/>"#;
const NEXT: &str = r#"<path d="M5 5l10 7-10 7z"/><path d="M19 5v14"/>"#;
const PREVIOUS: &str = r#"<path d="M19 5 9 12l10 7z"/><path d="M5 5v14"/>"#;
const FORWARD: &str = r#"<path d="M2.5 6l8.5 6-8.5 6zM12.5 6l8.5 6-8.5 6z"/>"#;
const BACKWARD: &str = r#"<path d="M21.5 6 13 12l8.5 6zM11.5 6 3 12l8.5 6z"/>"#;
const SHUFFLE: &str = r#"<path d="M3 7h3c2 0 3.2 1 4.2 2.6l2.6 4.8C13.8 16 15 17 17 17h4M3 17h3c2 0 3.2-1 4.2-2.6M13.8 9.6C14.8 8 16 7 18 7h3M18.5 4.5 21 7l-2.5 2.5M18.5 14.5 21 17l-2.5 2.5"/>"#;
const REPEAT: &str = r#"<path d="M17 3l3.5 3.5L17 10"/><path d="M3.5 11.5V10.5a4 4 0 0 1 4-4h13"/><path d="M7 21l-3.5-3.5L7 14"/><path d="M20.5 12.5v1a4 4 0 0 1-4 4h-13"/>"#;
const ONE: &str = r#"<path d="M11 10.5l1.5-1v5"/>"#;
const HEART: &str = r#"<path d="M12 20s-8-4.6-8-10.5A4.5 4.5 0 0 1 12 7a4.5 4.5 0 0 1 8 2.5C20 15.4 12 20 12 20z"/>"#;
const LIST_ADD: &str = r#"<path d="M3.5 6h12M3.5 11h12M3.5 16h7M18 13.5v7M14.5 17h7"/>"#;
const LIST_MUSIC: &str = r#"<path d="M3.5 6h11M3.5 11h11M3.5 16h6"/><circle cx="16" cy="18" r="2.5"/><path d="M18.5 18V9.5l3-1"/>"#;
const NOTE_FILE: &str = r#"<path d="M6 2.5h8l4.5 4.5v14.5H6z"/><path d="M14 2.5V7h4.5"/><circle cx="10.5" cy="16.5" r="2"/><path d="M12.5 16.5v-6l2.5 1"/>"#;
const RADIO: &str = r#"<circle cx="12" cy="12" r="1.5"/><path d="M8.5 8.5a5 5 0 0 0 0 7M15.5 8.5a5 5 0 0 1 0 7M5.5 5.5a9.2 9.2 0 0 0 0 13M18.5 5.5a9.2 9.2 0 0 1 0 13"/>"#;
const MUSIC_BOX: &str = r#"<rect x="3" y="3" width="18" height="18" rx="4"/><circle cx="10" cy="15.5" r="2"/><path d="M12 15.5V8l4 1.5"/>"#;
const CLIPBOARD: &str = r#"<rect x="5" y="4.5" width="14" height="17" rx="2"/><path d="M9 3h6v3H9zM12 10.5v6M9 13.5h6"/>"#;
const EXPLICIT: &str = r#"<rect x="4" y="4" width="16" height="16" rx="3"/><path d="M14.5 8h-5v8h5M9.5 12h4"/>"#;
const DEVICES: &str = r#"<rect x="2.5" y="4" width="13" height="10" rx="1.5"/><path d="M6 18h6M9 14v4"/><rect x="17" y="8" width="5" height="11" rx="1.5"/>"#;
const USER: &str = r#"<circle cx="12" cy="8" r="4"/><path d="M4.5 21a7.5 7.5 0 0 1 15 0"/>"#;
const GEAR: &str = r#"<circle cx="12" cy="12" r="3"/><path d="M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3M5.3 5.3l2.1 2.1M16.6 16.6l2.1 2.1M5.3 18.7l2.1-2.1M16.6 7.4l2.1-2.1"/>"#;
const CLOUD_ALERT: &str = r#"<path d="M7 18.5a4.5 4.5 0 0 1-.5-9 6 6 0 0 1 11.5 1.5 4 4 0 0 1-.5 7.5z"/><path d="M12 10.5v3.5M12 16.5h.01"/>"#;
const ALERT: &str = r#"<path d="M12 3 2.5 20h19z"/><path d="M12 9.5v5M12 17h.01"/>"#;
const BAN: &str = r#"<circle cx="12" cy="12" r="9"/><path d="M5.6 5.6l12.8 12.8"/>"#;
const HOURGLASS: &str = r#"<path d="M6.5 3h11M6.5 21h11M7.5 3c0 5 9 6 9 9s-9 4-9 9M16.5 3c0 5-9 6-9 9s9 4 9 9"/>"#;
const CHECK: &str = r#"<circle cx="12" cy="12" r="9"/><path d="M8 12.5l3 3 5-6"/>"#;
const FOLDER: &str = r#"<path d="M3 6.5A1.5 1.5 0 0 1 4.5 5H9l2 2.5h8.5A1.5 1.5 0 0 1 21 9v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 18z"/>"#;
const SPEAKER_BODY: &str = r#"<path d="M10 4.5 5 8.5H2.5v7H5l5 4z"/>"#;
const WAVES: &str = r#"<path d="M14 9a4.5 4.5 0 0 1 0 6M17 6.5a8 8 0 0 1 0 11"/>"#;
const CROSS: &str = r#"<path d="M14.5 9.5l5 5M19.5 9.5l-5 5"/>"#;

/// "?" badge for states the plugin doesn't know yet.
const UNKNOWN_BADGE: &str = r##"<circle cx="104" cy="24" r="11" fill="#6b7078" stroke="#0b0c0e" stroke-width="2"/><text x="104" y="30" text-anchor="middle" font-family="Segoe UI, DM Sans, sans-serif" font-size="15" font-weight="700" fill="#101214">?</text>"##;

/// Speaker with a level bar under it (volume-stack-<n>[-muted]).
fn volume_level(level: u32, muted: bool, size: Size) -> String {
    let state = if muted { RED } else { ACCENT };
    let rim = badge_colors(state).2;
    let g = format!("{SPEAKER_BODY}{}", if muted { CROSS } else { WAVES });
    let width = 68.0 * level.min(100) as f32 / 100.0;
    let bar = format!(r##"<rect x="38" y="101" width="68" height="8" rx="4" fill="#ffffff" fill-opacity="0.08"/><rect x="38" y="101" width="{width}" height="8" rx="4" fill="{rim}"/>"##);
    badge_tile(state, &g, size, "", if size == Size::Key { &bar } else { "" })
}

fn spotify_file(file: &str, size: Size) -> Option<String> {
    let name = file.strip_prefix("images/states/")?;
    if let Some(rest) = name.strip_prefix("volume-stack-") {
        let (n, muted) = rest.strip_suffix("-muted").map_or((rest, false), |n| (n, true));
        if let Ok(level) = n.parse::<u32>() {
            return Some(volume_level(level, muted, size));
        }
    }
    let (name, unknown) = name.strip_suffix("-unknown").map_or((name, false), |n| (n, true));
    let repeat_one = format!("{REPEAT}{ONE}");
    let vol_up = format!(r#"{SPEAKER_BODY}<path d="M14.5 12h6M17.5 9v6"/>"#);
    let vol_down = format!(r#"{SPEAKER_BODY}<path d="M14.5 12h6"/>"#);
    let vol_on = format!("{SPEAKER_BODY}{WAVES}");
    let vol_muted = format!("{SPEAKER_BODY}{CROSS}");
    let vol_low = format!(r#"{SPEAKER_BODY}<path d="M14 9a4.5 4.5 0 0 1 0 6"/>"#);
    let devices_off = with_slash(DEVICES);
    let (g, color, filled): (&str, &str, bool) = match name {
        "play" => (PLAY, ACCENT, false),
        "pause" => (PAUSE, GREEN, false),
        "next" => (NEXT, ACCENT, false),
        "previous" => (PREVIOUS, ACCENT, false),
        "forward-seek" => (FORWARD, ACCENT, false),
        "backward-seek" => (BACKWARD, ACCENT, false),
        "shuffle" | "shuffle-inactive" => (SHUFFLE, ACCENT, false),
        "shuffle-active" => (SHUFFLE, GREEN, false),
        "loop" | "loop-inactive" => (REPEAT, ACCENT, false),
        "loop-active" => (REPEAT, GREEN, false),
        "loop-one" | "loop-one-inactive" => (&repeat_one, ACCENT, false),
        "loop-one-active" => (&repeat_one, GREEN, false),
        "like" => (HEART, ACCENT, false),
        "unlike" => (HEART, GREEN, true),
        "add-to-playlist" => (LIST_ADD, ACCENT, false),
        "context-information" | "items" | "playlists" => (LIST_MUSIC, ACCENT, false),
        "song-stack" => (NOTE_FILE, ACCENT, false),
        "play-context" => (RADIO, ACCENT, false),
        "playback-control" => (MUSIC_BOX, ACCENT, false),
        "song-clipboard" => (CLIPBOARD, ACCENT, false),
        "song-explicit" => (EXPLICIT, RED, false),
        "song-not-explicit" => (EXPLICIT, GREY, false),
        "transfer-playback" => (DEVICES, ACCENT, false),
        "transfer-playback-offline" => (&devices_off, RED, false),
        "user-information" => (USER, ACCENT, false),
        "volume-up" => (&vol_up, ACCENT, false),
        "volume-down" => (&vol_down, ACCENT, false),
        "volume-mute" | "volume-stack" => (&vol_on, ACCENT, false),
        "volume-unmute" => (&vol_muted, RED, false),
        "set-volume" => (&vol_low, ACCENT, false),
        "volume-control" => (MIXER, ACCENT, false),
        "setup" => (GEAR, ACCENT, false),
        "no-setup" => (GEAR, GREY, false),
        "setup-error" => (GEAR, RED, false),
        "api-error" => (CLOUD_ALERT, RED, false),
        "api-rate-limited" => (CLOCK, RED, false),
        "fatal-error" => (ALERT, RED, false),
        "no-device-error" => (&devices_off, RED, false),
        "not-available" => (BAN, GREY, false),
        "busy" | "pending" => (HOURGLASS, GREY, false),
        "success" => (CHECK, GREEN, false),
        "local" => (FOLDER, ACCENT, false),
        _ => return None,
    };
    let state = if unknown { GREY } else { color };
    // a filled shape (liked heart) keeps the white outline over a translucent white fill
    let g = if filled { g.replace("<path ", r##"<path fill="#ffffff" fill-opacity="0.35" "##) } else { g.to_string() };
    Some(badge_tile(state, &g, size, "", if unknown { UNKNOWN_BADGE } else { "" }))
}

// ---------- public API ----------

/// Skin for an image a plugin shows on a key.
/// `image` is the plugin-relative file reference as written in the manifest or sent with
/// setImage ("actions/mute_0", "./actions/video_0_off.svg"…); data URLs never match.
/// `list` = small icon for the editor lists (no captions).
pub fn skin_svg(plugin: &str, action: &str, image: &str, list: bool) -> Option<String> {
    if image.starts_with("data:") {
        return None;
    }
    let size = if list { Size::List } else { Size::Key };
    match plugin {
        "me.amankhanna.oasystem" => system_tile(action, list),
        "me.amankhanna.oadiscord" => discord_file(&base(image), size),
        "com.ntanis.essentials-for-spotify" => spotify_file(&base(image), size),
        _ => None,
    }
}

/// Category / plugin icon in the editor.
pub fn plugin_icon_svg(plugin: &str) -> Option<String> {
    if plugin == "me.amankhanna.oadiscord" {
        // the white logo on the badge, no stroke glyph
        let logo = discord_logo("#ffffff").replace(r#"transform="translate(-16 50) scale(4.4)""#, r#"transform="translate(36 39) scale(3)""#).replace(r#"fill-opacity="0.14""#, r#"fill-opacity="1""#);
        return Some(badge_tile(ACCENT, "", Size::List, "", &logo));
    }
    let g = match plugin {
        "me.amankhanna.oasystem" => PULSE,
        "com.ntanis.essentials-for-spotify" => MUSIC_BOX,
        _ => return None,
    };
    Some(badge_tile(ACCENT, g, Size::List, "", ""))
}

/// Where the live title goes on a skinned key when the manifest doesn't say (under the badge).
pub fn title_align(action: &str) -> Option<&'static str> {
    action.starts_with("me.amankhanna.oasystem.").then_some("bottom")
}

pub fn data_url(svg: &str) -> String {
    format!("data:image/svg+xml;base64,{}", STANDARD.encode(svg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::image::decode_plugin_image;

    fn render(svg: &str) -> image::RgbaImage {
        decode_plugin_image(&data_url(svg), None).expect("skin renders")
    }

    const DISCORD: [&str; 26] = [
        "mute_0", "mute_1", "deafen_0", "deafen_1", "pushtotalk_0", "pushtotalk_1", "pushtomute_0", "pushtomute_1",
        "ptttoggle_0", "ptttoggle_1", "video_0", "video_0_off", "video_1", "stream_0", "stream_0_off", "stream_1",
        "volumeControl", "userVolumeControl", "headphonesAndMicrophone", "headphonesAndMicrophone_1", "textchannel",
        "voicechannel_0", "voicechannel_1", "soundboard", "notifications", "serverStats",
    ];

    #[test]
    fn every_discord_file_is_skinned_and_renders() {
        for f in DISCORD {
            let svg = skin_svg("me.amankhanna.oadiscord", "x", &format!("actions/{f}"), false).unwrap_or_else(|| panic!("{f}"));
            let img = render(&svg);
            let mid = img.get_pixel(20, 72);
            assert!(mid[0] < 70, "{f}: key stays dark {mid:?}");
        }
    }

    #[test]
    fn state_colours_follow_meaning() {
        let edge = |f: &str| *render(&skin_svg("me.amankhanna.oadiscord", "x", &format!("actions/{f}"), false).unwrap()).get_pixel(72, 22); // badge rim
        let (idle, muted, live, off) = (edge("mute_0"), edge("mute_1"), edge("video_1"), edge("video_0_off"));
        assert!(idle[0] > 200 && idle[1] > 120 && idle[2] < 110, "accent {idle:?}");
        assert!(muted[0] > 200 && muted[1] < 140, "red {muted:?}");
        assert!(live[1] > 180 && live[0] < 140, "green {live:?}");
        assert!(off[0] < 140 && off[0].abs_diff(off[2]) < 30, "grey {off:?}");
    }

    #[test]
    fn file_references_are_normalised() {
        let a = skin_svg("me.amankhanna.oadiscord", "x", "actions/mute_1", false);
        for v in ["./actions/mute_1.svg", "actions/mute_1@2x.png", "actions\\mute_1.png", " actions/mute_1 "] {
            assert_eq!(skin_svg("me.amankhanna.oadiscord", "x", v, false), a, "{v}");
        }
        assert!(skin_svg("me.amankhanna.oadiscord", "x", "data:image/svg+xml,<svg/>", false).is_none());
        assert!(skin_svg("me.amankhanna.oadiscord", "x", "actions/unknown", false).is_none());
    }

    #[test]
    fn system_information_goes_by_action() {
        for a in ["cpu", "ram", "uptime", "os"] {
            let uuid = format!("me.amankhanna.oasystem.{a}");
            for list in [false, true] {
                render(&skin_svg("me.amankhanna.oasystem", &uuid, "icon", list).unwrap());
            }
            assert_eq!(title_align(&uuid), Some("bottom"));
        }
        assert!(skin_svg("me.amankhanna.oasystem", "me.amankhanna.oasystem.gpu", "icon", false).is_none());
    }

    pub(super) const SPOTIFY: &[&str] = &["add-to-playlist", "add-to-playlist-unknown", "api-error", "api-rate-limited", "backward-seek", "busy", "context-information", "context-information-unknown", "fatal-error", "forward-seek", "items", "like", "like-unknown", "local", "loop-active", "loop-inactive", "loop-one-active", "loop-one-inactive", "loop-one-unknown", "loop-unknown", "next", "no-device-error", "no-setup", "not-available", "pause", "pending", "play", "play-context", "play-context-unknown", "playback-control", "playlists", "previous", "set-volume", "setup", "setup-error", "shuffle-active", "shuffle-inactive", "shuffle-unknown", "song-clipboard", "song-explicit", "song-explicit-unknown", "song-not-explicit", "song-stack", "song-stack-unknown", "success", "transfer-playback", "transfer-playback-offline", "transfer-playback-unknown", "unlike", "user-information", "user-information-unknown", "volume-control", "volume-down", "volume-mute", "volume-mute-unknown", "volume-stack-10", "volume-stack-50-muted", "volume-stack-100", "volume-stack-unknown", "volume-unmute", "volume-up"];

    #[test]
    fn spotify_states_are_skinned() {
        for f in SPOTIFY {
            let svg = skin_svg("com.ntanis.essentials-for-spotify", "x", &format!("images/states/{f}.png"), false).unwrap_or_else(|| panic!("{f}"));
            render(&svg);
        }
        // composites the plugin builds from parts are left to the generic look
        for f in ["long", "mode-stack-1-0-1", "volume-stack-abc"] {
            assert!(skin_svg("com.ntanis.essentials-for-spotify", "x", &format!("images/states/{f}"), false).is_none(), "{f}");
        }
        let bar = |svg: String| render(&svg).get_pixel(42, 105)[0];
        assert!(bar(volume_level(100, false, Size::Key)) > 180 && bar(volume_level(0, false, Size::Key)) < 80, "level bar follows the volume");
    }

    #[test]
    fn other_plugins_and_icons() {
        assert!(skin_svg("com.elgato.cpu", "com.elgato.cpu.action", "icon", false).is_none());
        assert!(plugin_icon_svg("me.amankhanna.oadiscord").is_some());
        assert!(plugin_icon_svg("com.elgato.obsstudio").is_none());
        assert!(title_align("me.amankhanna.oadiscord.togglemute").is_none());
        let f = render(&frame_svg());
        assert!(f.get_pixel(72, 18)[0] > 180, "gold rim of the generic badge");
    }
}

#[cfg(test)]
mod preview {
    /// `SKIN_PREVIEW=<dir> cargo test --lib skins::preview -- --ignored` writes a contact sheet.
    #[test]
    #[ignore]
    fn write_skin_sheet() {
        use super::*;
        let dir = std::path::PathBuf::from(std::env::var("SKIN_PREVIEW").unwrap());
        let mut items: Vec<(String, String)> = super::tests_list().into_iter().collect();
        items.push(("frame".into(), frame_svg()));
        let mut sheet = image::RgbaImage::from_pixel(8 * 152, items.len().div_ceil(8) as u32 * 152, image::Rgba([12, 13, 15, 255]));
        for (i, (_, svg)) in items.iter().enumerate() {
            let img = crate::plugins::image::decode_plugin_image(&data_url(svg), None).unwrap();
            image::imageops::overlay(&mut sheet, &img, (i % 8) as i64 * 152 + 4, (i / 8) as i64 * 152 + 4);
        }
        sheet.save(dir.join("skins.png")).unwrap();
    }
}

#[cfg(test)]
fn tests_list() -> Vec<(String, String)> {
    let mut v = Vec::new();
    for a in ["cpu", "ram", "uptime", "os"] {
        v.push((a.into(), skin_svg("me.amankhanna.oasystem", &format!("me.amankhanna.oasystem.{a}"), "icon", false).unwrap()));
    }
    for f in [
        "mute_0", "mute_1", "deafen_0", "deafen_1", "pushtotalk_0", "pushtotalk_1", "pushtomute_0", "pushtomute_1",
        "ptttoggle_0", "ptttoggle_1", "video_0", "video_0_off", "video_1", "stream_0", "stream_0_off", "stream_1",
        "volumeControl", "userVolumeControl", "headphonesAndMicrophone", "headphonesAndMicrophone_1", "textchannel",
        "voicechannel_0", "voicechannel_1", "soundboard", "notifications", "serverStats",
    ] {
        v.push((f.into(), skin_svg("me.amankhanna.oadiscord", "x", &format!("actions/{f}"), false).unwrap()));
    }
    for f in tests::SPOTIFY {
        v.push((f.to_string(), skin_svg("com.ntanis.essentials-for-spotify", "x", &format!("images/states/{f}"), false).unwrap()));
    }
    v
}
