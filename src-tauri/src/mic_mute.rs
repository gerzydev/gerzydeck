//! Windows capture endpoint muting, with a durable recovery marker.
//! Callers must serialize mute/unmute pairs and only undo a successful `Ok(true)` mute.
//! Devices are matched by their Core Audio friendly name, as used by CPAL.

use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use windows::Win32::{
    Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
    Foundation::RPC_E_CHANGED_MODE,
    Media::Audio::{
        eCapture, Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator,
        DEVICE_STATE_ACTIVE,
    },
    System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
        STGM_READ,
    },
};

const MARKER: &str = "mic-muted.json";
#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Marker {
    device: String,
}

fn write_marker(dir: &Path, name: &str) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(&Marker {
        device: name.into(),
    })
    .map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut file = fs::File::create(dir.join(MARKER)).map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())
}
fn read_marker(dir: &Path) -> Result<Option<Marker>, String> {
    match fs::read(dir.join(MARKER)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
fn remove_marker(dir: &Path) -> Result<(), String> {
    match fs::remove_file(dir.join(MARKER)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
// Balance only successful initializations, including S_FALSE. An existing STA is usable.
struct Com(bool);
impl Com {
    fn init() -> Result<Self, String> {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr == RPC_E_CHANGED_MODE {
            return Ok(Self(false));
        }
        hr.ok().map_err(|e| e.to_string())?;
        Ok(Self(true))
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn inputs() -> windows::core::Result<Vec<(String, windows::Win32::Media::Audio::IMMDevice)>> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let collection = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
        let mut result = Vec::new();
        for i in 0..collection.GetCount()? {
            let device = collection.Item(i)?;
            let properties = device.OpenPropertyStore(STGM_READ)?;
            let value = properties.GetValue(&PKEY_Device_FriendlyName)?;
            let name = windows::core::BSTR::try_from(&value)?.to_string();
            result.push((name, device));
        }
        Ok(result)
    }
}
fn endpoint(name: &str) -> Result<IAudioEndpointVolume, String> {
    let device = inputs()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, d)| d)
        .ok_or_else(|| format!("Capture device not found: {name}"))?;
    unsafe { device.Activate(CLSCTX_ALL, None).map_err(|e| e.to_string()) }
}
/// Capture sides of the virtual microphones the soundboard plays into: muting one of them would
/// silence the soundboard itself, so they are not offered.
const VIRTUAL_CAPTURE: [&str; 4] = ["Sonar - Microphone", "CABLE Output", "VoiceMeeter Out", "Voicemeeter Out"];

fn physical(name: &str) -> bool {
    !VIRTUAL_CAPTURE.iter().any(|v| name.contains(v))
}

/// Physical microphones (virtual soundboard microphones left out).
pub fn list_inputs() -> Vec<String> {
    let result = (|| {
        let _com = Com::init()?;
        inputs()
            .map(|v| v.into_iter().map(|(n, _)| n).filter(|n| physical(n)).collect())
            .map_err(|e| e.to_string())
    })();
    result.unwrap_or_else(|e| {
        log::warn!("Cannot list microphones: {e}");
        Vec::new()
    })
}
pub fn mute(data_dir: &Path, name: &str) -> Result<bool, String> {
    let _com = Com::init()?;
    let volume = endpoint(name)?;
    if unsafe { volume.GetMute() }
        .map_err(|e| e.to_string())?
        .as_bool()
    {
        return Ok(false);
    }
    // Persist recovery information before touching hardware.
    write_marker(data_dir, name)?;
    if let Err(e) = unsafe { volume.SetMute(true, std::ptr::null()) } {
        if let Err(cleanup) = remove_marker(data_dir) {
            log::warn!("Cannot remove failed microphone mute marker: {cleanup}");
        }
        return Err(e.to_string());
    }
    Ok(true)
}
pub fn unmute(data_dir: &Path, name: &str) -> Result<(), String> {
    let _com = Com::init()?;
    let volume = endpoint(name)?;
    unsafe { volume.SetMute(false, std::ptr::null()) }.map_err(|e| e.to_string())?;
    remove_marker(data_dir)
}
pub fn restore_after_crash(data_dir: &Path) {
    match read_marker(data_dir) {
        Ok(Some(marker)) => {
            // on success `unmute` removes the marker; on failure (microphone unplugged right now)
            // it stays, so the next start tries again
            if let Err(e) = unmute(data_dir, &marker.device) {
                log::warn!("Cannot restore microphone {}: {e}", marker.device);
            }
            return;
        }
        Ok(None) => return,
        Err(e) => log::warn!("Cannot read microphone recovery marker: {e}"),
    }
    if let Err(e) = remove_marker(data_dir) {
        log::warn!("Cannot remove microphone recovery marker: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marker_lifecycle_and_corrupt_json() {
        let dir = std::env::temp_dir().join(format!(
            "gerzy-mic-marker-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&dir).unwrap();
        remove_marker(&dir).unwrap();
        assert_eq!(read_marker(&dir).unwrap(), None);
        let name = "Microphone \"USB\" – test";
        write_marker(&dir, name).unwrap();
        assert_eq!(
            read_marker(&dir).unwrap(),
            Some(Marker {
                device: name.into()
            })
        );
        remove_marker(&dir).unwrap();
        assert_eq!(read_marker(&dir).unwrap(), None);
        fs::write(dir.join(MARKER), b"{broken").unwrap();
        assert!(read_marker(&dir).is_err());
        // Invalid recovery data must be discarded without any COM/device operation.
        restore_after_crash(&dir);
        assert_eq!(read_marker(&dir).unwrap(), None);
        remove_marker(&dir).unwrap();
        fs::remove_dir(&dir).unwrap();
    }
    #[test]
    fn virtual_microphones_are_not_offered() {
        assert!(!physical("SteelSeries Sonar - Microphone (SteelSeries Sonar Virtual Audio Device)"));
        assert!(!physical("CABLE Output (VB-Audio Virtual Cable)"));
        assert!(!physical("VoiceMeeter Output (VB-Audio VoiceMeeter VAIO)"));
        assert!(physical("Mikrofon (USB PnP Audio Device)"));
    }

    #[test]
    #[ignore = "Lists live capture devices; never changes mute state"]
    fn list_live_inputs() {
        println!("{:?}", list_inputs());
    }
}
