//! Frame rate through Intel PresentMon (bundled console app, MIT): it reads the Windows ETW events
//! every present() raises, so nothing is injected into the game and anti-cheat has nothing to see.
//! Needs admin rights or membership in "Performance Log Users"; otherwise it exits with
//! "access denied" and `error()` reports "permission".

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SESSION: &str = "GerzysDeckFps";
/// frames older than this do not count for FPS / latency (the game paused, closed or lost focus)
const WINDOW: Duration = Duration::from_millis(1500);
/// frames kept for the 1% low
const LOW_WINDOW: Duration = Duration::from_secs(10);
/// a click latency stays shown this long after the last click
const CLICK_HOLD: Duration = Duration::from_secs(10);
/// wait before starting PresentMon again after it failed
const RETRY: Duration = Duration::from_secs(30);

/// One presented frame of one process.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FrameRow {
    /// time between this present and the previous one, ms
    ms: f32,
    /// frame start → on screen, ms
    latency: Option<f32>,
    /// mouse click → on screen, ms (only on frames a click contributed to)
    click: Option<f32>,
    /// GPU busy time of the frame, ms
    gpu_busy: Option<f32>,
}

/// Everything the deck shows about the frames of one process.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameSample {
    pub app: String,
    pub fps: f32,
    pub frame_time: f32,
    /// average FPS of the slowest 1 % of frames
    pub fps_low: Option<f32>,
    pub latency: Option<f32>,
    pub click_latency: Option<f32>,
    /// share of the frame time the GPU was busy, %
    pub gpu_bound: Option<f32>,
}

/// Column positions in PresentMon's CSV, decided by the header. Names are accepted with or
/// without the "Ms" prefix, the column set differs between PresentMon versions.
#[derive(Debug, Clone, PartialEq)]
struct Columns {
    app: usize,
    pid: usize,
    frame: FrameTime,
    latency: Option<usize>,
    click: Option<usize>,
    gpu_busy: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
enum FrameTime {
    Column(usize),
    /// CPU busy + wait (metric sets without MsBetweenPresents)
    Sum(usize, usize),
}

impl Columns {
    fn from_header(header: &str) -> Option<Self> {
        let names: Vec<&str> = header.trim().split(',').map(str::trim).collect();
        let at = |n: &str| names.iter().position(|c| *c == n || c.strip_prefix("Ms") == Some(n));
        let frame = at("BetweenPresents")
            .or_else(|| at("BetweenAppStart"))
            .map(FrameTime::Column)
            .or_else(|| Some(FrameTime::Sum(at("CPUBusy")?, at("CPUWait")?)))?;
        Some(Self {
            app: at("Application")?,
            pid: at("ProcessID")?,
            frame,
            latency: at("DisplayLatency"),
            click: at("ClickToPhotonLatency"),
            gpu_busy: at("GPUBusy"),
        })
    }

    /// (pid, application, frame) of one CSV row.
    fn row(&self, line: &str) -> Option<(u32, String, FrameRow)> {
        let cells: Vec<&str> = line.trim().split(',').collect();
        let num = |i: usize| cells.get(i)?.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v >= 0.0 && *v < 10_000.0);
        let ms = match self.frame {
            FrameTime::Column(i) => num(i)?,
            FrameTime::Sum(a, b) => num(a)? + num(b)?,
        };
        if ms <= 0.0 {
            return None;
        }
        let pid = cells.get(self.pid)?.trim().parse().ok()?;
        let app = cells.get(self.app).map_or("", |s| s.trim()).to_string();
        let opt = |c: Option<usize>| c.and_then(num);
        Some((pid, app, FrameRow { ms, latency: opt(self.latency), click: opt(self.click).filter(|v| *v > 0.0), gpu_busy: opt(self.gpu_busy) }))
    }
}

/// Recent frames per process.
#[derive(Default)]
struct Frames {
    by_pid: HashMap<u32, Process>,
}

struct Process {
    app: String,
    frames: VecDeque<(Instant, FrameRow)>,
    last_click: Option<(Instant, f32)>,
}

fn mean(v: impl Iterator<Item = f32>) -> Option<f32> {
    let (sum, n) = v.fold((0.0f32, 0usize), |(s, n), x| (s + x, n + 1));
    (n > 0).then(|| sum / n as f32)
}

