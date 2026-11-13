//! Bounded plugin image decoding and title composition in user-facing orientation.
use base64::{engine::general_purpose::STANDARD, Engine};
use image::{
    imageops::FilterType, GrayImage, ImageFormat, ImageReader, Limits, Luma, Rgb, RgbImage,
    RgbaImage,
};
use imageproc::drawing::{draw_text_mut, text_size};
use std::{
    fs::File,
    io::{Cursor, Read},
    path::Path,
    sync::{Arc, OnceLock},
};

const MAX_INPUT: usize = 4 * 1024 * 1024;
const MAX_RASTER_ALLOC: u64 = 64 * 1024 * 1024;

/// Decode supported data URLs or a confined plugin-relative image path.
pub fn decode_plugin_image(src: &str, plugin_dir: Option<&Path>) -> Option<RgbaImage> {
    if src.len() > MAX_INPUT || src.is_empty() {
        return None;
    }
    if let Some(url) = src.strip_prefix("data:") {
        // Plugins send loosely formed data URLs (no params, upper-case charset, image/jpg,
        // wrapped base64, PNG labelled as JPEG). Accept them; the decoders below stay bounded.
        let (header, data) = url.split_once(',')?;
        let mut parts = header.split(';').map(|p| p.trim().to_ascii_lowercase());
        let mime = parts.next().unwrap_or_default();
        let base64 = parts.any(|p| p == "base64");
        let bytes = if base64 {
            let clean: String = data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
            STANDARD
                .decode(&clean)
                .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(clean.trim_end_matches('=')))
                .ok()?
        } else {
            percent_decode(data).unwrap_or_else(|| data.as_bytes().to_vec())
        };
        let looks_svg = || {
            let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
            head.contains("<svg")
        };
        return match mime.as_str() {
            "image/svg+xml" => svg(&bytes),
            "image/png" | "image/jpeg" | "image/jpg" => raster(&bytes, None),
            "" if looks_svg() => svg(&bytes),
            "" => raster(&bytes, None),
            _ => None,
        };
    }
    let path = super::manifest::resolve_image(plugin_dir?, src)?;
    let mut bytes = Vec::new();
    let file = File::open(&path).ok()?;
    if file.metadata().ok()?.len() > MAX_INPUT as u64 {
        return None;
    }
    file.take(MAX_INPUT as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_INPUT {
        return None;
    }
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
    {
        svg(&bytes)
    } else {
        raster(&bytes, None)
    }
}

