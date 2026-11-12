//! Key image pipeline: uploads → normalized PNG, key config → device-safe JPEG.

use ab_glyph::{FontVec, PxScale};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::{self, FilterType};
use image::{ImageFormat, ImageReader, Limits, Rgb, RgbImage};
use imageproc::drawing::{draw_text_mut, text_size};
use std::io::Cursor;
use std::sync::OnceLock;

/// Measured on the D6 (firmware V3): the key panel shows 104×104 px; 100×100 left a 4 px stale strip top/left.
pub const KEY_SIZE: u32 = 104;
pub const UPLOAD_SIZE: u32 = 200;
pub const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024;
pub const DEFAULT_BG: [u8; 3] = [0x14, 0x16, 0x18];
const LABEL_COLOR: Rgb<u8> = Rgb([0xEE, 0xF0, 0xF2]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    BadImage(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::BadImage(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for RenderError {}

#[derive(Debug, Clone, Default)]
pub struct KeySpec {
    pub image: Option<Vec<u8>>,
    pub label: String,
    pub bg: [u8; 3],
}

pub fn parse_hex(s: &str) -> [u8; 3] {
    let h = s.trim_start_matches('#');
    if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap();
        [p(0), p(2), p(4)]
    } else {
        DEFAULT_BG
    }
}

fn decode(bytes: &[u8]) -> Option<image::DynamicImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().ok() // GIF/WebP: first frame
}

/// Any image → 200×200 PNG, centre cover crop, alpha kept.
pub fn normalize_upload(bytes: &[u8]) -> Result<Vec<u8>, RenderError> {
    if bytes.is_empty() {
        return Err(RenderError::BadImage("Prázdný soubor".into()));
    }
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(RenderError::BadImage("Obrázek je větší než 20 MB".into()));
    }
    let img = decode(bytes).ok_or_else(|| RenderError::BadImage("Soubor není podporovaný obrázek".into()))?;
    let img = img.resize_to_fill(UPLOAD_SIZE, UPLOAD_SIZE, FilterType::Lanczos3);
    let mut out = Vec::new();
    img.to_rgba8()
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|e| RenderError::BadImage(e.to_string()))?;
    Ok(out)
}

pub(crate) fn font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        ["segoeuib.ttf", "arialbd.ttf", "segoeui.ttf", "arial.ttf"].iter().find_map(|name| {
            let path = std::path::Path::new("C:\\Windows\\Fonts").join(name);
            std::fs::read(path).ok().and_then(|b| FontVec::try_from_vec(b).ok())
        })
    })
    .as_ref()
}

fn shorten(label: &str) -> String {
    if label.chars().count() > 12 {
        label.chars().take(11).chain(std::iter::once('…')).collect()
    } else {
        label.to_string()
    }
}

/// The bottom label of a key with a picture, exactly as on normal keys (plugin tiles use it too).
pub fn draw_key_label(canvas: &mut RgbImage, label: &str) {
    draw_label(canvas, label, true);
}

fn draw_label(canvas: &mut RgbImage, label: &str, with_image: bool) {
    let Some(font) = font() else {
        log::warn!("no system font found, skipping key label");
        return;
    };
    let text = shorten(label);
    let scale = PxScale::from(if with_image { 15.0 } else { 19.0 });
    let (w, h) = text_size(scale, font, &text);
    if with_image {
        // dark gradient strip so the label stays readable on any picture
        let strip = KEY_SIZE * 62 / 100;
        for y in strip..KEY_SIZE {
            let a = (y - strip) as f32 / (KEY_SIZE - strip) as f32 * 0.75;
            for x in 0..KEY_SIZE {
                let p = canvas.get_pixel_mut(x, y);
                for c in 0..3 {
                    p[c] = (p[c] as f32 * (1.0 - a)) as u8;
                }
            }
        }
    }
    let x = (KEY_SIZE as i32 - w as i32) / 2;
    let y = if with_image { KEY_SIZE as i32 - 6 - h as i32 } else { (KEY_SIZE as i32 - h as i32) / 2 };
    draw_text_mut(canvas, LABEL_COLOR, x.max(1), y, scale, font, &text);
}

/// Matches the editor preview (12 px on a 92 px tile).
const CORNER_RADIUS: f32 = 14.0;