impl Frames {
    fn push(&mut self, pid: u32, app: String, row: FrameRow, now: Instant) {
        let p = self.by_pid.entry(pid).or_insert_with(|| Process { app, frames: VecDeque::new(), last_click: None });
        p.frames.push_back((now, row));
        if let Some(c) = row.click {
            p.last_click = Some((now, c));
        }
        while p.frames.front().is_some_and(|(t, _)| now.duration_since(*t) > LOW_WINDOW) {
            p.frames.pop_front();
        }
        // forget processes that stopped presenting
        if self.by_pid.len() > 64 {
            self.by_pid.retain(|_, p| p.frames.back().is_some_and(|(t, _)| now.duration_since(*t) <= WINDOW));
        }
    }

    /// Frame numbers of `pid` over the last frames.
    fn sample(&self, pid: u32, now: Instant) -> Option<FrameSample> {
        let p = self.by_pid.get(&pid)?;
        let recent: Vec<FrameRow> = p.frames.iter().filter(|(t, _)| now.duration_since(*t) <= WINDOW).map(|(_, r)| *r).collect();
        let frame_time = mean(recent.iter().map(|r| r.ms))?;
        // 1% low: the slowest 1 % of the frames of the last 10 s (at least 100 frames)
        let mut all: Vec<f32> = p.frames.iter().map(|(_, r)| r.ms).collect();
        let fps_low = (all.len() >= 100).then(|| {
            all.sort_by(|a, b| b.total_cmp(a));
            let worst = &all[..all.len() / 100];
            1000.0 / (worst.iter().sum::<f32>() / worst.len() as f32)
        });
        let busy: Vec<f32> = recent.iter().filter_map(|r| Some(r.gpu_busy? / r.ms)).collect();
        Some(FrameSample {
            app: p.app.clone(),
            fps: 1000.0 / frame_time,
            frame_time,
            fps_low,
            latency: mean(recent.iter().filter_map(|r| r.latency)),
            click_latency: p.last_click.filter(|(t, _)| now.duration_since(*t) <= CLICK_HOLD).map(|(_, c)| c),
            gpu_bound: mean(busy.into_iter()).map(|b| (b * 100.0).clamp(0.0, 100.0)),
        })
    }
}

/// "permission" when PresentMon's error output says the ETW session was refused.
fn classify(stderr: &str) -> &'static str {
    let s = stderr.to_ascii_lowercase();
    if s.contains("access denied") || s.contains("access is denied") || s.contains("performance log users") || s.contains("administrat") {
        "permission"
    } else {
        "unavailable"
    }
}

/// A running PresentMon capture, started on demand and stopped when no key needs it.
pub struct PresentMon {
    exe: Option<PathBuf>,
    child: Option<Child>,
    job: Option<job::Job>,
    frames: Arc<Mutex<Frames>>,
    stderr: Arc<Mutex<String>>,
    error: Option<&'static str>,
    failed_at: Option<Instant>,
}

impl PresentMon {
    pub fn new(exe: Option<PathBuf>) -> Self {
        let exe = exe.filter(|p| p.is_file());
        if exe.is_none() {
            log::warn!("presentmon: PresentMon.exe not found, FPS falls back to RTSS");
        }
        Self { exe, child: None, job: None, frames: Arc::default(), stderr: Arc::default(), error: None, failed_at: None }
    }

