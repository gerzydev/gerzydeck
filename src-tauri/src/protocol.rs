//! Wire protocol of the Fifine AmpliGame D6 (VID 0x3142, PID 0x0060, firmware V3).
//! Verified on hardware: see docs/superpowers/specs (Ověřeno na hardwaru).

pub const PACKET_SIZE: usize = 1024;
pub const NUM_KEYS: u8 = 15;
pub const VENDOR_ID: u16 = 0x3142;
pub const PRODUCT_ID: u16 = 0x0060;
pub const USAGE_PAGE: u16 = 0xFFA0;

/// "CRT\0\0" + body, zero-padded to 1024 bytes.
fn command(body: &[u8]) -> Vec<u8> {
    let mut p = vec![0u8; PACKET_SIZE];
    p[..5].copy_from_slice(b"CRT\0\0");
    p[5..5 + body.len()].copy_from_slice(body);
    p
}

pub fn connect() -> Vec<u8> { command(b"CONNECT\0\0\0") }
pub fn blef() -> Vec<u8> { command(b"BLEF\0") }
pub fn mode() -> Vec<u8> { command(b"MOD\0\0\x01") }
pub fn wake() -> Vec<u8> { command(b"DIS\0\0\x01") }
pub fn clear() -> Vec<u8> { command(b"CLE\0\0\0\xff") }
pub fn stop() -> Vec<u8> { command(b"STP\0\0") }
pub fn pixmap_end() -> Vec<u8> { command(b"PIXMAPEND") }
pub fn sleep() -> Vec<u8> { command(b"SLP\0\0") }

pub fn brightness(level: u8) -> Vec<u8> {
    command(&[b'L', b'I', b'G', 0, 0, level.min(100)])
}

/// Logical key (1 = top-left, row-major) → LCD slot. Panel rows are mounted bottom-up.
pub fn key_to_slot(key: u8) -> u8 {
    match key {
        1..=5 => key + 10,
        11..=15 => key - 10,
        _ => key,
    }
}

pub fn image_upload(key: u8, jpeg: &[u8]) -> Vec<Vec<u8>> {
    assert!((1..=NUM_KEYS).contains(&key), "key out of range");
    assert!(jpeg.len() <= 0xFFFF, "jpeg too large");
    let len = jpeg.len() as u16;
    let mut out = vec![command(&[b'B', b'A', b'T', 0, 0, (len >> 8) as u8, len as u8, key_to_slot(key)])];
    for chunk in jpeg.chunks(PACKET_SIZE) {
        let mut p = vec![0u8; PACKET_SIZE];
        p[..chunk.len()].copy_from_slice(chunk);
        out.push(p);
    }
    out.push(stop());
    out
}

/// Boot logo (800×480 JPEG, already rotated 180°), stored in the deck's flash.
/// Layout copied from the official Fifine app: "CRT\0\0LOG\0\0" + big-endian length, then the JPEG
/// in zero-padded 1024-byte packets. The device needs ~3 s between the command and the data,
/// answers "ACK\0\0OK" when done, then gets STP (see device.rs).
pub fn boot_logo(jpeg: &[u8]) -> Result<(Vec<u8>, Vec<Vec<u8>>), String> {
    if jpeg.len() < 4 || jpeg[..3] != [0xFF, 0xD8, 0xFF] {
        return Err("Boot logo musí být JPEG".into());
    }
    // the official app sent 2 length bytes; stay inside them
    if jpeg.len() > 0xFFFF {
        return Err("Boot logo je moc velké (max 64 kB)".into());
    }
    let len = jpeg.len() as u16;
    let cmd = command(&[b'L', b'O', b'G', 0, 0, (len >> 8) as u8, len as u8]);
    let packets = jpeg
        .chunks(PACKET_SIZE)
        .map(|c| {
            let mut p = vec![0u8; PACKET_SIZE];
            p[..c.len()].copy_from_slice(c);
            p
        })
        .collect();
    Ok((cmd, packets))
}

pub fn init_sequence(level: u8) -> Vec<Vec<u8>> {
    vec![connect(), blef(), mode(), wake(), brightness(level), clear(), stop()]
}

pub fn batch_end() -> Vec<Vec<u8>> {
    vec![pixmap_end(), stop()]
}

/// Windows hidapi needs the report-ID byte (0) in front of the payload.
pub fn with_report_id(p: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(p.len() + 1);
    v.push(0);
    v.extend_from_slice(p);
    v
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Ack,
    Key { device_id: u8, key: u8, pressed: bool },
}

