//! D6 device thread: discovery, reconnect, paced writes, heartbeat, input events.

use crate::protocol::{self, Input};
use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub trait HidPort: Send {
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()>;
    /// Ok(0) on timeout.
    fn read_timeout(&mut self, buf: &mut [u8], ms: i32) -> anyhow::Result<usize>;
}

pub trait HidBackend: Send + 'static {
    fn find(&mut self) -> Option<Box<dyn HidPort>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    Connected,
    Disconnected,
    Key { key: u8, pressed: bool },
    Raw { device_id: u8, pressed: bool },
    /// the deck cleared its keys (after a boot logo upload): send every image again
    Redraw,
}

#[derive(Debug)]
pub enum DeviceCmd {
    SetImages(Vec<(u8, Vec<u8>)>),
    SetBrightness(u8),
    SetKeyMap(Option<[u8; 15]>),
    /// Writes the boot logo (800×480 JPEG, rotated) into the deck's flash.
    SetBootLogo { jpeg: Vec<u8>, reply: Sender<Result<(), String>> },
    Stop,
}

#[derive(Debug, Clone)]
pub struct DeckOptions {
    pub report_id_prefix: bool,
    pub write_delay: Duration,
    pub heartbeat: Duration,
    pub reconnect: Duration,
    pub brightness: u8,
    /// pause between the LOG command and its data (the deck prepares its flash)
    pub logo_erase_wait: Duration,
    /// how long to wait for the deck's ACK after the logo data
    pub logo_ack_timeout: Duration,
}

impl Default for DeckOptions {
    fn default() -> Self {
        Self {
            report_id_prefix: true,
            write_delay: Duration::from_millis(5),
            heartbeat: Duration::from_secs(2),
            reconnect: Duration::from_secs(2),
            brightness: 80,
            logo_erase_wait: Duration::from_secs(3),
            logo_ack_timeout: Duration::from_secs(10),
        }
    }
}

/// Real backend over hidapi.
#[derive(Default)]
pub struct HidApiBackend {
    api: Option<hidapi::HidApi>,
}

struct HidApiPort(hidapi::HidDevice);

impl HidPort for HidApiPort {
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.0.write(data)?;
        Ok(())
    }
    fn read_timeout(&mut self, buf: &mut [u8], ms: i32) -> anyhow::Result<usize> {
        Ok(self.0.read_timeout(buf, ms)?)
    }
}

impl HidBackend for HidApiBackend {
    fn find(&mut self) -> Option<Box<dyn HidPort>> {
        match &mut self.api {
            Some(api) => {
                if let Err(e) = api.refresh_devices() {
                    log::warn!("hid refresh failed: {e}");
                }
            }
            None => match hidapi::HidApi::new() {
                Ok(api) => self.api = Some(api),
                Err(e) => {
                    log::error!("hidapi init failed: {e}");
                    return None;
                }
            },
        }
        let api = self.api.as_ref()?;
        let info = api.device_list().find(|d| {
            d.vendor_id() == protocol::VENDOR_ID && d.product_id() == protocol::PRODUCT_ID && d.usage_page() == protocol::USAGE_PAGE
        })?;
        match info.open_device(api) {
            Ok(dev) => Some(Box::new(HidApiPort(dev))),
            Err(e) => {
                log::warn!("opening deck failed: {e}");
                None
            }
        }
    }
}

struct Worker<B: HidBackend> {
    backend: B,
    opts: DeckOptions,
    events: Sender<DeviceEvent>,
    cmds: Receiver<DeviceCmd>,
    key_map: Option<[u8; 15]>,
}

enum Exit {
    Stop,
    Disconnected,
}

impl<B: HidBackend> Worker<B> {
    fn frame(&self, p: &[u8]) -> Vec<u8> {
        if self.opts.report_id_prefix { protocol::with_report_id(p) } else { p.to_vec() }
    }

    /// Handles a command while no device is connected. Returns false on Stop.
    fn idle_cmd(&mut self, cmd: DeviceCmd) -> bool {
        match cmd {
            DeviceCmd::Stop => return false,
            DeviceCmd::SetBrightness(b) => self.opts.brightness = b,
            DeviceCmd::SetKeyMap(m) => self.key_map = m,
            DeviceCmd::SetImages(_) => {} // engine re-sends everything on Connected
            DeviceCmd::SetBootLogo { reply, .. } => {
                let _ = reply.send(Err(crate::i18n::tr("Deck není připojený", "The deck is not connected").into()));
            }
        }
        true
    }