    /// Why FPS capture does not run, if it does not.
    pub fn error(&self) -> Option<&'static str> {
        if self.exe.is_none() { Some("unavailable") } else { self.error }
    }

    /// Starts PresentMon unless it runs or failed recently; notices when it exited.
    pub fn ensure(&mut self) {
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(None) => return,
                Ok(Some(status)) => {
                    // give the stderr reader a moment to collect the message
                    std::thread::sleep(Duration::from_millis(50));
                    let text = self.stderr.lock().unwrap().clone();
                    let kind = classify(&text);
                    log::warn!("presentmon exited ({status}): {}", text.trim());
                    self.error = Some(kind);
                    self.failed_at = Some(Instant::now());
                    self.child = None;
                    self.job = None;
                }
                Err(e) => log::warn!("presentmon: {e}"),
            }
        }
        if self.child.is_some() || self.failed_at.is_some_and(|t| t.elapsed() < RETRY) {
            return;
        }
        let Some(exe) = self.exe.clone() else { return };
        match self.spawn(&exe) {
            Ok(()) => log::info!("presentmon started"),
            Err(e) => {
                log::warn!("presentmon: start failed: {e}");
                self.error = Some("unavailable");
                self.failed_at = Some(Instant::now());
            }
        }
    }

    fn spawn(&mut self, exe: &std::path::Path) -> std::io::Result<()> {
        let mut cmd = Command::new(exe);
        cmd.args([
            "--output_stdout", "--no_console_stats", "--stop_existing_session", "--session_name", SESSION,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        no_window(&mut cmd);
        let mut child = cmd.spawn()?;
        // the job kills PresentMon together with the app, even after a crash
        self.job = job::Job::kill_on_close().and_then(|j| j.assign(&child).then_some(j));
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let frames = Arc::clone(&self.frames);
        std::thread::Builder::new().name("presentmon-out".into()).spawn(move || {
            let mut lines = BufReader::new(stdout).lines();
            let Some(columns) = lines.by_ref().map_while(Result::ok).find_map(|l| Columns::from_header(&l)) else {
                return;
            };
            for line in lines.map_while(Result::ok) {
                if let Some((pid, app, row)) = columns.row(&line) {
                    frames.lock().unwrap().push(pid, app, row, Instant::now());
                }
            }
        })?;
        let err_text = Arc::clone(&self.stderr);
        err_text.lock().unwrap().clear();
        std::thread::Builder::new().name("presentmon-err".into()).spawn(move || {
            let mut buf = [0u8; 1024];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 { break; }
                let mut t = err_text.lock().unwrap();
                if t.len() < 8192 {
                    t.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            }
        })?;
        self.child = Some(child);
        self.error = None;
        Ok(())
    }

    /// Frame numbers of the process `pid` (the foreground window's).
    pub fn sample(&self, pid: u32) -> Option<FrameSample> {
        self.child.as_ref()?;
        self.frames.lock().unwrap().sample(pid, Instant::now())
    }

    pub fn running(&self) -> bool {
        self.child.is_some()
    }

    /// Stops the capture and its ETW session.
    pub fn stop(&mut self) {
        let Some(mut child) = self.child.take() else { return };
        let _ = child.kill();
        let _ = child.wait();
        self.job = None;
        self.frames.lock().unwrap().by_pid.clear();
        // a killed PresentMon leaves its real-time ETW session behind
        if let Some(exe) = &self.exe {
            let mut cmd = Command::new(exe);
            cmd.args(["--terminate_existing_session", "--session_name", SESSION]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            no_window(&mut cmd);
            if let Ok(mut c) = cmd.spawn() {
                let _ = c.wait();
            }
        }
        log::info!("presentmon stopped");
    }
}

impl Drop for PresentMon {
    fn drop(&mut self) {
        self.stop();
    }
}

fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let _ = cmd;
}

#[cfg(windows)]
mod job {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// Job object that kills its processes when the last handle closes.
    pub struct Job(isize);

    // SAFETY: a kernel handle owned by this value only; Windows handles may move between threads
    unsafe impl Send for Job {}

    impl Job {
        pub fn kill_on_close() -> Option<Self> {
            // SAFETY: plain Win32 calls on a handle we own
            unsafe {
                let h = CreateJobObjectW(None, windows::core::PCWSTR::null()).ok()?;
                let job = Job(h.0 as isize);
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(h, JobObjectExtendedLimitInformation, &limits as *const _ as _, std::mem::size_of_val(&limits) as u32).ok()?;
                Some(job)
            }
        }

        pub fn assign(&self, child: &std::process::Child) -> bool {
            use std::os::windows::io::AsRawHandle;
            // SAFETY: both handles are valid for the duration of the call
            unsafe { AssignProcessToJobObject(HANDLE(self.0 as *mut _), HANDLE(child.as_raw_handle())).is_ok() }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: closing the handle we own exactly once
            unsafe {
                let _ = CloseHandle(HANDLE(self.0 as *mut _));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(ms: f32) -> FrameRow {
        FrameRow { ms, latency: None, click: None, gpu_busy: None }
    }

    #[test]
    fn header_picks_columns_with_or_without_ms_prefix() {
        let c = Columns::from_header("Application,ProcessID,SwapChainAddress,MsBetweenPresents,DisplayLatency,MsClickToPhotonLatency,MsGPUBusy").unwrap();
        assert_eq!(c, Columns { app: 0, pid: 1, frame: FrameTime::Column(3), latency: Some(4), click: Some(5), gpu_busy: Some(6) });
        assert_eq!(
            c.row("cs2.exe,19492,0x1F,4.0,12.5,NA,3.6"),
            Some((19492, "cs2.exe".into(), FrameRow { ms: 4.0, latency: Some(12.5), click: None, gpu_busy: Some(3.6) }))
        );
        let v2 = Columns::from_header("Application,ProcessID,CPUStartTime,MsCPUBusy,MsCPUWait").unwrap();
        assert_eq!(v2.row("game.exe,7,12.5,6.0,2.0").map(|r| r.2.ms), Some(8.0));
        assert_eq!((v2.latency, v2.click, v2.gpu_busy), (None, None, None));
        assert!(Columns::from_header("Application,Something").is_none());
    }

    #[test]
    fn bad_rows_are_skipped() {
        let c = Columns::from_header("Application,ProcessID,MsBetweenPresents").unwrap();
        for line in ["", "x", "a.exe,notapid,5", "a.exe,1,NA", "a.exe,1,0", "a.exe,1,-3", "a.exe,1,inf"] {
            assert_eq!(c.row(line), None, "{line}");
        }
    }

    #[test]
    fn sample_averages_recent_frames_of_one_process() {
        let mut f = Frames::default();
        let t0 = Instant::now();
        for i in 0..10 {
            f.push(1, "cs2.exe".into(), FrameRow { ms: 4.0, latency: Some(10.0), click: None, gpu_busy: Some(3.8) }, t0 + Duration::from_millis(i * 4));
            f.push(2, "chrome.exe".into(), FrameRow { gpu_busy: Some(4.0), ..row(16.0) }, t0 + Duration::from_millis(i * 16));
        }
        let now = t0 + Duration::from_millis(200);
        let s = f.sample(1, now).unwrap();
        assert_eq!(s.app, "cs2.exe");
        assert!((s.fps - 250.0).abs() < 0.01 && (s.frame_time - 4.0).abs() < 0.01);
        assert_eq!(s.latency, Some(10.0));
        assert!((s.gpu_bound.unwrap() - 95.0).abs() < 0.01, "GPU busy 3.8 of 4 ms");
        assert!((f.sample(2, now).unwrap().gpu_bound.unwrap() - 25.0).abs() < 0.01);
        assert_eq!(s.fps_low, None, "fewer than 100 frames");
        assert_eq!(f.sample(3, now), None);
        assert_eq!(f.sample(1, t0 + Duration::from_secs(3)), None, "nothing recent");
    }

    #[test]
    fn one_percent_low_uses_the_slowest_frames() {
        let mut f = Frames::default();
        let t0 = Instant::now();
        for i in 0..200u64 {
            // two hitches of 50 ms among 4 ms frames
            let ms = if i == 50 || i == 150 { 50.0 } else { 4.0 };
            f.push(1, "g.exe".into(), row(ms), t0 + Duration::from_millis(i * 5));
        }
        let s = f.sample(1, t0 + Duration::from_millis(1000)).unwrap();
        assert!((s.fps_low.unwrap() - 20.0).abs() < 0.01, "{:?}", s.fps_low);
    }

    #[test]
    fn click_latency_is_held_for_a_while() {
        let mut f = Frames::default();
        let t0 = Instant::now();
        f.push(1, "g.exe".into(), FrameRow { click: Some(18.0), ..row(4.0) }, t0);
        f.push(1, "g.exe".into(), row(4.0), t0 + Duration::from_millis(500));
        assert_eq!(f.sample(1, t0 + Duration::from_millis(600)).unwrap().click_latency, Some(18.0));
    }

    #[test]
    fn access_denied_means_permission() {
        assert_eq!(classify("error: failed to start trace session: access denied.\nPresentMon requires either administrative privileges or to be run by a user in the Performance Log Users user group."), "permission");
        assert_eq!(classify("error: something else"), "unavailable");
    }

    #[test]
    fn missing_exe_is_unavailable() {
        let pm = PresentMon::new(Some(PathBuf::from("Z:/nope/PresentMon.exe")));
        assert_eq!(pm.error(), Some("unavailable"));
        assert!(!pm.running());
    }
}
