//! Sound decoding that skips damaged frames, like Windows' own players do.
//!
//! rodio's decoder gives up after three frames in a row that do not decode. MP3s cut out of a
//! longer stream often start with such frames (their bit reservoir points into audio that is no
//! longer there), so the sound would not play at all. This decoder drops every packet that does
//! not decode and plays the rest. OGG Vorbis still goes through rodio (symphonia here is built
//! without Vorbis).

use std::time::Duration;
use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub type BoxedSource = Box<dyn rodio::Source<Item = i16> + Send>;

/// A playable source for a sound file; `ext` is the stored extension (mp3, wav, flac, ogg).
pub fn open<R: MediaSource + 'static>(reader: R, ext: &str) -> Result<BoxedSource, String> {
    if ext == "ogg" {
        return rodio::Decoder::new(std::io::BufReader::new(reader)).map(|d| Box::new(d) as BoxedSource).map_err(|e| e.to_string());
    }
    Tolerant::new(Box::new(reader), ext).map(|t| Box::new(t) as BoxedSource)
}

struct Tolerant {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    spec: SignalSpec,
    /// interleaved samples of the current packet
    buf: Vec<i16>,
    pos: usize,
    total: Option<Duration>,
    /// packets that did not decode (logged once at the end)
    skipped: usize,
}

impl Tolerant {
    fn new(source: Box<dyn MediaSource>, ext: &str) -> Result<Self, String> {
        let mut hint = Hint::new();
        hint.with_extension(ext);
        let mss = MediaSourceStream::new(source, Default::default());
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &FormatOptions { enable_gapless: true, ..Default::default() }, &MetadataOptions::default())
            .map_err(|e| e.to_string())?;
        let track = probed.format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or("no audio track")?;
        let total = track.codec_params.time_base.zip(track.codec_params.n_frames).map(|(base, n)| {
            let t = base.calc_time(n);
            Duration::from_secs(t.seconds) + Duration::from_secs_f64(t.frac)
        });
        let decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).map_err(|e| e.to_string())?;
        let track = track.id;
        let mut me = Tolerant { format: probed.format, decoder, track, spec: SignalSpec::new(44_100, Default::default()), buf: Vec::new(), pos: 0, total, skipped: 0 };
        if !me.refill() {
            return Err(format!("no packet decodes ({} damaged)", me.skipped));
        }
        Ok(me)
    }

    /// Decodes the next packet that decodes; false at the end of the stream.
    fn refill(&mut self) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(_) => return false,
            };
            if packet.track_id() != self.track {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) if decoded.frames() > 0 => {
                    self.spec = *decoded.spec();
                    let mut samples = SampleBuffer::<i16>::new(decoded.capacity() as u64, self.spec);
                    samples.copy_interleaved_ref(decoded);
                    self.buf.clear();
                    self.buf.extend_from_slice(samples.samples());
                    self.pos = 0;
                    return true;
                }
                Ok(_) => {}
                Err(Error::ResetRequired) => self.decoder.reset(),
                // a damaged packet: skip it, the stream goes on with the next one
                Err(_) => self.skipped += 1,
            }
        }
    }
}

impl Iterator for Tolerant {
    type Item = i16;

    fn next(&mut self) -> Option<i16> {
        let sample = *self.buf.get(self.pos)?;
        self.pos += 1;
        if self.pos >= self.buf.len() && !self.refill() {
            self.buf.clear();
            self.pos = 0;
            if self.skipped > 0 {
                log::info!("sound played with {} damaged packets skipped", self.skipped);
            }
        }
        Some(sample)
    }
}

impl rodio::Source for Tolerant {
    fn current_frame_len(&self) -> Option<usize> {
        // samples left in this packet; the format may change at the next one
        Some(self.buf.len() - self.pos)
    }
    fn channels(&self) -> u16 {
        self.spec.channels.count() as u16
    }
    fn sample_rate(&self) -> u32 {
        self.spec.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// MPEG-1 Layer III, 44.1 kHz, 128 kbit/s, mono: 417-byte frames. A frame with all-zero side
    /// info decodes to silence; `damaged` claims 4095 bits of audio data that are not there.
    fn frame(damaged: bool) -> Vec<u8> {
        let mut f = vec![0u8; 417];
        f[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0xC0]);
        if damaged {
            // side info: main_data_begin (9) + private (5) + scfsi (4), then part2_3_length (12)
            f[6] = 0x3F;
            f[7] = 0xFC;
        }
        f
    }

    fn mp3(damaged: usize, good: usize) -> Vec<u8> {
        (0..damaged).map(|_| frame(true)).chain((0..good).map(|_| frame(false))).flatten().collect()
    }

    #[test]
    fn plays_an_mp3_whose_first_frames_are_damaged() {
        let bytes = mp3(8, 30);
        assert!(rodio::Decoder::new(Cursor::new(bytes.clone())).is_err(), "rodio alone gives up on this file");
        let source = open(Cursor::new(bytes), "mp3").unwrap();
        assert_eq!((source.channels(), source.sample_rate()), (1, 44_100));
        let n = source.count();
        assert!(n >= 20 * 1152, "the good frames play: {n} samples");
    }

    #[test]
    fn damaged_frames_in_the_middle_do_not_end_the_sound() {
        let bytes: Vec<u8> = [mp3(0, 10), mp3(6, 10)].concat();
        let n = open(Cursor::new(bytes), "mp3").unwrap().count();
        assert!(n >= 18 * 1152, "{n} samples");
    }

    #[test]
    fn junk_is_rejected() {
        assert!(open(Cursor::new(b"hello, not audio".to_vec()), "mp3").is_err());
        assert!(open(Cursor::new(mp3(12, 0)), "mp3").is_err());
    }
}

#[cfg(test)]
mod live {
    /// `cargo test --lib sound_decode::live -- --ignored --nocapture`: decodes every stored sound,
    /// plus the files given in GD_SOUND_FILES (separated by `;`).
    #[test]
    #[ignore]
    fn decode_user_sounds() {
        let dir = crate::sound::sounds_dir(&std::path::PathBuf::from(std::env::var("APPDATA").unwrap()).join("GerzysDeck"));
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        files.extend(std::env::var("GD_SOUND_FILES").unwrap_or_default().split(';').filter(|s| !s.is_empty()).map(Into::into));
        let mut failed = 0;
        for f in files {
            let ext = f.extension().and_then(|e| e.to_str()).unwrap_or_default().to_string();
            let rodio_ok = rodio::Decoder::new(std::io::BufReader::new(std::fs::File::open(&f).unwrap())).is_ok();
            match super::open(std::fs::File::open(&f).unwrap(), &ext) {
                Ok(s) => println!("{:>9} samples  rodio={rodio_ok}  {}", s.count(), f.display()),
                Err(e) => { failed += 1; println!("FAILED {e}  {}", f.display()) }
            }
        }
        assert_eq!(failed, 0);
    }
}