/// `key_map[device_id - 1]` = logical key; `None` = identity.
pub fn parse_input(buf: &[u8], key_map: Option<&[u8; 15]>) -> Option<Input> {
    let buf = if buf.len() >= 12 && buf[0] == 0 && &buf[1..4] == b"ACK" { &buf[1..] } else { buf };
    if buf.len() < 11 || &buf[0..3] != b"ACK" || &buf[5..7] != b"OK" {
        return None;
    }
    let id = buf[9];
    if id == 0 || id == 0xFF {
        return Some(Input::Ack);
    }
    if !(1..=NUM_KEYS).contains(&id) {
        return None;
    }
    let key = key_map.map_or(id, |m| m[(id - 1) as usize]);
    Some(Input::Key { device_id: id, key, pressed: buf[10] == 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_logo_packets_match_the_official_app_byte_for_byte() {
        // captured from the Fifine app (USBPcap) while it replaced the boot logo
        let jpeg = include_bytes!("../tests/fixtures/bootlogo.jpg");
        let cmd = include_bytes!("../tests/fixtures/bootlogo-cmd.bin");
        let data = include_bytes!("../tests/fixtures/bootlogo-data.bin");
        let (c, packets) = boot_logo(jpeg).unwrap();
        assert_eq!(c.as_slice(), cmd.as_slice());
        assert_eq!(packets.len(), 47);
        assert!(packets.iter().all(|p| p.len() == PACKET_SIZE));
        assert_eq!(packets.concat().as_slice(), data.as_slice());
    }

    #[test]
    fn boot_logo_rejects_non_jpeg_and_oversized_data() {
        assert!(boot_logo(b"not a jpeg").is_err());
        let mut big = vec![0xFF, 0xD8, 0xFF];
        big.resize(70_000, 0);
        assert!(boot_logo(&big).is_err());
    }

    fn crt(s: &[u8]) -> Vec<u8> {
        let mut v = b"CRT\0\0".to_vec();
        v.extend_from_slice(s);
        v
    }

    #[test]
    fn fixed_packets_have_exact_prefixes() {
        assert_eq!(connect().len(), 1024);
        assert_eq!(&connect()[..15], &crt(b"CONNECT\0\0\0")[..]);
        assert_eq!(&wake()[..11], &crt(b"DIS\0\0\x01")[..]);
        assert_eq!(&mode()[..11], &crt(b"MOD\0\0\x01")[..]);
        assert_eq!(&clear()[..12], &crt(b"CLE\0\0\0\xff")[..]);
        assert_eq!(&pixmap_end()[..14], &crt(b"PIXMAPEND")[..]);
        assert!(stop()[10..].iter().all(|&b| b == 0));
    }

    #[test]
    fn brightness_clamps() {
        assert_eq!(brightness(150)[10], 100);
        assert_eq!(&brightness(42)[..11], &crt(b"LIG\0\0\x2a")[..]);
    }

    #[test]
    fn slot_map() {
        let got: Vec<u8> = [1, 5, 6, 10, 11, 15].iter().map(|&k| key_to_slot(k)).collect();
        assert_eq!(got, vec![11, 15, 6, 10, 1, 5]);
    }

    #[test]
    fn image_upload_framing() {
        let jpeg = vec![7u8; 2500];
        let p = image_upload(1, &jpeg);
        assert_eq!(p.len(), 1 + 3 + 1);
        let mut hdr = crt(b"BAT\0\0");
        hdr.extend_from_slice(&[0x09, 0xC4, 11]);
        assert_eq!(&p[0][..13], &hdr[..]);
        assert_eq!(p[3][2500 - 2048 - 1], 7);
        assert_eq!(p[3][2500 - 2048], 0);
        assert_eq!(&p[4][..8], &crt(b"STP")[..]);
        assert!(p.iter().all(|x| x.len() == 1024));
    }

    #[test]
    fn init_sequence_and_batch_end() {
        let names: Vec<String> = init_sequence(55)
            .iter()
            .map(|p| String::from_utf8_lossy(&p[5..9]).trim_end_matches('\0').to_string())
            .collect();
        assert_eq!(names, vec!["CONN", "BLEF", "MOD", "DIS", "LIG", "CLE", "STP"]);
        assert_eq!(init_sequence(55)[4][10], 55);
        let end: Vec<Vec<u8>> = batch_end().iter().map(|p| p[5..8].to_vec()).collect();
        assert_eq!(end, vec![b"PIX".to_vec(), b"STP".to_vec()]);
    }

    #[test]
    fn report_id_prefix() {
        let w = with_report_id(&connect());
        assert_eq!(w.len(), 1025);
        assert_eq!(w[0], 0);
        assert_eq!(w[1], b'C');
    }

    fn report(id: u8, st: u8) -> Vec<u8> {
        let mut v = b"ACK\0\0OK\0\0".to_vec();
        v.extend_from_slice(&[id, st, 0xff]);
        v.resize(512, 0);
        v
    }

    #[test]
    fn parse_input_cases() {
        assert_eq!(parse_input(&report(3, 1), None), Some(Input::Key { device_id: 3, key: 3, pressed: true }));
        assert_eq!(parse_input(&report(3, 0), None), Some(Input::Key { device_id: 3, key: 3, pressed: false }));
        assert_eq!(parse_input(&report(0xff, 0xff), None), Some(Input::Ack));
        let mut prefixed = vec![0u8];
        prefixed.extend(report(15, 1));
        assert!(matches!(parse_input(&prefixed, None), Some(Input::Key { key: 15, .. })));
        assert_eq!(parse_input(b"nope", None), None);
        assert_eq!(parse_input(&report(16, 1), None), None);
        let flipped = [11, 12, 13, 14, 15, 6, 7, 8, 9, 10, 1, 2, 3, 4, 5];
        assert!(matches!(parse_input(&report(1, 1), Some(&flipped)), Some(Input::Key { key: 11, .. })));
    }
}
