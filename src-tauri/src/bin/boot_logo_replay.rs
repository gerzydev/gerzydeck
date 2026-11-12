//! Writes the boot logo captured from the official Fifine app (tests/fixtures/bootlogo.jpg)
//! through our device thread: the same bytes the official app sent, as a known-good first write.
//!   cargo run --bin boot_logo_replay            (close Gerzy's deck and the Fifine app first)
//!   cargo run --bin boot_logo_replay -- <file>  any image, converted like the app does
use gerzys_deck::device::{spawn_device, DeckOptions, DeviceCmd, DeviceEvent, HidApiBackend};
use std::sync::mpsc::channel;
use std::time::Duration;

fn main() {
    let jpeg = match std::env::args().nth(1) {
        Some(path) => gerzys_deck::render::boot_logo_jpeg(&std::fs::read(path).expect("read image")).expect("convert image"),
        None => include_bytes!("../../tests/fixtures/bootlogo.jpg").to_vec(),
    };
    let (etx, erx) = channel();
    let (cmd, handle) = spawn_device(HidApiBackend::default(), DeckOptions::default(), etx);
    println!("Čekám na deck…");
    loop {
        match erx.recv_timeout(Duration::from_secs(10)) {
            Ok(DeviceEvent::Connected) => break,
            Ok(_) => continue,
            Err(_) => {
                println!("Deck nenalezen (je zavřená aplikace Fifine i Gerzy's deck?)");
                return;
            }
        }
    }
    std::thread::sleep(Duration::from_millis(500));
    println!("Nahrávám boot logo ({} B)…", jpeg.len());
    let (r, rx) = channel();
    cmd.send(DeviceCmd::SetBootLogo { jpeg, reply: r }).unwrap();
    match rx.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(())) => println!("Hotovo: deck potvrdil uložení."),
        Ok(Err(e)) => println!("Chyba: {e}"),
        Err(_) => println!("Chyba: bez odpovědi"),
    }
    let _ = cmd.send(DeviceCmd::Stop);
    let _ = handle.join();
}
