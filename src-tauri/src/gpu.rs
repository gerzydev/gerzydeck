//! GPU monitor: NVIDIA numbers through NVML (nvml.dll ships with the driver), FPS through RTSS.

use serde::Serialize;

/// GPU metrics, in the order a key press cycles through them.
pub const GPU_METRICS: &[&str] = &["fps", "load", "temp", "vram", "power", "clock", "fan"];
/// CPU / memory metrics, cycled the same way (a press stays inside the key's group).
pub const CPU_METRICS: &[&str] = &["cpu", "cputemp", "ram"];
/// Every metric a monitor key can show (config validation).
pub const METRICS: &[&str] = &["fps", "load", "temp", "vram", "power", "clock", "fan", "cpu", "cputemp", "ram"];

/// One sample of everything the monitor reads. Units: %, °C, MiB, W, MHz.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStats {
    /// e.g. "NVIDIA GeForce RTX 5070 Ti"; empty when NVML is unavailable
    pub name: String,
    pub load: Option<f32>,
    pub temp: Option<f32>,
    pub vram_used: Option<f32>,
    pub vram_total: Option<f32>,
    pub power: Option<f32>,
    pub clock: Option<f32>,
    pub fan: Option<f32>,
    /// None when RTSS is not running or no hooked application renders
    pub fps: Option<f32>,
    /// executable the FPS belongs to
    pub fps_app: Option<String>,
    /// why NVML gave nothing (no NVIDIA driver, …)
    pub error: Option<String>,
    /// total CPU load in %
    pub cpu: Option<f32>,
    /// CPU temperature in °C from MSI Afterburner; None when Afterburner does not run or monitor it
    pub cpu_temp: Option<f32>,
    pub ram_used: Option<f32>,
    pub ram_total: Option<f32>,
}

/// Set by the engine while a GPU key is on the deck; the poller sleeps otherwise.
pub static WANTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const POLL: std::time::Duration = std::time::Duration::from_secs(1);
/// how long to wait before trying to load nvml.dll again after it failed
const NVML_RETRY: std::time::Duration = std::time::Duration::from_secs(30);

/// Reads the GPU once a second while `WANTED` is set and hands every sample to `publish`.
pub fn spawn_poller(publish: impl Fn(GpuStats) + Send + 'static) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("gpu".into())
        .spawn(move || {
            let mut nvml: Option<nvml_wrapper::Nvml> = None;
            let mut nvml_error: Option<String> = None;
            let mut last_try: Option<std::time::Instant> = None;
            loop {
                std::thread::sleep(POLL);
                if !WANTED.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                if nvml.is_none() && last_try.is_none_or(|t| t.elapsed() >= NVML_RETRY) {
                    last_try = Some(std::time::Instant::now());
                    match nvml_wrapper::Nvml::init() {
                        Ok(n) => {
                            nvml = Some(n);
                            nvml_error = None;
                        }
                        Err(e) => {
                            log::warn!("gpu: NVML unavailable: {e}");
                            nvml_error = Some(crate::i18n::tr("Ovladač NVIDIA nenalezen", "NVIDIA driver not found").into());
                        }
                    }
                }
                let mut stats = match &nvml {
                    Some(n) => sample(n),
                    None => GpuStats { error: nvml_error.clone(), ..GpuStats::default() },
                };
                if let Some(f) = crate::rtss::read_fps(foreground_pid()) {
                    stats.fps = Some(f.fps);
                    stats.fps_app = Some(f.app);
                }
                publish(stats);
            }
        })
        .expect("spawn gpu poller")
}

