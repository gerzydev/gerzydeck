//! Soundboard: plays a sound to two outputs at once — the user's own (headphones) and a
//! virtual microphone (SteelSeries Sonar "Microphone" playback device, VB-Cable, Voicemeeter),
//! so people in a game or call hear it too.
//!
//! Sound files live in `%APPDATA%\GerzysDeck\sounds` under a content hash, like key images.
//! Playback runs on its own thread: rodio's output streams are not `Send`.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

pub const MAX_SOUND_BYTES: usize = 50 * 1024 * 1024;
const EXTS: [&str; 4] = ["mp3", "wav", "ogg", "flac"];

/// Playback devices that feed a virtual microphone, in order of preference.
const VIRTUAL_MICS: [&str; 4] = ["Sonar - Microphone", "CABLE Input", "VoiceMeeter Input", "Voicemeeter Input"];

/// "0123456789abcdef.mp3"
pub fn is_sound_id(id: &str) -> bool {
    let Some((hash, ext)) = id.split_once('.') else { return false };
    hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) && EXTS.contains(&ext)
}

pub fn sounds_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sounds")
}

fn sniff(bytes: &[u8]) -> Option<&'static str> {
    let b = |n: usize| bytes.get(..n);
    if b(4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        Some("wav")
    } else if b(4) == Some(b"OggS") {
        Some("ogg")
    } else if b(4) == Some(b"fLaC") {
        Some("flac")
    } else if b(3) == Some(b"ID3") || (bytes.len() > 1 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0) {
        Some("mp3")
    } else {
        None
    }
}

/// Stores a sound file (MP3, WAV, OGG Vorbis, FLAC) and returns its id.
pub fn import(data_dir: &Path, bytes: &[u8]) -> Result<String, String> {
    use crate::i18n::tr;
    if bytes.len() > MAX_SOUND_BYTES {
        return Err(tr("Zvuk je moc velký (max 50 MB)", "The sound is too big (max 50 MB)").into());
    }
    let unsupported = || tr("Tenhle formát neumím přehrát. Použij MP3, WAV, OGG nebo FLAC.", "Can't play this format. Use MP3, WAV, OGG or FLAC.").to_string();
    let ext = sniff(bytes).ok_or_else(unsupported)?;
    rodio::Decoder::new(std::io::Cursor::new(bytes.to_vec())).map_err(|_| unsupported())?;
    let hash = Sha256::digest(bytes);
    let id = format!("{}.{ext}", hash[..8].iter().map(|b| format!("{b:02x}")).collect::<String>());
    let dir = sounds_dir(data_dir);
    let path = dir.join(&id);
    if !path.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    }
    Ok(id)
}

#[derive(Debug, Clone, PartialEq)]
pub enum SoundCmd {
    /// a deck key (or the editor's test button) asks for a sound; volumes 0–100
    Press { sound: String, me: u8, mic: u8, mode: Mode },
    StopAll,
    /// `me`: "" = Windows default output, else a device name.
    /// `mic`: "auto" = first virtual microphone found, "none" = off, else a device name.
    SetDevices { me: String, mic: String },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// second press stops the sound
    Toggle,
    /// every press starts it again from the beginning
    Restart,
}

/// One playing copy of a sound on one output; dropping it stops it.
pub trait Voice {
    fn done(&self) -> bool;
}

pub trait Backend {
    /// Starts `path` on both outputs (a volume of 0 skips that output).
    fn start(&mut self, path: &Path, me: f32, mic: f32) -> Vec<Box<dyn Voice>>;
    fn set_devices(&mut self, me: &str, mic: &str);
}

/// Slider 0–100 → amplitude; squared so the slider feels even to the ear.
fn gain(v: u8) -> f32 {
    let v = v.min(100) as f32 / 100.0;
    v * v
}

pub struct Player<B: Backend> {
    backend: B,
    dir: PathBuf,
    playing: HashMap<String, Vec<Box<dyn Voice>>>,
}

impl<B: Backend> Player<B> {
    pub fn new(backend: B, dir: PathBuf) -> Self {
        Self { backend, dir, playing: HashMap::new() }
    }

    pub fn is_playing(&self, sound: &str) -> bool {
        self.playing.get(sound).is_some_and(|v| v.iter().any(|x| !x.done()))
    }

    pub fn handle(&mut self, cmd: SoundCmd) {
        self.playing.retain(|_, v| v.iter().any(|x| !x.done()));
        match cmd {
            SoundCmd::StopAll => self.playing.clear(),
            SoundCmd::SetDevices { me, mic } => {
                self.playing.clear();
                self.backend.set_devices(&me, &mic);
            }
            SoundCmd::Press { sound, me, mic, mode } => {
                if !is_sound_id(&sound) {
                    return;
                }
                let was_playing = self.playing.remove(&sound).is_some();
                if was_playing && mode == Mode::Toggle {
                    return;
                }
                let path = self.dir.join(&sound);
                if !path.is_file() {
                    log::warn!("sound {sound} is missing");
                    return;
                }
                let voices = self.backend.start(&path, gain(me), gain(mic));
                if !voices.is_empty() {
                    self.playing.insert(sound, voices);
                }
            }
        }
    }
}

