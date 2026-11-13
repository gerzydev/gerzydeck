//! CPU temperature from MSI Afterburner's shared memory "MAHMSharedMemory" (hardware monitoring).
//! Afterburner has to run with CPU temperature enabled in Settings → Monitoring.

/// Current CPU temperature in °C, None when Afterburner does not run or does not monitor it.
pub fn read_cpu_temp() -> Option<f32> {
    None // TODO(codex)
}