/// Fades pixels outside a rounded rectangle to black, anti-aliased with 4×4 supersampling.
fn round_corners(img: &mut RgbImage, r: f32) {
    let size = img.width() as f32;
    let coverage = |px: u32, py: u32| -> f32 {
        let mut inside = 0;
        for sy in 0..4 {
            for sx in 0..4 {
                let x = px as f32 + (sx as f32 + 0.5) / 4.0;
                let y = py as f32 + (sy as f32 + 0.5) / 4.0;
                let cx = x.clamp(r, size - r);
                let cy = y.clamp(r, size - r);
                if (x - cx).powi(2) + (y - cy).powi(2) <= r * r {
                    inside += 1;
                }
            }
        }
        inside as f32 / 16.0
    };
    let edge = r.ceil() as u32;
    for y in 0..img.height() {
        for x in 0..img.width() {
            let near_x = x < edge || x >= img.width() - edge;
            let near_y = y < edge || y >= img.height() - edge;
            if !(near_x && near_y) {
                continue;
            }
            let a = coverage(x, y);
            if a < 1.0 {
                let p = img.get_pixel_mut(x, y);
                for c in 0..3 {
                    p[c] = (p[c] as f32 * a).round() as u8;
                }
            }
        }
    }
}

/// One key → 100×100 baseline JPEG, rotated 180° for the inverted panels. Never fails.
pub fn render_key_jpeg(spec: &KeySpec) -> Vec<u8> {
    let bg = Rgb(spec.bg);
    let mut canvas = RgbImage::from_pixel(KEY_SIZE, KEY_SIZE, bg);
    let mut has_image = false;
    if let Some(img) = spec.image.as_deref().and_then(decode) {
        let img = img.resize_to_fill(KEY_SIZE, KEY_SIZE, FilterType::Lanczos3).to_rgba8();
        for (x, y, p) in img.enumerate_pixels() {
            let a = p[3] as u16;
            let dst = canvas.get_pixel_mut(x, y);
            for c in 0..3 {
                dst[c] = ((p[c] as u16 * a + spec.bg[c] as u16 * (255 - a)) / 255) as u8;
            }
        }
        has_image = true;
    }
    if !spec.label.is_empty() {
        draw_label(&mut canvas, &spec.label, has_image);
    }
    round_corners(&mut canvas, CORNER_RADIUS);
    encode_tile(&canvas)
}

/// Round a tile, then encode it for the device.
pub fn tile_jpeg(canvas: &RgbImage) -> Vec<u8> {
    let mut canvas = canvas.clone();
    round_corners(&mut canvas, CORNER_RADIUS);
    encode_tile(&canvas)
}

/// 100×100 canvas as seen by the user → device JPEG (rotated 180°, baseline, JFIF).
pub fn encode_tile(canvas: &RgbImage) -> Vec<u8> {
    let rotated = imageops::rotate180(canvas);
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode_image(&rotated)
        .expect("in-memory JPEG encode");
    clean_baseline_jpeg(&jpeg)
}

/// Standard JFIF 1.01 APP0, no thumbnail. The D6 decoder rejects JPEGs without it.
const JFIF_APP0: [u8; 18] = [0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00];

/// Keeps only SOI, APP0 (JFIF), DQT, SOF0, DHT and SOS…EOI; inserts JFIF when missing.
pub fn clean_baseline_jpeg(data: &[u8]) -> Vec<u8> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return data.to_vec();
    }
    let mut out = vec![0xFF, 0xD8];
    if !(data[2] == 0xFF && data[3] == 0xE0) {
        out.extend_from_slice(&JFIF_APP0);
    }
    let n = data.len();
    let mut i = 2;
    while i < n {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        while i < n && data[i] == 0xFF {
            i += 1;
        }
        if i >= n {
            break;
        }
        let marker = data[i];
        i += 1;
        if marker == 0xD9 {
            out.extend_from_slice(&[0xFF, 0xD9]);
            return out;
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            continue;
        }
        if i + 2 > n {
            break;
        }
        let len = ((data[i] as usize) << 8) | data[i + 1] as usize;
        if marker == 0xDA {
            out.extend_from_slice(&[0xFF, 0xDA]);
            out.extend_from_slice(&data[i..]);
            if !out.ends_with(&[0xFF, 0xD9]) {
                out.extend_from_slice(&[0xFF, 0xD9]);
            }
            return out;
        }
        if matches!(marker, 0xDB | 0xC0 | 0xC4 | 0xE0) {
            out.extend_from_slice(&[0xFF, marker]);
            out.extend_from_slice(&data[i..(i + len).min(n)]);
        }
        i += len;
    }
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

