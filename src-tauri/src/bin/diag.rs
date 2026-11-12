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

    let opts = DeckOptions { report_id_prefix: !args.iter().any(|a| a == "--no-report-id"), ..DeckOptions::default() };
    let (etx, erx) = channel();
    let (cmd, _handle) = spawn_device(HidApiBackend::default(), opts, etx);
    println!("Ctrl+C ukončí.");
    for ev in erx {
        match ev {
            DeviceEvent::Connected => {
                println!("Připojeno, nahrávám čísla 1–15…");
                let images = (1..=15u8)
                    .map(|k| {
                        let bg = match k { 1..=5 => [0x1f, 0x5f, 0x3a], 6..=10 => [0x1f, 0x3a, 0x5f], _ => [0x5f, 0x1f, 0x3a] };
                        (k, render_key_jpeg(&KeySpec { image: None, label: k.to_string(), bg }))
                    })
                    .collect();
                let _ = cmd.send(DeviceCmd::SetImages(images));
            }
            DeviceEvent::Disconnected => println!("Odpojeno, čekám…"),
            DeviceEvent::Raw { device_id, pressed } => {
                print!("raw id=0x{device_id:02x} {} ", if pressed { "down" } else { "up  " });
            }
            DeviceEvent::Key { key, .. } => println!("→ logická klávesa {key}"),
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