    /// Forwards a key report read while a long operation runs; true when it was the deck's ACK.
    fn handle_read(&self, data: &[u8]) -> bool {
        match protocol::parse_input(data, self.key_map.as_ref()) {
            Some(Input::Ack) => true,
            Some(Input::Key { device_id, key, pressed }) => {
                let _ = self.events.send(DeviceEvent::Raw { device_id, pressed });
                let _ = self.events.send(DeviceEvent::Key { key, pressed });
                false
            }
            None => false,
        }
    }

    /// Reads for `dur`, forwarding keys; with `until_ack` returns early (Ok(true)) on the deck's ACK.
    fn read_for(&self, port: &mut dyn HidPort, dur: Duration, until_ack: bool) -> anyhow::Result<bool> {
        let deadline = Instant::now() + dur;
        let mut buf = [0u8; 513];
        while Instant::now() < deadline {
            let n = port.read_timeout(&mut buf, 20)?;
            if n > 0 && self.handle_read(&buf[..n]) && until_ack {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The official app's sequence: STP, LOG + length, ~3 s pause, data, ACK from the deck, STP,
    /// then (after another pause) CLE ×2 — the keys are blank afterwards and must be redrawn.
    /// Err(None) = the port failed (disconnected), Err(Some(msg)) = the deck refused / did not answer.
    fn write_boot_logo(&self, port: &mut dyn HidPort, jpeg: &[u8]) -> Result<(), Option<String>> {
        let (cmd, packets) = protocol::boot_logo(jpeg).map_err(Some)?;
        let io = |e: anyhow::Error| {
            log::warn!("deck write failed during boot logo: {e}");
            None
        };
        log::info!("boot logo: sending {} bytes", jpeg.len());
        port.write(&self.frame(&protocol::stop())).map_err(io)?;
        port.write(&self.frame(&cmd)).map_err(io)?;
        self.read_for(port, self.opts.logo_erase_wait, false).map_err(io)?;
        for p in &packets {
            port.write(&self.frame(p)).map_err(io)?;
        }
        let acked = self.read_for(port, self.opts.logo_ack_timeout, true).map_err(io)?;
        port.write(&self.frame(&protocol::stop())).map_err(io)?;
        if !acked {
            return Err(Some(crate::i18n::tr("Deck nepotvrdil uložení loga", "The deck did not confirm saving the logo").into()));
        }
        self.read_for(port, self.opts.logo_erase_wait, false).map_err(io)?;
        port.write(&self.frame(&protocol::clear())).map_err(io)?;
        port.write(&self.frame(&protocol::clear())).map_err(io)?;
        log::info!("boot logo: stored");
        Ok(())
    }

    fn run(mut self) {
        loop {
            let port = loop {
                if let Some(p) = self.backend.find() {
                    break p;
                }
                match self.cmds.recv_timeout(self.opts.reconnect) {
                    Ok(cmd) => {
                        if !self.idle_cmd(cmd) {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            };
            if self.events.send(DeviceEvent::Connected).is_err() {
                return;
            }
            match self.session(port) {
                Exit::Stop => return,
                Exit::Disconnected => {
                    if self.events.send(DeviceEvent::Disconnected).is_err() {
                        return;
                    }
                }
            }
        }
    }

    fn session(&mut self, mut port: Box<dyn HidPort>) -> Exit {
        let mut queue: VecDeque<Vec<u8>> = protocol::init_sequence(self.opts.brightness).into();
        let mut pending: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
        let mut last_heartbeat = Instant::now();
        let mut buf = [0u8; 513];
        let mut logo: Option<(Vec<u8>, Sender<Result<(), String>>)> = None;
        loop {
            loop {
                match self.cmds.try_recv() {
                    Ok(DeviceCmd::SetImages(images)) => pending.extend(images),
                    Ok(DeviceCmd::SetBootLogo { jpeg, reply }) => {
                        if let Some((_, old)) = logo.replace((jpeg, reply)) {
                            let _ = old.send(Err(crate::i18n::tr("Nahrazeno novějším logem", "Replaced by a newer logo").into()));
                        }
                    }
                    Ok(DeviceCmd::SetBrightness(b)) => {
                        self.opts.brightness = b;
                        queue.push_back(protocol::brightness(b));
                        queue.push_back(protocol::stop());
                    }
                    Ok(DeviceCmd::SetKeyMap(m)) => self.key_map = m,
                    Ok(DeviceCmd::Stop) | Err(TryRecvError::Disconnected) => {
                        let _ = port.write(&self.frame(&protocol::sleep()));
                        return Exit::Stop;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
            // the logo goes out between batches, never in the middle of an image upload
            if queue.is_empty() {
                if let Some((jpeg, reply)) = logo.take() {
                    match self.write_boot_logo(&mut *port, &jpeg) {
                        Ok(()) => {
                            // like a fresh plug-in: wake the panel again, then the engine redraws every key
                            queue.extend(protocol::init_sequence(self.opts.brightness));
                            let _ = reply.send(Ok(()));
                            let _ = self.events.send(DeviceEvent::Redraw);
                        }
                        Err(Some(msg)) => {
                            let _ = reply.send(Err(msg));
                            let _ = self.events.send(DeviceEvent::Redraw);
                        }
                        Err(None) => {
                            let _ = reply.send(Err(crate::i18n::tr("Deck se během nahrávání odpojil", "The deck disconnected during the upload").into()));
                            return Exit::Disconnected;
                        }
                    }
                    last_heartbeat = Instant::now() - self.opts.heartbeat;
                }
            }
            if last_heartbeat.elapsed() >= self.opts.heartbeat {
                queue.push_back(protocol::connect());
                last_heartbeat = Instant::now();
            }
            if queue.is_empty() && !pending.is_empty() {
                for (key, jpeg) in std::mem::take(&mut pending) {
                    queue.extend(protocol::image_upload(key, &jpeg));
                }
                queue.extend(protocol::batch_end());
            }
            let wait_ms = match queue.pop_front() {
                Some(pkt) => {
                    if let Err(e) = port.write(&self.frame(&pkt)) {
                        log::warn!("deck write failed: {e}");
                        return Exit::Disconnected;
                    }
                    self.opts.write_delay.as_millis() as i32
                }
                None => 10,
            };
            match port.read_timeout(&mut buf, wait_ms) {
                Ok(0) => {}
                Ok(n) => {
                    if let Some(Input::Key { device_id, key, pressed }) = protocol::parse_input(&buf[..n], self.key_map.as_ref()) {
                        let _ = self.events.send(DeviceEvent::Raw { device_id, pressed });
                        if self.events.send(DeviceEvent::Key { key, pressed }).is_err() {
                            return Exit::Stop;
                        }
                    }
                }
                Err(e) => {
                    log::warn!("deck read failed: {e}");
                    return Exit::Disconnected;
                }
            }
        }
    }
}

/// Starts the device thread. Drop or send `Stop` to end it.
pub fn spawn_device(backend: impl HidBackend, opts: DeckOptions, events: Sender<DeviceEvent>) -> (Sender<DeviceCmd>, JoinHandle<()>) {
    let (tx, rx) = channel();
    let worker = Worker { backend, opts, events, cmds: rx, key_map: None };
    let handle = std::thread::Builder::new()
        .name("deck-device".into())
        .spawn(move || worker.run())
        .expect("spawn device thread");
    (tx, handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Clone, Default)]
    struct Shared {
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
        reads: Arc<Mutex<VecDeque<Vec<u8>>>>,
        fail: Arc<AtomicBool>,
    }

    struct FakePort(Shared);
    impl HidPort for FakePort {
        fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
            if self.0.fail.load(Ordering::SeqCst) {
                anyhow::bail!("unplugged");
            }
            self.0.writes.lock().unwrap().push(data.to_vec());
            Ok(())
        }
        fn read_timeout(&mut self, buf: &mut [u8], ms: i32) -> anyhow::Result<usize> {
            if self.0.fail.load(Ordering::SeqCst) {
                anyhow::bail!("unplugged");
            }
            let next = self.0.reads.lock().unwrap().pop_front();
            match next {
                Some(r) => {
                    buf[..r.len()].copy_from_slice(&r);
                    Ok(r.len())
                }
                None => {
                    std::thread::sleep(Duration::from_millis(ms.max(0) as u64));
                    Ok(0)
                }
            }
        }
    }

    #[derive(Clone, Default)]
    struct FakeBackend {
        ports: Arc<Mutex<Vec<Shared>>>,
    }
    impl HidBackend for FakeBackend {
        fn find(&mut self) -> Option<Box<dyn HidPort>> {
            let s = Shared::default();
            self.ports.lock().unwrap().push(s.clone());
            Some(Box::new(FakePort(s)))
        }
    }

    fn opts() -> DeckOptions {
        DeckOptions {
            write_delay: Duration::ZERO,
            heartbeat: Duration::from_millis(20),
            reconnect: Duration::from_millis(10),
            ..DeckOptions::default()
        }
    }

    fn wait_for(rx: &Receiver<DeviceEvent>, want: DeviceEvent) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match rx.recv_timeout(left) {
                Ok(ev) if ev == want => return,
                Ok(_) => continue,
                Err(_) => panic!("timed out waiting for {want:?}"),
            }
        }
    }

    fn report(id: u8, st: u8) -> Vec<u8> {
        let mut v = b"ACK\0\0OK\0\0".to_vec();
        v.extend_from_slice(&[id, st, 0xff]);
        v.resize(512, 0);
        v
    }

    fn start(o: DeckOptions) -> (FakeBackend, Sender<DeviceCmd>, Receiver<DeviceEvent>, std::thread::JoinHandle<()>) {
        let backend = FakeBackend::default();
        let (etx, erx) = channel();
        let (ctx, h) = spawn_device(backend.clone(), o, etx);
        wait_for(&erx, DeviceEvent::Connected);
        (backend, ctx, erx, h)
    }

    fn stop(ctx: Sender<DeviceCmd>, h: std::thread::JoinHandle<()>) {
        ctx.send(DeviceCmd::Stop).unwrap();
        h.join().unwrap();
    }

    #[test]
    fn sends_init_sequence_with_report_id() {
        let (b, ctx, _erx, h) = start(DeckOptions { heartbeat: Duration::from_secs(60), ..opts() });
        std::thread::sleep(Duration::from_millis(50));
        let writes = b.ports.lock().unwrap()[0].writes.lock().unwrap().clone();
        assert!(writes.len() >= 7, "{}", writes.len());
        assert!(writes.iter().all(|w| w.len() == 1025 && w[0] == 0));
        assert_eq!(writes[0][1], b'C');
        stop(ctx, h);
    }

    #[test]
    fn emits_raw_and_key_events() {
        let (b, ctx, erx, h) = start(opts());
        b.ports.lock().unwrap()[0].reads.lock().unwrap().push_back(report(2, 1));
        wait_for(&erx, DeviceEvent::Raw { device_id: 2, pressed: true });
        wait_for(&erx, DeviceEvent::Key { key: 2, pressed: true });
        stop(ctx, h);
    }

    #[test]
    fn key_map_applies() {
        let (b, ctx, erx, h) = start(opts());
        let mut map = [0u8; 15];
        for (i, v) in map.iter_mut().enumerate() {
            *v = 15 - i as u8;
        }
        ctx.send(DeviceCmd::SetKeyMap(Some(map))).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        b.ports.lock().unwrap()[0].reads.lock().unwrap().push_back(report(1, 1));
        wait_for(&erx, DeviceEvent::Key { key: 15, pressed: true });
        stop(ctx, h);
    }

    #[test]
    fn drops_on_error_and_reconnects() {
        let (b, ctx, erx, h) = start(opts());
        b.ports.lock().unwrap()[0].fail.store(true, Ordering::SeqCst);
        wait_for(&erx, DeviceEvent::Disconnected);
        wait_for(&erx, DeviceEvent::Connected);
        assert_eq!(b.ports.lock().unwrap().len(), 2);
        stop(ctx, h);
    }

    #[test]
    fn sends_heartbeats() {
        let (b, ctx, _erx, h) = start(opts());
        std::thread::sleep(Duration::from_millis(90));
        let n = b.ports.lock().unwrap()[0].writes.lock().unwrap().iter().filter(|w| &w[6..13] == b"CONNECT").count();
        assert!(n >= 3, "got {n}");
        stop(ctx, h);
    }

    #[test]
    fn image_batches_merge_and_latest_wins() {
        let (b, ctx, _erx, h) = start(DeckOptions { heartbeat: Duration::from_secs(60), ..opts() });
        std::thread::sleep(Duration::from_millis(30));
        let port = b.ports.lock().unwrap()[0].clone();
        port.writes.lock().unwrap().clear();
        {
            // hold the read lock so the device thread cannot run a loop iteration between the two sends
            let _guard = port.reads.lock().unwrap();
            ctx.send(DeviceCmd::SetImages(vec![(1, vec![1; 10])])).unwrap();
            ctx.send(DeviceCmd::SetImages(vec![(1, vec![2; 10]), (2, vec![3; 10])])).unwrap();
        }
        std::thread::sleep(Duration::from_millis(50));
        let w = port.writes.lock().unwrap().clone();
        assert_eq!(w.iter().filter(|p| &p[6..9] == b"BAT").count(), 2);
        assert_eq!(w.iter().filter(|p| &p[6..15] == b"PIXMAPEND").count(), 1);
        assert!(w.iter().any(|p| p[1] == 2 && p[10] == 2));
        assert!(!w.iter().any(|p| p[1] == 1 && p[10] == 1 && p[2] == 1));
        stop(ctx, h);
    }

    #[test]
    fn stop_sends_sleep() {
        let (b, ctx, _erx, h) = start(opts());
        stop(ctx, h);
        let w = b.ports.lock().unwrap()[0].writes.lock().unwrap().clone();
        assert_eq!(&w.last().unwrap()[6..9], b"SLP");
    }

    fn logo_opts() -> DeckOptions {
        DeckOptions { heartbeat: Duration::from_secs(60), logo_erase_wait: Duration::from_millis(40), logo_ack_timeout: Duration::from_millis(300), ..opts() }
    }

    #[test]
    fn boot_logo_follows_the_official_sequence() {
        let (b, ctx, erx, h) = start(logo_opts());
        std::thread::sleep(Duration::from_millis(30));
        let port = b.ports.lock().unwrap()[0].clone();
        port.writes.lock().unwrap().clear();
        let jpeg = include_bytes!("../tests/fixtures/bootlogo.jpg").to_vec();
        let (r, rx) = channel();
        ctx.send(DeviceCmd::SetBootLogo { jpeg, reply: r }).unwrap();
        // the device answers once the data is in
        std::thread::sleep(Duration::from_millis(80));
        port.reads.lock().unwrap().push_back(report(0, 0));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(()));
        wait_for(&erx, DeviceEvent::Redraw);
        let w = port.writes.lock().unwrap().clone();
        let names: Vec<String> = w.iter().map(|p| if &p[1..4] == b"CRT" { String::from_utf8_lossy(&p[6..9]).into_owned() } else { "data".into() }).collect();
        let first = names.iter().position(|n| n == "LOG").unwrap();
        assert_eq!(names[first - 1], "STP");
        assert!(names[first + 1..first + 48].iter().all(|n| n == "data"));
        assert_eq!(names[first + 48], "STP");
        assert_eq!(&names[first + 49..first + 51], ["CLE", "CLE"]);
        // exactly the captured bytes, behind the HID report id
        assert_eq!(&w[first][1..], include_bytes!("../tests/fixtures/bootlogo-cmd.bin").as_slice());
        let data: Vec<u8> = w[first + 1..first + 48].iter().flat_map(|p| p[1..].to_vec()).collect();
        assert_eq!(data.as_slice(), include_bytes!("../tests/fixtures/bootlogo-data.bin").as_slice());
        stop(ctx, h);
    }

    #[test]
    fn boot_logo_without_ack_reports_an_error() {
        let (_b, ctx, _erx, h) = start(logo_opts());
        let (r, rx) = channel();
        ctx.send(DeviceCmd::SetBootLogo { jpeg: include_bytes!("../tests/fixtures/bootlogo.jpg").to_vec(), reply: r }).unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap().is_err());
        stop(ctx, h);
    }

    #[test]
    fn boot_logo_while_disconnected_is_refused() {
        let (etx, _erx) = channel();
        struct NoDeck;
        impl HidBackend for NoDeck {
            fn find(&mut self) -> Option<Box<dyn HidPort>> {
                None
            }
        }
        let (ctx, h) = spawn_device(NoDeck, opts(), etx);
        let (r, rx) = channel();
        ctx.send(DeviceCmd::SetBootLogo { jpeg: vec![0xFF, 0xD8, 0xFF, 0], reply: r }).unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap().is_err());
        stop(ctx, h);
    }
}