fn percent_decode(src: &str) -> Option<Vec<u8>> {
    let bytes = src.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let a = (*bytes.get(i + 1)? as char).to_digit(16)?;
            let b = (*bytes.get(i + 2)? as char).to_digit(16)?;
            out.push((a * 16 + b) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}

fn raster(bytes: &[u8], mime: Option<&str>) -> Option<RgbaImage> {
    if bytes.len() > MAX_INPUT {
        return None;
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let format = reader.format()?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        return None;
    }
    if mime.is_some_and(|m| {
        !matches!(
            (m, format),
            ("image/png", ImageFormat::Png) | ("image/jpeg", ImageFormat::Jpeg)
        )
    }) {
        return None;
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(MAX_RASTER_ALLOC);
    reader.limits(limits);
    reader.decode().ok().map(|img| img.to_rgba8())
}

fn svg(bytes: &[u8]) -> Option<RgbaImage> {
    if bytes.len() > MAX_INPUT {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    let fonts = FONTS.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    });
    let opts = resvg::usvg::Options {
        font_family: "Segoe UI".into(),
        fontdb: Arc::clone(fonts),
        max_depth: 128,
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            // SVGs never read local paths or URLs. Embedded PNG/JPEG pass the
            // same bounded decoder before resvg is allowed to decode them.
            resolve_string: Box::new(|_, _| None),
            resolve_data: Box::new(|mime, data, _| {
                raster(&data, Some(mime))?;
                match mime {
                    "image/png" => Some(resvg::usvg::ImageKind::PNG(data)),
                    "image/jpeg" => Some(resvg::usvg::ImageKind::JPEG(data)),
                    _ => None,
                }
            }),
        },
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_str(text, &opts).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(144, 144)?;
    let transform = resvg::tiny_skia::Transform::from_scale(
        144.0 / tree.size().width(),
        144.0 / tree.size().height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut data = pixmap.take();
    for p in data.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a != 0 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(144, 144, data)
}

/// Shared, rasterised base for title-only keys and the generic theme.
pub fn theme_frame() -> Arc<RgbaImage> {
    static FRAME: OnceLock<Arc<RgbaImage>> = OnceLock::new();
    FRAME.get_or_init(|| Arc::new(decode_plugin_image(&super::skins::data_url(&super::skins::frame_svg()), None).expect("built-in plugin frame"))).clone()
}

/// Only sparse, neutral light glyphs qualify. Coloured logos and full pictures stay intact.
fn monochrome_glyph(img: &RgbaImage) -> bool {
    let mut bright = 0u64;
    let mut neutral = 0u64;
    let mut light = 0u64;
    for p in img.pixels().filter(|p| p[3] >= 16) {
        let hi = *p.0[..3].iter().max().unwrap() as u16;
        let lo = *p.0[..3].iter().min().unwrap() as u16;
        if hi >= 96 {
            bright += 1;
            if hi - lo <= hi / 6 { neutral += 1; }
            if lo >= 160 { light += 1; }
        }
    }
    let area = img.width() as u64 * img.height() as u64;
    bright > 0 && bright * 100 <= area * 65 && neutral * 100 >= bright * 96 && light * 100 >= bright * 75
}

/// Put any plugin image on the generic badge: simple light glyphs become clean white (like the
/// skins' glyphs), coloured artwork keeps its colours.
pub fn theme_image(img: &RgbaImage) -> RgbaImage {
    let mut frame = (*theme_frame()).clone();
    if img.width() == 0 || img.height() == 0 { return frame; }
    let glyph = monochrome_glyph(img);
    let mut source = img.clone();
    if glyph {
        // Transparent glyphs keep alpha and shading. On dark opaque backgrounds the
        // luminance becomes the mask, removing the dark box behind the glyph.
        let transparent = source.pixels().any(|p| p[3] < 16);
        for p in source.pixels_mut() {
            let level = (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3;
            if transparent {
                p[0] = level as u8;
                p[1] = level as u8;
                p[2] = level as u8;
            } else {
                p[3] = (p[3] as u32 * level.saturating_sub(32) / 223) as u8;
                p[0] = 255; p[1] = 255; p[2] = 255;
            }
        }
    }
    let max = source.width().max(source.height()) as u64;
    let w = (source.width() as u64 * 104 / max).max(1) as u32;
    let h = (source.height() as u64 * 104 / max).max(1) as u32;
    let mut inner = image::imageops::resize(&source, w, h, FilterType::Lanczos3);
    let r = 12.0f32.min(w.min(h) as f32 / 2.0);
    for (x, y, p) in inner.enumerate_pixels_mut() {
        let dx = (r - (x as f32 + 0.5)).max(0.0).max(x as f32 + 0.5 - (w as f32 - r));
        let dy = (r - (y as f32 + 0.5)).max(0.0).max(y as f32 + 0.5 - (h as f32 - r));
        let coverage = (r + 0.5 - dx.hypot(dy)).clamp(0.0, 1.0);
        p[3] = (p[3] as f32 * coverage).round() as u8;
    }
    image::imageops::overlay(&mut frame, &inner, ((144 - w) / 2) as i64, ((144 - h) / 2) as i64);
    frame
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    Top,
    Middle,
    #[default]
    Bottom,
}

#[derive(Debug, Clone)]
pub struct TitleParams {
    pub show: bool,
    pub size: u32,
    pub color: [u8; 3],
    pub align: Align,
    pub bold: bool,
}
impl Default for TitleParams {
    fn default() -> Self {
        Self {
            show: true,
            size: 9,
            color: [255; 3],
            align: Align::Bottom,
            bold: false,
        }
    }
}

/// Compose a 104px tile. Rounded corners and device rotation belong to tile_jpeg.
pub fn compose_tile(
    base: Option<&RgbaImage>,
    bg: [u8; 3],
    title: Option<&str>,
    params: &TitleParams,
) -> RgbImage {
    let size = crate::render::KEY_SIZE;
    let mut canvas = RgbImage::from_pixel(size, size, Rgb(bg));
    if let Some(base) = base.filter(|b| b.width() > 0 && b.height() > 0) {
        let scaled = image::imageops::resize(base, size, size, FilterType::Lanczos3);
        for (x, y, p) in scaled.enumerate_pixels() {
            let a = p[3] as u16;
            let target = canvas.get_pixel_mut(x, y);
            for c in 0..3 {
                target[c] = ((p[c] as u16 * a + bg[c] as u16 * (255 - a)) / 255) as u8;
            }
        }
    }
    if !params.show || params.size == 0 {
        return canvas;
    }
    let Some(font) = crate::render::font() else {
        return canvas;
    };
    let Some(title) = title else {
        return canvas;
    };
    // Bound work and arithmetic for untrusted title settings; ordinary SDK sizes
    // retain their exact point-to-pixel scale.
    let title: String = title.chars().take(4096).collect();
    let mut scale = ab_glyph::PxScale::from(params.size.min(256) as f32 * size as f32 / 72.0);
    let lines: Vec<&str> = title.split('\n').take(size as usize).collect();
    // shrink long lines to fit the tile width (keeps a margin for the outline)
    let widest = lines.iter().map(|l| text_size(scale, font, l).0).max().unwrap_or(0) as f32;
    let available = size as f32 - 8.0;
    if widest > available {
        scale = ab_glyph::PxScale::from((scale.y * available / widest).max(8.0));
    }
    let line_height = scale.y.ceil() as i32 + 1;
    let block_height = line_height * lines.len() as i32;
    let y0 = match params.align {
        Align::Top => 1,
        Align::Middle => (size as i32 - block_height) / 2,
        Align::Bottom => size as i32 - block_height - 1,
    };
    let mut mask = GrayImage::new(size, size);
    for (i, line) in lines.iter().enumerate() {
        let (width, _) = text_size(scale, font, line);
        let x = (size as i64 - width as i64) / 2;
        draw_text_mut(
            &mut mask,
            Luma([255]),
            x as i32,
            y0 + i as i32 * line_height,
            scale,
            font,
            line,
        );
    }
    if params.bold {
        // The shared loader supplies its preferred system face; one-pixel
        // horizontal emboldening gives the SDK bold flag a visible effect.
        let normal = mask.clone();
        for y in 0..size {
            for x in 1..size {
                mask.get_pixel_mut(x, y)[0] =
                    normal.get_pixel(x, y)[0].max(normal.get_pixel(x - 1, y)[0]);
            }
        }
    }
    let blend = |dst: &mut Rgb<u8>, color: [u8; 3], alpha: u8| {
        for c in 0..3 {
            dst[c] = ((dst[c] as u16 * (255 - alpha as u16) + color[c] as u16 * alpha as u16) / 255)
                as u8;
        }
    };
    for y in 0..size {
        for x in 0..size {
            let mut outline = 0;
            for ny in y.saturating_sub(1)..=(y + 1).min(size - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(size - 1) {
                    outline = outline.max(mask.get_pixel(nx, ny)[0]);
                }
            }
            blend(canvas.get_pixel_mut(x, y), [20; 3], outline);
        }
    }
    for (x, y, p) in mask.enumerate_pixels() {
        blend(canvas.get_pixel_mut(x, y), params.color, p[0]);
    }
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use image::{ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;

    fn png() -> Vec<u8> {
        let img = RgbaImage::from_pixel(2, 2, Rgba([255, 0, 0, 255]));
        let mut bytes = Cursor::new(Vec::new());
        img.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }
    /// Long live values ("16.2GB used of 32GB") must shrink to fit instead of overflowing.
    #[test]
    fn long_titles_shrink_to_fit_the_tile() {
        let params = TitleParams { size: 16, color: [255; 3], ..TitleParams::default() };
        let t = compose_tile(None, [0; 3], Some("16.2GB used of 32GB total"), &params);
        let edge = crate::render::KEY_SIZE - 1;
        for y in 0..=edge {
            for x in [0, 1, edge - 1, edge] {
                assert!(t.get_pixel(x, y)[0] < 128, "text touches the edge at ({x},{y})");
            }
        }
        // still visible
        assert!(t.pixels().any(|p| p[0] > 200));
    }

    /// Real plugins send loosely formed data URLs; all of these must decode.
    #[test]
    fn lenient_data_urls_from_real_plugins() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>"#;
        let b64 = STANDARD.encode(png());
        let wrapped: String = b64.as_bytes().chunks(8).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("\n");
        for src in [
            format!("data:image/svg+xml,{svg}"),
            format!("data:image/svg+xml;charset=UTF-8,{}", svg.replace('#', "%23")),
            format!("data:image/svg+xml;utf8,{svg}"),
            format!("data:image/jpg;base64,{b64}"),
            format!("data:image/jpeg;base64,{b64}"), // PNG bytes labelled as JPEG
            format!("data:image/png;base64,{wrapped}"),
            format!("data:image/png;charset=utf-8;base64,{b64}"),
            format!("data:;base64,{b64}"),
        ] {
            assert!(decode_plugin_image(&src, None).is_some(), "{}", &src[..src.len().min(60)]);
        }
    }

    #[test]
    fn png_data_url_decodes() {
        let src = format!("data:image/png;base64,{}", STANDARD.encode(png()));
        let img = decode_plugin_image(&src, None).unwrap();
        assert_eq!(img.dimensions(), (2, 2));
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
    }
    #[test]
    fn jpeg_data_url_and_raster_limits() {
        let mut jpeg = Cursor::new(Vec::new());
        image::RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0]))
            .write_to(&mut jpeg, ImageFormat::Jpeg)
            .unwrap();
        assert!(decode_plugin_image(
            &format!("data:image/jpeg;base64,{}", STANDARD.encode(jpeg.get_ref())),
            None
        )
        .is_some());
        // MIME mismatch (PNG labelled as JPEG) is tolerated: real plugins do this.
        assert!(decode_plugin_image(
            &format!("data:image/jpeg;base64,{}", STANDARD.encode(png())),
            None
        )
        .is_some());
        // non-image mime types are still refused
        assert!(decode_plugin_image(&format!("data:text/html;base64,{}", STANDARD.encode(png())), None).is_none());
        let mut wide = Cursor::new(Vec::new());
        RgbaImage::new(4097, 1)
            .write_to(&mut wide, ImageFormat::Png)
            .unwrap();
        assert!(decode_plugin_image(
            &format!("data:image/png;base64,{}", STANDARD.encode(wide.get_ref())),
            None
        )
        .is_none());
    }
    #[test]
    fn svg_external_files_ignored_and_alpha_unpremultiplied() {
        let path =
            std::env::temp_dir().join(format!("gd-external-image-{}.png", std::process::id()));
        std::fs::write(&path, png()).unwrap();
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><image href="{}" width="20" height="20"/></svg>"#,
            path.to_string_lossy().replace('\\', "/")
        );
        let img = decode_plugin_image(
            &format!("data:image/svg+xml;base64,{}", STANDARD.encode(svg)),
            None,
        )
        .unwrap();
        assert!(img.pixels().all(|p| p[3] == 0));
        std::fs::remove_file(path).unwrap();
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20" fill="red" opacity="0.5"/></svg>"#;
        let img = decode_plugin_image(
            &format!("data:image/svg+xml;base64,{}", STANDARD.encode(svg)),
            None,
        )
        .unwrap();
        assert_eq!(img.get_pixel(72, 72)[0], 255);
        assert!((127..=128).contains(&img.get_pixel(72, 72)[3]));
    }
    #[test]
    fn svg_base64_and_urlencoded_render_red() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20" fill="red"/></svg>"#;
        let encoded: String = svg.bytes().map(|b| format!("%{b:02X}")).collect();
        for src in [
            format!("data:image/svg+xml;base64,{}", STANDARD.encode(svg)),
            format!("data:image/svg+xml;charset=utf8,{encoded}"),
        ] {
            let img = decode_plugin_image(&src, None).unwrap();
            assert_eq!(img.dimensions(), (144, 144));
            assert_eq!(img.get_pixel(72, 72).0, [255, 0, 0, 255]);
        }
    }
    #[test]
    fn invalid_and_oversized_inputs_are_rejected() {
        for src in [
            "garbage",
            "data:image/png;base64,no!",
            "data:image/svg+xml;charset=utf8,%FF",
            "data:image/svg+xml;charset=utf8,%X0",
        ] {
            assert!(decode_plugin_image(src, None).is_none(), "{src}");
        }
        assert!(decode_plugin_image(&"x".repeat(4 * 1024 * 1024 + 1), None).is_none());
    }
    #[test]
    fn confined_files_and_traversal() {
        let dir = std::env::temp_dir().join(format!("gd-image-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("icon@2x.png"), png()).unwrap();
        assert!(decode_plugin_image("icon", Some(&dir)).is_some());
        for src in ["../icon", "../../x", "C:\\Windows\\x", "\\\\server\\x"] {
            assert!(decode_plugin_image(src, Some(&dir)).is_none());
        }
        std::fs::write(dir.join("large.png"), vec![0; 4 * 1024 * 1024 + 1]).unwrap();
        assert!(decode_plugin_image("large.png", Some(&dir)).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
    fn bright_in(img: &image::RgbImage, from: u32, to: u32) -> usize {
        img.enumerate_pixels()
            .filter(|(_, y, p)| *y >= from && *y < to && p[0] > 180)
            .count()
    }
    #[test]
    fn top_bottom_title_and_hidden() {
        assert!(
            crate::render::font().is_some(),
            "system font required for title test"
        );
        let mut params = TitleParams::default();
        params.align = Align::Top;
        let top = compose_tile(None, [0; 3], Some("Title"), &params);
        params.align = Align::Bottom;
        let bottom = compose_tile(None, [0; 3], Some("Title"), &params);
        assert_eq!(top.dimensions(), (104, 104));
        assert!(bright_in(&top, 0, 34) > 0);
        assert_eq!(bright_in(&top, 69, 104), 0);
        assert!(bright_in(&bottom, 69, 104) > 0);
        assert_eq!(bright_in(&bottom, 0, 34), 0);
        params.show = false;
        assert!(compose_tile(None, [0; 3], Some("Title"), &params)
            .pixels()
            .all(|p| p.0 == [0; 3]));
    }
    #[test]
    fn alpha_compositing_multiline_bold_and_jpeg() {
        let base = RgbaImage::from_pixel(2, 2, Rgba([200, 0, 0, 128]));
        let hidden = TitleParams {
            show: false,
            ..Default::default()
        };
        let tile = compose_tile(Some(&base), [0, 0, 200], None, &hidden);
        assert_eq!(tile.get_pixel(0, 0).0, [100, 0, 99]);
        let normal = compose_tile(
            None,
            [0; 3],
            Some("A\nB"),
            &TitleParams {
                align: Align::Middle,
                ..Default::default()
            },
        );
        let bold = compose_tile(
            None,
            [0; 3],
            Some("A\nB"),
            &TitleParams {
                align: Align::Middle,
                bold: true,
                ..Default::default()
            },
        );
        assert_ne!(normal, bold);
        let jpeg = crate::render::tile_jpeg(&tile);
        let decoded = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        assert_eq!(decoded.dimensions(), (104, 104));
        assert!(decoded.get_pixel(0, 0).0.iter().all(|v| *v < 30));
        assert_eq!(
            compose_tile(
                None,
                [0; 3],
                Some(&"x".repeat(5000)),
                &TitleParams {
                    size: u32::MAX,
                    ..Default::default()
                }
            )
            .dimensions(),
            (104, 104)
        );
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;
    use image::Rgba;
    fn glyph(bg: Rgba<u8>) -> RgbaImage {
        let mut img = RgbaImage::from_pixel(100, 100, bg);
        for y in 30..70 { for x in 40..60 { img.put_pixel(x, y, Rgba([255; 4])); } }
        img
    }
    #[test]
    fn white_transparent_and_dark_background_glyphs_stay_clean_white() {
        for bg in [Rgba([0, 0, 0, 0]), Rgba([15, 15, 15, 255])] {
            let img = glyph(bg);
            assert!(monochrome_glyph(&img));
            assert_eq!(theme_image(&img).get_pixel(72, 72).0, [255, 255, 255, 255]);
        }
    }
    #[test]
    fn transparent_glyph_keeps_alpha_and_shading() {
        let mut img = glyph(Rgba([0, 0, 0, 0]));
        img.put_pixel(50, 50, Rgba([200, 200, 200, 128]));
        // The sparse glyph still qualifies despite an antialiased/shaded edge.
        assert!(monochrome_glyph(&img));
        let themed = theme_image(&img);
        assert!(themed.get_pixel(72, 72)[0] < 255);
        assert!(themed.get_pixel(72, 72)[0] > 50);
    }
    #[test]
    fn colourful_photo_preserves_colours_and_gets_a_frame() {
        let img = RgbaImage::from_fn(104, 104, |x, y| Rgba([(x * 2) as u8, (y * 2) as u8, 180, 255]));
        assert!(!monochrome_glyph(&img));
        let themed = theme_image(&img);
        assert_eq!(themed.dimensions(), (144, 144));
        assert_eq!(themed.get_pixel(72, 72), img.get_pixel(52, 52));
        assert_eq!(themed.get_pixel(45, 61), img.get_pixel(25, 41));
        assert!(themed.get_pixel(18, 72)[0] > themed.get_pixel(18, 72)[2], "gold rim around the picture");
        assert_ne!(themed.get_pixel(20, 20), img.get_pixel(0, 0), "inner corners are clipped");
    }
    #[test]
    fn coloured_logo_and_full_white_picture_are_not_recoloured() {
        let mut img = glyph(Rgba([0; 4]));
        for p in img.pixels_mut().filter(|p| p[3] > 0) { p[1] = 40; p[2] = 40; }
        assert!(!monochrome_glyph(&img));
        assert!(!monochrome_glyph(&RgbaImage::from_pixel(20, 20, Rgba([255; 4]))));
    }
    #[test]
    fn empty_and_tiny_images_are_safe() {
        for img in [RgbaImage::new(0, 0), RgbaImage::new(0, 1), RgbaImage::new(1, 0), RgbaImage::from_pixel(1, 1, Rgba([255; 4]))] {
            assert_eq!(theme_image(&img).dimensions(), (144, 144));
        }
        assert!(Arc::ptr_eq(&theme_frame(), &theme_frame()));
    }
}
