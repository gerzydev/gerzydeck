//! GPU monitor: NVIDIA numbers through NVML (nvml.dll ships with the driver), FPS through RTSS.

use serde::Serialize;

/// Metrics a GPU key can show, in the order a key press cycles through them.
pub const METRICS: &[&str] = &["fps", "load", "temp", "vram", "power", "clock", "fan"];

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
}
