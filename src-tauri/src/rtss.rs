//! Frame rate from RivaTuner Statistics Server (RTSS) shared memory "RTSSSharedMemoryV2".
//! RTSS only has to run in the background; its on-screen display can stay off.

/// Frame rate of one application hooked by RTSS.
#[derive(Debug, Clone, PartialEq)]
pub struct RtssFps {
    pub fps: f32,
    /// executable file name, e.g. "cs2.exe"
    pub app: String,
    pub pid: u32,
}

/// Current frame rate: the entry of `pid_hint` (the foreground process) when RTSS hooks it,
/// otherwise the most recently updated entry. None when RTSS is not running or nothing renders.
pub fn read_fps(_pid_hint: Option<u32>) -> Option<RtssFps> {
    None // TODO(codex)
}
