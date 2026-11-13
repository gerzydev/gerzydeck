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
/// frames older than this do not count (the game paused, closed or lost focus)
const WINDOW: Duration = Duration::from_millis(1500);
/// wait before starting PresentMon again after it failed
const RETRY: Duration = Duration::from_secs(30);

/// Where the frame time of a CSV row comes from, decided by the header.
#[derive(Debug, Clone, PartialEq)]
struct Columns {
    app: usize,
    pid: usize,
    frame: FrameTime,
}

#[derive(Debug, Clone, PartialEq)]
enum FrameTime {
    Column(usize),
    /// CPU busy + wait (newer metric sets without MsBetweenPresents)
    Sum(usize, usize),
}

impl Columns {
    fn from_header(header: &str) -> Option<Self> {
        let names: Vec<&str> = header.trim().split(',').map(str::trim).collect();
        let at = |n: &str| names.iter().position(|c| *c == n);
        let frame = at("MsBetweenPresents")
            .or_else(|| at("MsBetweenAppStart"))
            .map(FrameTime::Column)
            .or_else(|| Some(FrameTime::Sum(at("MsCPUBusy")?, at("MsCPUWait")?)))?;
        Some(Self { app: at("Application")?, pid: at("ProcessID")?, frame })
    }

    /// (pid, application, frame time in ms) of one CSV row.
    fn row(&self, line: &str) -> Option<(u32, String, f32)> {
        let cells: Vec<&str> = line.trim().split(',').collect();
        let num = |i: usize| cells.get(i)?.trim().parse::<f32>().ok().filter(|v| v.is_finite());
        let ms = match self.frame {
            FrameTime::Column(i) => num(i)?,
            FrameTime::Sum(a, b) => num(a)? + num(b)?,
        };
        let pid = cells.get(self.pid)?.trim().parse().ok()?;
        (ms > 0.0 && ms < 10_000.0).then(|| (pid, cells.get(self.app).map_or("", |s| s.trim()).to_string(), ms))
    }
}

/// Recent frames per process.
#[derive(Default)]
struct Frames {
    by_pid: HashMap<u32, (String, VecDeque<(Instant, f32)>)>,
}

impl Frames {
    fn push(&mut self, pid: u32, app: String, ms: f32, now: Instant) {
        let (_, frames) = self.by_pid.entry(pid).or_insert_with(|| (app, VecDeque::new()));
        frames.push_back((now, ms));
        while frames.front().is_some_and(|(t, _)| now.duration_since(*t) > WINDOW) {
            frames.pop_front();
        }
        // forget processes that stopped presenting
        if self.by_pid.len() > 64 {
            self.by_pid.retain(|_, (_, f)| f.back().is_some_and(|(t, _)| now.duration_since(*t) <= WINDOW));
        }
    }

    /// Average frame rate of `pid` over the last frames, with its executable name.
    fn fps(&self, pid: u32, now: Instant) -> Option<(f32, String)> {
        let (app, frames) = self.by_pid.get(&pid)?;
        let recent: Vec<f32> = frames.iter().filter(|(t, _)| now.duration_since(*t) <= WINDOW).map(|(_, ms)| *ms).collect();
        if recent.is_empty() {
            return None;
        }
        let mean = recent.iter().sum::<f32>() / recent.len() as f32;
        Some((1000.0 / mean, app.clone()))
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
            "--no_track_display", "--no_track_gpu", "--no_track_input",
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
                if let Some((pid, app, ms)) = columns.row(&line) {
                    frames.lock().unwrap().push(pid, app, ms, Instant::now());
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

    /// Frame rate of the process `pid` (the foreground window's), with its executable.
    pub fn fps(&self, pid: u32) -> Option<(f32, String)> {
        self.child.as_ref()?;
        self.frames.lock().unwrap().fps(pid, Instant::now())
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

    #[test]
    fn header_picks_frame_time_column_or_cpu_sum() {
        let c = Columns::from_header("Application,ProcessID,SwapChainAddress,PresentRuntime,MsBetweenPresents,MsInPresentAPI").unwrap();
        assert_eq!(c, Columns { app: 0, pid: 1, frame: FrameTime::Column(4) });
        assert_eq!(c.row("cs2.exe,19492,0x1F,DXGI,3.472,0.1"), Some((19492, "cs2.exe".into(), 3.472)));
        let v2 = Columns::from_header("Application,ProcessID,CPUStartTime,MsCPUBusy,MsCPUWait").unwrap();
        assert_eq!(v2.row("game.exe,7,12.5,6.0,2.0").map(|r| r.2), Some(8.0));
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
    fn fps_averages_recent_frames_of_one_process() {
        let mut f = Frames::default();
        let t0 = Instant::now();
        for i in 0..10 {
            f.push(1, "cs2.exe".into(), 4.0, t0 + Duration::from_millis(i * 4));
            f.push(2, "chrome.exe".into(), 16.0, t0 + Duration::from_millis(i * 16));
        }
        let now = t0 + Duration::from_millis(200);
        let (fps, app) = f.fps(1, now).unwrap();
        assert_eq!(app, "cs2.exe");
        assert!((fps - 250.0).abs() < 0.01);
        assert!((f.fps(2, now).unwrap().0 - 62.5).abs() < 0.01);
        assert_eq!(f.fps(3, now), None);
        // nothing for longer than the window: no frame rate
        assert_eq!(f.fps(1, t0 + Duration::from_secs(3)), None);
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