/// Second bytes of header markers, in order, up to and including SOS (0xDA).
pub fn jpeg_markers(buf: &[u8]) -> Vec<u8> {
    let mut m = Vec::new();
    if buf.len() < 2 || buf[0] != 0xFF || buf[1] != 0xD8 {
        return m;
    }
    m.push(0xD8);
    let mut i = 2;
    while i + 3 < buf.len() && buf[i] == 0xFF {
        let k = buf[i + 1];
        m.push(k);
        if k == 0xDA {
            break;
        }
        i += 2 + (((buf[i + 2] as usize) << 8) | buf[i + 3] as usize);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
    use std::io::Cursor;

    fn png_of(img: &image::DynamicImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn rendered_key_is_device_safe() {
        let j = render_key_jpeg(&KeySpec { image: None, label: "Fireball".into(), bg: [0x33, 0x66, 0x99] });
        let m = jpeg_markers(&j);
        assert_eq!(m[0], 0xD8);
        assert_eq!(m[1], 0xE0);
        assert!(m.contains(&0xC0));
        assert!(!m.contains(&0xC2) && !m.contains(&0xDD) && !m.contains(&0xE1));
        for w in j.windows(2) {
            assert!(!(w[0] == 0xFF && (0xD0..=0xD7).contains(&w[1])), "RST marker");
        }
        let img = image::load_from_memory(&j).unwrap();
        // The D6 panel is 104×104 (measured on hardware with a magenta margin), not 100×100.
        assert_eq!((img.width(), img.height()), (104, 104));
        assert!(j.len() < 0xFFFF);
    }

    #[test]
    fn image_is_rotated_180() {
        let mut src = RgbImage::from_pixel(200, 200, Rgb([0, 0, 0]));
        for y in 0..100 {
            for x in 0..200 {
                src.put_pixel(x, y, Rgb([255, 0, 0]));
            }
        }
        let j = render_key_jpeg(&KeySpec { image: Some(png_of(&src.into())), label: String::new(), bg: [0, 0, 0] });
        let out = image::load_from_memory(&j).unwrap().to_rgb8();
        assert!(out.get_pixel(50, 90)[0] > 200);
        assert!(out.get_pixel(50, 10)[0] < 50);
    }

    #[test]
    fn normalize_transparent_png_gif_and_junk() {
        let t = RgbaImage::from_pixel(640, 300, Rgba([0, 0, 0, 0]));
        let n = normalize_upload(&png_of(&t.into())).unwrap();
        assert_eq!(image::guess_format(&n).unwrap(), ImageFormat::Png);
        let img = image::load_from_memory(&n).unwrap();
        assert_eq!((img.width(), img.height()), (200, 200));

        let g = RgbImage::from_pixel(50, 50, Rgb([0, 255, 0]));
        let mut gif = Vec::new();
        image::DynamicImage::from(g).write_to(&mut Cursor::new(&mut gif), ImageFormat::Gif).unwrap();
        assert_eq!(image::load_from_memory(&normalize_upload(&gif).unwrap()).unwrap().width(), 200);

        assert!(matches!(normalize_upload(b"hello"), Err(RenderError::BadImage(_))));
        assert!(matches!(normalize_upload(&[]), Err(RenderError::BadImage(_))));
    }

    #[test]
    fn broken_image_falls_back_to_bg_and_label() {
        let j = render_key_jpeg(&KeySpec { image: Some(b"not an image".to_vec()), label: "X".into(), bg: [10, 20, 30] });
        assert!(image::load_from_memory(&j).is_ok());
    }

    #[test]
    fn clean_drops_exif_and_dri_and_inserts_jfif() {
        let seg = |m: u8, payload: &[u8]| {
            let mut v = vec![0xFF, m, 0, (payload.len() + 2) as u8];
            v.extend_from_slice(payload);
            v
        };
        let mut src = vec![0xFF, 0xD8];
        src.extend(seg(0xE0, &[1, 2]));
        src.extend(seg(0xE1, &[9, 9, 9]));
        src.extend(seg(0xDD, &[0, 4]));
        src.extend(seg(0xDB, &[5]));
        src.extend([0xFF, 0xDA, 0, 2, 0x11, 0x22, 0xFF, 0xD9]);
        assert_eq!(jpeg_markers(&clean_baseline_jpeg(&src)), vec![0xD8, 0xE0, 0xDB, 0xDA]);

        let mut no_app0 = vec![0xFF, 0xD8];
        no_app0.extend(seg(0xDB, &[5]));
        no_app0.extend([0xFF, 0xDA, 0, 2, 0x11, 0xFF, 0xD9]);
        assert_eq!(jpeg_markers(&clean_baseline_jpeg(&no_app0)), vec![0xD8, 0xE0, 0xDB, 0xDA]);
    }

    /// The tile on the deck should look like the editor preview: rounded corners, black outside.
    #[test]
    fn tile_has_rounded_black_corners() {
        let white = image::RgbImage::from_pixel(200, 200, Rgb([255, 255, 255]));
        let j = render_key_jpeg(&KeySpec { image: Some(png_of(&white.into())), label: String::new(), bg: [255, 255, 255] });
        let out = image::load_from_memory(&j).unwrap().to_rgb8();
        for (x, y) in [(0, 0), (103, 0), (0, 103), (103, 103), (2, 2)] {
            assert!(out.get_pixel(x, y)[0] < 40, "corner ({x},{y}) not black: {:?}", out.get_pixel(x, y));
        }
        for (x, y) in [(52, 52), (52, 1), (1, 52), (52, 102)] {
            assert!(out.get_pixel(x, y)[0] > 215, "edge/centre ({x},{y}) should keep the image: {:?}", out.get_pixel(x, y));
        }
    }

    /// Chroma subsampling would make coloured edges blocky at 100×100 (guard; the image encoder is 4:4:4).
    #[test]
    fn key_jpeg_has_full_chroma_resolution() {
        let j = render_key_jpeg(&KeySpec { image: None, label: "Ab".into(), bg: [0x33, 0x66, 0x99] });
        let sof = j.windows(2).position(|w| w == [0xFF, 0xC0]).expect("SOF0") + 2;
        let ncomp = j[sof + 7] as usize;
        assert_eq!(ncomp, 3);
        for c in 0..ncomp {
            assert_eq!(j[sof + 8 + c * 3 + 1], 0x11, "component {c} is subsampled");
        }
    }

    #[test]
    fn hex_colors() {
        assert_eq!(parse_hex("#336699"), [0x33, 0x66, 0x99]);
        assert_eq!(parse_hex("bogus"), [0x14, 0x16, 0x18]);
    }
}

#[cfg(test)]
mod preview {
    use super::*;

    /// `cargo test --lib render::preview -- --ignored` writes key previews to %TEMP%\deck-preview.png
    #[test]
    #[ignore]
    fn write_preview() {
        let purple = image::RgbImage::from_pixel(200, 200, Rgb([0x8a, 0x2b, 0xe2]));
        let mut png = Vec::new();
        image::DynamicImage::from(purple).write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        let extra = std::env::var("DECK_PREVIEW_IMAGE").ok().and_then(|p| std::fs::read(p).ok());
        let specs = [
            KeySpec { image: None, label: "Fireball".into(), bg: [0x33, 0x66, 0x99] },
            KeySpec { image: Some(png), label: "Ohnivá koule".into(), bg: DEFAULT_BG },
            KeySpec { image: extra, label: String::new(), bg: DEFAULT_BG },
        ];
        let mut sheet = RgbImage::from_pixel(340, 100, Rgb([255, 255, 255]));
        for (i, s) in specs.iter().enumerate() {
            let tile = imageops::rotate180(&image::load_from_memory(&render_key_jpeg(s)).unwrap().to_rgb8());
            imageops::overlay(&mut sheet, &tile, i as i64 * 120, 0);
        }
        sheet.save(std::env::temp_dir().join("deck-preview.png")).unwrap();
    }
}