/// One NVML reading of the first GPU; each number is optional (not every card reports a fan, …).
fn sample(nvml: &nvml_wrapper::Nvml) -> GpuStats {
    use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
    let device = match nvml.device_by_index(0) {
        Ok(d) => d,
        Err(e) => {
            log::warn!("gpu: no device: {e}");
            return GpuStats { error: Some(crate::i18n::tr("Grafická karta NVIDIA nenalezena", "No NVIDIA graphics card found").into()), ..GpuStats::default() };
        }
    };
    let mem = device.memory_info().ok();
    const MIB: f32 = 1024.0 * 1024.0;
    GpuStats {
        name: device.name().unwrap_or_default(),
        load: device.utilization_rates().ok().map(|u| u.gpu as f32),
        temp: device.temperature(TemperatureSensor::Gpu).ok().map(|t| t as f32),
        vram_used: mem.as_ref().map(|m| m.used as f32 / MIB),
        vram_total: mem.as_ref().map(|m| m.total as f32 / MIB),
        power: device.power_usage().ok().map(|mw| mw as f32 / 1000.0),
        clock: device.clock_info(Clock::Graphics).ok().map(|c| c as f32),
        fan: device.fan_speed(0).ok().map(|f| f as f32),
        ..GpuStats::default()
    }
}

/// Process id of the foreground window: RTSS may hook several programs, the one in front wins.
fn foreground_pid() -> Option<u32> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let mut pid = 0u32;
    // SAFETY: plain Win32 queries; a null window yields thread id 0 and leaves pid at 0
    unsafe {
        let hwnd = GetForegroundWindow();
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    (pid != 0).then_some(pid)
}

/// Value of one metric in a sample (VRAM: used MiB).
pub fn metric_value(stats: &GpuStats, metric: &str) -> Option<f32> {
    match metric {
        "fps" => stats.fps,
        "load" => stats.load,
        "temp" => stats.temp,
        "vram" => stats.vram_used,
        "power" => stats.power,
        "clock" => stats.clock,
        "fan" => stats.fan,
        _ => None,
    }
}

/// Metric shown after `current` when the key is pressed.
pub fn next_metric(current: &str) -> &'static str {
    let i = METRICS.iter().position(|m| *m == current).map_or(0, |i| (i + 1) % METRICS.len());
    METRICS[i]
}


/// What a GPU key shows; drawn by `render::render_gpu_jpeg`.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuTile {
    /// small text on top: the metric, or the key's own label
    pub caption: String,
    /// big number; None draws a dash
    pub value: Option<String>,
    pub unit: String,
    /// last samples scaled to 0–1, oldest first
    pub history: Vec<f32>,
    /// red accent instead of amber (hot GPU, VRAM almost full)
    pub warn: bool,
    /// short note under the value instead of the graph (NVML missing, …)
    pub note: Option<String>,
}

/// Samples kept for the graph on the key.
pub const HISTORY: usize = 30;

fn caption(metric: &str) -> String {
    use crate::i18n::tr;
    match metric {
        "fps" => "FPS",
        "load" => tr("ZÁTĚŽ GPU", "GPU LOAD"),
        "temp" => tr("TEPLOTA", "TEMP"),
        "vram" => "VRAM",
        "power" => tr("SPOTŘEBA", "POWER"),
        "clock" => tr("TAKT", "CLOCK"),
        "fan" => tr("VENTILÁTOR", "FAN"),
        _ => "GPU",
    }
    .into()
}