// ---------- real audio (rodio / WASAPI) ----------

use rodio::cpal::traits::{DeviceTrait, HostTrait};

pub fn list_outputs() -> Vec<String> {
    rodio::cpal::default_host()
        .output_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

/// The virtual microphone "auto" picks among `names`.
pub fn auto_mic(names: &[String]) -> Option<String> {
    VIRTUAL_MICS.iter().find_map(|v| names.iter().find(|n| n.contains(v)).cloned())
}

struct Out {
    name: String,
    _stream: rodio::OutputStream,
    handle: rodio::OutputStreamHandle,
}

fn open(name: &str) -> Option<Out> {
    let device = rodio::cpal::default_host().output_devices().ok()?.find(|d| d.name().is_ok_and(|n| n == name))?;
    match rodio::OutputStream::try_from_device(&device) {
        Ok((stream, handle)) => Some(Out { name: name.into(), _stream: stream, handle }),
        Err(e) => {
            log::warn!("cannot open audio output {name}: {e}");
            None
        }
    }
}

struct SinkVoice(rodio::Sink);
impl Voice for SinkVoice {
    fn done(&self) -> bool {
        self.0.empty()
    }
}

#[derive(Default)]
pub struct Rodio {
    me_choice: String,
    mic_choice: String,
    me: Option<Out>,
    mic: Option<Out>,
}

impl Rodio {
    fn wanted(&self) -> (Option<String>, Option<String>) {
        let host = rodio::cpal::default_host();
        let me = if self.me_choice.is_empty() { host.default_output_device().and_then(|d| d.name().ok()) } else { Some(self.me_choice.clone()) };
        let mic = match self.mic_choice.as_str() {
            "none" => None,
            "auto" | "" => auto_mic(&list_outputs()),
            name => Some(name.to_string()),
        };
        // never play the same sound twice into one device
        let mic = mic.filter(|m| Some(m) != me.as_ref());
        (me, mic)
    }

    fn play_on(out: &mut Option<Out>, want: Option<String>, path: &Path, volume: f32) -> Option<Box<dyn Voice>> {
        let want = want?;
        if volume <= 0.0 {
            return None;
        }
        if out.as_ref().is_none_or(|o| o.name != want) {
            *out = open(&want);
        }
        let file = std::fs::File::open(path).ok()?;
        let source = rodio::Decoder::new(std::io::BufReader::new(file)).map_err(|e| log::warn!("cannot decode {}: {e}", path.display())).ok()?;
        let sink = match rodio::Sink::try_new(&out.as_ref()?.handle) {
            Ok(s) => s,
            Err(_) => {
                // device went away (unplugged, Sonar restarted): reopen once
                *out = open(&want);
                rodio::Sink::try_new(&out.as_ref()?.handle).ok()?
            }
        };
        sink.set_volume(volume);
        sink.append(source);
        Some(Box::new(SinkVoice(sink)))
    }
}

impl Backend for Rodio {
    fn start(&mut self, path: &Path, me: f32, mic: f32) -> Vec<Box<dyn Voice>> {
        let (want_me, want_mic) = self.wanted();
        [Self::play_on(&mut self.me, want_me, path, me), Self::play_on(&mut self.mic, want_mic, path, mic)].into_iter().flatten().collect()
    }

    fn set_devices(&mut self, me: &str, mic: &str) {
        if self.me_choice != me || self.mic_choice != mic {
            self.me_choice = me.into();
            self.mic_choice = mic.into();
            self.me = None;
            self.mic = None;
        }
    }
}

/// Starts the playback thread.
pub fn spawn_player(data_dir: &Path) -> Sender<SoundCmd> {
    let (tx, rx): (Sender<SoundCmd>, Receiver<SoundCmd>) = channel();
    let dir = sounds_dir(data_dir);
    let _ = std::thread::Builder::new().name("sound".into()).spawn(move || {
        let mut player = Player::new(Rodio { mic_choice: "auto".into(), ..Rodio::default() }, dir);
        while let Ok(cmd) = rx.recv() {
            player.handle(cmd);
        }
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct Log {
        started: Vec<(String, f32, f32)>,
        live: Vec<Rc<RefCell<bool>>>,
        devices: Vec<(String, String)>,
    }

    struct FakeVoice(Rc<RefCell<bool>>);
    impl Voice for FakeVoice {
        fn done(&self) -> bool {
            !*self.0.borrow()
        }
    }
    impl Drop for FakeVoice {
        fn drop(&mut self) {
            *self.0.borrow_mut() = false;
        }
    }

    struct Fake(Rc<RefCell<Log>>);
    impl Backend for Fake {
        fn start(&mut self, path: &Path, me: f32, mic: f32) -> Vec<Box<dyn Voice>> {
            let mut log = self.0.borrow_mut();
            log.started.push((path.file_name().unwrap().to_string_lossy().into_owned(), me, mic));
            let alive = Rc::new(RefCell::new(true));
            log.live.push(alive.clone());
            vec![Box::new(FakeVoice(alive))]
        }
        fn set_devices(&mut self, me: &str, mic: &str) {
            self.0.borrow_mut().devices.push((me.into(), mic.into()));
        }
    }

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("gd-sound-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const A: &str = "0123456789abcdef.wav";

    fn player() -> (Player<Fake>, Rc<RefCell<Log>>) {
        let d = dir();
        std::fs::write(d.join(A), b"x").unwrap();
        let log = Rc::new(RefCell::new(Log::default()));
        (Player::new(Fake(log.clone()), d), log)
    }

    fn press(mode: Mode) -> SoundCmd {
        SoundCmd::Press { sound: A.into(), me: 100, mic: 50, mode }
    }

    #[test]
    fn toggle_starts_then_stops() {
        let (mut p, log) = player();
        p.handle(press(Mode::Toggle));
        assert!(p.is_playing(A));
        assert_eq!(log.borrow().started, vec![(A.to_string(), 1.0, 0.25)]);
        p.handle(press(Mode::Toggle));
        assert!(!p.is_playing(A));
        assert!(!*log.borrow().live[0].borrow(), "the voice was stopped");
        assert_eq!(log.borrow().started.len(), 1);
    }

    #[test]
    fn toggle_after_the_sound_ended_plays_again() {
        let (mut p, log) = player();
        p.handle(press(Mode::Toggle));
        *log.borrow().live[0].borrow_mut() = false; // finished on its own
        p.handle(press(Mode::Toggle));
        assert_eq!(log.borrow().started.len(), 2);
        assert!(p.is_playing(A));
    }

    #[test]
    fn restart_stops_the_old_copy_and_plays_again() {
        let (mut p, log) = player();
        p.handle(press(Mode::Restart));
        p.handle(press(Mode::Restart));
        let l = log.borrow();
        assert_eq!(l.started.len(), 2);
        assert!(!*l.live[0].borrow());
        assert!(*l.live[1].borrow());
    }

    #[test]
    fn stop_all_and_device_change_stop_everything() {
        let (mut p, log) = player();
        p.handle(press(Mode::Toggle));
        p.handle(SoundCmd::StopAll);
        assert!(!p.is_playing(A));
        p.handle(press(Mode::Toggle));
        p.handle(SoundCmd::SetDevices { me: String::new(), mic: "none".into() });
        assert!(!p.is_playing(A));
        assert_eq!(log.borrow().devices, vec![(String::new(), "none".to_string())]);
    }

    #[test]
    fn missing_or_bad_ids_are_ignored() {
        let (mut p, log) = player();
        p.handle(SoundCmd::Press { sound: "../../config.json".into(), me: 100, mic: 100, mode: Mode::Toggle });
        p.handle(SoundCmd::Press { sound: "fedcba9876543210.mp3".into(), me: 100, mic: 100, mode: Mode::Toggle });
        assert!(log.borrow().started.is_empty());
    }

    #[test]
    fn ids_and_sniffing() {
        assert!(is_sound_id(A));
        assert!(is_sound_id("0123456789abcdef.ogg"));
        assert!(!is_sound_id("0123456789abcdef.exe"));
        assert!(!is_sound_id("0123456789ABCDEF.mp3"));
        assert!(!is_sound_id("../x.mp3"));
        assert_eq!(sniff(b"ID3\x04rest"), Some("mp3"));
        assert_eq!(sniff(&[0xFF, 0xFB, 0x90, 0]), Some("mp3"));
        assert_eq!(sniff(b"OggS...."), Some("ogg"));
        assert_eq!(sniff(b"fLaC...."), Some("flac"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), Some("wav"));
        assert_eq!(sniff(b"<html>"), None);
    }

    fn wav(samples: usize) -> Vec<u8> {
        let data = samples * 2;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data as u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&1u16.to_le_bytes()); // mono
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&16000u32.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data as u32).to_le_bytes());
        v.resize(v.len() + data, 0);
        v
    }

    #[test]
    fn import_stores_by_hash_and_rejects_junk() {
        let d = dir();
        let id = import(&d, &wav(800)).unwrap();
        assert!(is_sound_id(&id) && id.ends_with(".wav"), "{id}");
        assert!(sounds_dir(&d).join(&id).is_file());
        assert_eq!(import(&d, &wav(800)).unwrap(), id, "same file, same id");
        assert!(import(&d, b"RIFF\0\0\0\0WAVEjunk").is_err());
        assert!(import(&d, b"hello").is_err());
    }

    #[test]
    fn auto_mic_prefers_sonar() {
        let names: Vec<String> = ["Sluchátka (ASUS XONAR U5)", "CABLE Input (VB-Audio Virtual Cable)", "SteelSeries Sonar - Microphone (SteelSeries Sonar Virtual Audio Device)"].map(String::from).into();
        assert!(auto_mic(&names).unwrap().contains("Sonar - Microphone"));
        assert!(auto_mic(&names[..2]).unwrap().contains("CABLE"));
        assert_eq!(auto_mic(&names[..1]), None);
    }
}
