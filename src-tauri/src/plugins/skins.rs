//! Built-in look for popular plugins, in the app's own style (dark key, coloured rounded frame,
//! stroke glyph). Skins replace a plugin's *own image files* — manifest state images and the
//! files a plugin later picks with setImage — so keys still change with the plugin's state
//! (a muted mic turns red, a running screen share green). Images a plugin draws itself
//! (data URLs) are not skinned here; the engine gives those the generic frame instead.
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

/// Key frame. `inner` is drawn on top; `color` tints the frame and its glow.
pub fn frame_with(color: &str, inner: &str) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144">
<defs>
<linearGradient id="k" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#22262a"/><stop offset="1" stop-color="#101214"/></linearGradient>
<radialGradient id="l" cx="0.5" cy="0" r="0.8"><stop offset="0" stop-color="{color}" stop-opacity="0.22"/><stop offset="1" stop-color="{color}" stop-opacity="0"/></radialGradient>
<filter id="g" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="3"/></filter>
</defs>
<rect width="144" height="144" fill="url(#k)"/>
<rect width="144" height="144" fill="url(#l)"/>
<rect x="7" y="7" width="130" height="130" rx="20" fill="none" stroke="{color}" stroke-opacity="0.45" stroke-width="5" filter="url(#g)"/>
<rect x="7" y="7" width="130" height="130" rx="20" fill="none" stroke="{color}" stroke-width="2.5"/>
<rect x="8.5" y="8.5" width="127" height="127" rx="18.5" fill="none" stroke="#ffffff" stroke-opacity="0.06" stroke-width="1"/>
{inner}
</svg>"##
    )
}

/// The empty accent frame: base for the generic look of images plugins draw themselves.
pub fn frame_svg() -> String {
    frame_with(ACCENT, "")
}

fn glyph(g: &str, color: &str, x: f32, y: f32, scale: f32) -> String {
    format!(r#"<g transform="translate({x} {y}) scale({scale})" fill="none" stroke="{color}" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">{g}</g>"#)
}

fn caption(text: &str, y: f32) -> String {
    format!(r##"<text x="72" y="{y}" text-anchor="middle" font-family="Segoe UI, DM Sans, sans-serif" font-size="14" font-weight="700" letter-spacing="2" fill="#8a9098">{text}</text>"##)
}

/// Big centred glyph, optional small caption under it.
fn centred(g: &str, color: &str, cap: Option<&str>) -> String {
    match cap {
        None => frame_with(color, &glyph(g, color, 42.0, 42.0, 2.5)),
        Some(c) => frame_with(color, &format!("{}{}", glyph(g, color, 45.0, 30.0, 2.25), caption(c, 118.0))),
    }
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
const CHAT: &str = r#"<path d="M4 5.5h16a1.5 1.5 0 0 1 1.5 1.5v9a1.5 1.5 0 0 1-1.5 1.5h-9l-5 4v-4H4A1.5 1.5 0 0 1 2.5 16V7A1.5 1.5 0 0 1 4 5.5z"/><path d="M8 11.5h.01M12 11.5h.01M16 11.5h.01"/>"#;
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
    let (g, name) = match action {
        "me.amankhanna.oasystem.cpu" => (CPU, "CPU"),
        "me.amankhanna.oasystem.ram" => (RAM, "RAM"),
        "me.amankhanna.oasystem.uptime" => (CLOCK, "UPTIME"),
        "me.amankhanna.oasystem.os" => (MONITOR, "OS"),
        _ => return None,
    };
    Some(if list {
        frame_with(ACCENT, &glyph(g, ACCENT, 36.0, 36.0, 3.0))
    } else {
        // glyph on top, the live value in the middle (title_align), caption at the bottom
        frame_with(ACCENT, &format!("{}{}", glyph(g, ACCENT, 57.0, 17.0, 1.25), caption(name, 121.0)))
    })
}

// ---------- Discord (me.amankhanna.oadiscord) ----------
// State 0 = idle (accent), state 1 = active: red when it cuts you off, green when you're live.
// "_off" files (and headphonesAndMicrophone_1) are sent while Discord isn't connected / in a call.

fn discord_file(file: &str) -> Option<String> {
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
    Some(centred(g, color, cap))
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
    match plugin {
        "me.amankhanna.oasystem" => system_tile(action, list),
        "me.amankhanna.oadiscord" => discord_file(&base(image)),
        _ => None,
    }
}

/// Category / plugin icon in the editor.
pub fn plugin_icon_svg(plugin: &str) -> Option<String> {
    let g = match plugin {
        "me.amankhanna.oasystem" => PULSE,
        "me.amankhanna.oadiscord" => CHAT,
        _ => return None,
    };
    Some(frame_with(ACCENT, &glyph(g, ACCENT, 36.0, 36.0, 3.0)))
}

/// Where the live title goes on a skinned key when the manifest doesn't say.
pub fn title_align(action: &str) -> Option<&'static str> {
    action.starts_with("me.amankhanna.oasystem.").then_some("middle")
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
        let edge = |f: &str| *render(&skin_svg("me.amankhanna.oadiscord", "x", &format!("actions/{f}"), false).unwrap()).get_pixel(72, 7);
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
            assert_eq!(title_align(&uuid), Some("middle"));
        }
        assert!(skin_svg("me.amankhanna.oasystem", "me.amankhanna.oasystem.gpu", "icon", false).is_none());
    }

    #[test]
    fn other_plugins_and_icons() {
        assert!(skin_svg("com.elgato.cpu", "com.elgato.cpu.action", "icon", false).is_none());
        assert!(plugin_icon_svg("me.amankhanna.oadiscord").is_some());
        assert!(plugin_icon_svg("com.elgato.obsstudio").is_none());
        assert!(title_align("me.amankhanna.oadiscord.togglemute").is_none());
        let f = render(&frame_svg());
        assert!(f.get_pixel(72, 7)[0] > 200);
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
    v
}