/// Builds the tile of `metric` from the newest-last sample history.
pub fn tile(metric: &str, label: &str, history: &std::collections::VecDeque<GpuStats>) -> GpuTile {
    let latest = history.back();
    let caption = if label.trim().is_empty() { caption(metric) } else { label.trim().to_string() };
    let value = latest.and_then(|s| metric_value(s, metric));
    let total = latest.and_then(|s| s.vram_total);
    let decimal = |v: f32| format!("{v:.1}").replace('.', crate::i18n::tr(",", "."));
    let (text, unit) = match (metric, value) {
        (_, None) => (None, ""),
        ("vram", Some(v)) => (Some(decimal(v / 1024.0)), "GB"),
        ("fps", Some(v)) => (Some(format!("{}", v.round() as i64)), "FPS"),
        ("temp", Some(v)) => (Some(format!("{}", v.round() as i64)), "°C"),
        ("power", Some(v)) => (Some(format!("{}", v.round() as i64)), "W"),
        ("clock", Some(v)) => (Some(format!("{}", v.round() as i64)), "MHz"),
        (_, Some(v)) => (Some(format!("{}", v.round() as i64)), "%"),
    };
    let warn = match (metric, value) {
        ("temp", Some(t)) => t >= 83.0,
        ("vram", Some(v)) => total.is_some_and(|t| t > 0.0 && v / t >= 0.9),
        _ => false,
    };
    let series: Vec<f32> = history.iter().map(|s| metric_value(s, metric).unwrap_or(0.0)).collect();
    let peak = series.iter().copied().fold(0.0f32, f32::max);
    let (lo, hi) = match metric {
        "load" | "fan" => (0.0, 100.0),
        "temp" => (20.0, 100.0),
        "vram" => (0.0, total.unwrap_or(peak).max(1.0)),
        "fps" => (0.0, (peak * 1.15).max(30.0)),
        "power" => (0.0, (peak * 1.2).max(50.0)),
        _ => (0.0, (peak * 1.1).max(1.0)),
    };
    let history = series.iter().map(|v| ((v - lo) / (hi - lo)).clamp(0.0, 1.0)).collect();
    let note = latest.and_then(|s| s.error.clone()).filter(|_| metric != "fps" || value.is_none());
    GpuTile { caption, value: text, unit: unit.into(), history, warn, note }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_cycle_and_wrap() {
        assert_eq!(next_metric("fps"), "load");
        assert_eq!(next_metric("fan"), "fps");
        assert_eq!(next_metric("bogus"), "fps");
    }

    #[test]
    fn metric_values_come_from_the_sample() {
        let s = GpuStats { fps: Some(144.0), vram_used: Some(9000.0), temp: Some(61.0), ..GpuStats::default() };
        assert_eq!(metric_value(&s, "fps"), Some(144.0));
        assert_eq!(metric_value(&s, "vram"), Some(9000.0));
        assert_eq!(metric_value(&s, "temp"), Some(61.0));
        assert_eq!(metric_value(&s, "load"), None);
    }

    /// Reads the real card; run with `cargo test nvml_reads -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn nvml_reads_the_card() {
        let n = nvml_wrapper::Nvml::init().expect("nvml");
        let s = sample(&n);
        println!("{s:?}");
        assert!(!s.name.is_empty());
        assert!(s.temp.is_some() && s.load.is_some());
    }
    fn sample_with(f: impl Fn(&mut GpuStats)) -> GpuStats {
        let mut s = GpuStats { name: "RTX".into(), vram_total: Some(16384.0), ..GpuStats::default() };
        f(&mut s);
        s
    }

    #[test]
    fn tile_formats_value_unit_and_graph() {
        let h: std::collections::VecDeque<_> = [60.0, 100.0, 140.0].iter().map(|&v| sample_with(|s| s.fps = Some(v))).collect();
        let t = tile("fps", "", &h);
        assert_eq!((t.caption.as_str(), t.value.as_deref(), t.unit.as_str()), ("FPS", Some("140"), "FPS"));
        assert_eq!(t.history.len(), 3);
        assert!(t.history[2] > t.history[0] && t.history[2] < 1.0);
        assert!(!t.warn && t.note.is_none());
    }

    #[test]
    fn tile_warns_on_heat_and_full_vram() {
        let hot: std::collections::VecDeque<_> = [sample_with(|s| s.temp = Some(85.0))].into();
        assert!(tile("temp", "", &hot).warn);
        let full: std::collections::VecDeque<_> = [sample_with(|s| s.vram_used = Some(15500.0))].into();
        let t = tile("vram", "Paměť", &full);
        assert!(t.warn);
        assert_eq!(t.caption, "Paměť");
        assert_eq!(t.unit, "GB");
    }

    #[test]
    fn tile_without_data_shows_a_dash_and_the_error() {
        let empty = std::collections::VecDeque::new();
        assert_eq!(tile("load", "", &empty).value, None);
        let broken: std::collections::VecDeque<_> = [GpuStats { error: Some("no driver".into()), ..GpuStats::default() }].into();
        let t = tile("temp", "", &broken);
        assert_eq!((t.value, t.note.as_deref()), (None, Some("no driver")));
    }
}
