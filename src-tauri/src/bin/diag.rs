//! Hardware diagnostics for the D6.
//!   cargo run --bin diag                  numbers 1–15 on the keys + log presses
//!   cargo run --bin diag -- --no-report-id   write without the leading report-ID byte
//!   cargo run --bin diag -- --type        keyboard output test (focus Notepad within 3 s)
use gerzys_deck::device::{spawn_device, DeckOptions, DeviceCmd, DeviceEvent, HidApiBackend};
use gerzys_deck::protocol;
use gerzys_deck::render::{render_key_jpeg, KeySpec};
use std::sync::mpsc::channel;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--text") {
        use gerzys_deck::keyboard::{Keyboard, SendInputKeyboard};
        std::thread::sleep(std::time::Duration::from_millis(300));
        SendInputKeyboard.type_text(args.get(i + 1).map(String::as_str).unwrap_or(""));
        return;
    }
    if args.iter().any(|a| a == "--type") {
        type_test();
        return;
    }

    println!("Rozhraní D6:");
    if let Ok(api) = hidapi::HidApi::new() {
        for d in api.device_list().filter(|d| d.vendor_id() == protocol::VENDOR_ID && d.product_id() == protocol::PRODUCT_ID) {
            println!("  usagePage=0x{:x} usage={} interface={} {}", d.usage_page(), d.usage(), d.interface_number(), d.path().to_string_lossy());
        }
    }

    let frame = args.iter().any(|a| a == "--frame");
    let frame104 = args.iter().any(|a| a == "--frame104");
    let opts = DeckOptions { report_id_prefix: !args.iter().any(|a| a == "--no-report-id"), ..DeckOptions::default() };
    let (etx, erx) = channel();
    let (cmd, _handle) = spawn_device(HidApiBackend::default(), opts, etx);
    println!("Ctrl+C ukončí.");
    for ev in erx {
        match ev {
            DeviceEvent::Connected => {
                println!("Připojeno, nahrávám čísla 1–15…");
                let images: Vec<(u8, Vec<u8>)> = if frame104 {
                    let mut big = image::RgbImage::from_pixel(104, 104, image::Rgb([255, 0, 255]));
                    image::imageops::overlay(&mut big, &frame_pattern(), 4, 4);
                    let tile = gerzys_deck::render::encode_tile(&big);
                    (1..=15u8).map(|k| (k, tile.clone())).collect()
                } else if frame {
                    let tile = gerzys_deck::render::encode_tile(&frame_pattern());
                    (1..=15u8).map(|k| (k, tile.clone())).collect()
                } else {
                    (1..=15u8)
                        .map(|k| {
                            let bg = match k { 1..=5 => [0x1f, 0x5f, 0x3a], 6..=10 => [0x1f, 0x3a, 0x5f], _ => [0x5f, 0x1f, 0x3a] };
                            (k, render_key_jpeg(&KeySpec { image: None, label: k.to_string(), bg }))
                        })
                        .collect()
                };
                let _ = cmd.send(DeviceCmd::SetImages(images));
            }
            DeviceEvent::Disconnected => println!("Odpojeno, čekám…"),
            DeviceEvent::Raw { device_id, pressed } => {
                print!("raw id=0x{device_id:02x} {} ", if pressed { "down" } else { "up  " });
            }
            DeviceEvent::Key { key, .. } => println!("→ logická klávesa {key}"),
            DeviceEvent::Redraw => {}
        }
    }
}

fn type_test() {
    use gerzys_deck::keyboard::{Keyboard, SendInputKeyboard};
    let kb = SendInputKeyboard;
    println!("Za 3 s píšu do okna v popředí.");
    std::thread::sleep(std::time::Duration::from_secs(3));
    let tap = |c: &str| {
        kb.down(c);
        kb.up(c);
    };
    kb.down("ShiftLeft");
    tap("KeyH");
    kb.up("ShiftLeft");
    tap("KeyY");
    tap("Digit1");
    tap("Space");
    kb.type_text("ěščř");
    println!("Hotovo: na české klávesnici má být „Hz+ ěščř“.");
}

/// Nested 2 px frames every 4 px: red (edge), orange, yellow, green, blue — shows how much of the 100×100 tile is visible.
fn frame_pattern() -> image::RgbImage {
    let colors = [[255, 0, 0], [255, 140, 0], [255, 230, 0], [0, 200, 60], [0, 120, 255]];
    let mut img = image::RgbImage::from_pixel(100, 100, image::Rgb([255, 255, 255]));
    for (i, c) in colors.iter().enumerate() {
        let o = (i * 4) as u32;
        for y in o..100 - o {
            for x in o..100 - o {
                let d = (x - o).min(y - o).min(99 - o - x).min(99 - o - y);
                if d < 2 {
                    img.put_pixel(x, y, image::Rgb(*c));
                }
            }
        }
    }
    img
}
