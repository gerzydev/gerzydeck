//! Nonblocking plugin supervision. Each child owns a Windows kill-on-close job.
use super::{
    install::{valid_id, InstalledPlugin},
    manifest::Runtime,
};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum PluginStatus {
    Running,
    Starting,
    Crashed { restarts: u32 },
    MissingNode,
    Disabled,
}

#[derive(Debug, Clone)]
pub struct NodeResolver {
    path: Option<PathBuf>,
}
impl Default for NodeResolver {
    fn default() -> Self {
        Self { path: Self::find() }
    }
}
impl NodeResolver {
    pub fn with_path(path: Option<PathBuf>) -> Self {
        Self { path }
    }
    pub fn find() -> Option<PathBuf> {
        let mut command = Command::new("where.exe");
        command.arg("node");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        if let Ok(output) = command.output() {
            if output.status.success() {
                if let Some(path) = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(|s| PathBuf::from(s.trim()))
                    .find(|p| {
                        p.is_absolute()
                            && p.is_file()
                            && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
                    })
                {
                    return Some(path);
                }
            }
        }
        std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .map(|p| p.join("nodejs/node.exe"))
            .filter(|p| p.is_file())
    }
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

/// A process and its job: dropping this value kills the entire process tree.
pub struct Child {
    inner: std::process::Child,
    #[cfg(windows)]
    job: Option<win::Handle>,
}
impl Child {
    pub fn id(&self) -> u32 {
        self.inner.id()
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.inner.try_wait()
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.inner.wait()
    }
    pub fn kill(&mut self) -> io::Result<()> {
        #[cfg(windows)]
        {
            self.job.take();
        }
        match self.inner.try_wait()? {
            Some(_) => Ok(()),
            None => self.inner.kill(),
        }
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        if let Err(error) = self.kill() {
            log::warn!("Nelze ukončit plugin {}: {error}", self.id());
        }
        if let Err(error) = self.inner.wait() {
            log::warn!("Nelze převzít výsledek pluginu {}: {error}", self.id());
        }
    }
}

pub fn spawn(
    plugin: &InstalledPlugin,
    runtime: Runtime,
    port: u16,
    uuid: &str,
    info: &str,
    log_dir: &Path,
    node: &NodeResolver,
) -> io::Result<Child> {
    if !valid_id(&plugin.id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Neplatné ID pluginu",
        ));
    }
    let root = plugin.dir.canonicalize()?;
    let runtime_path = match &runtime {
        Runtime::Exe(p) | Runtime::Html(p) => p,
        Runtime::Node { script, .. } => script,
    }
    .canonicalize()?;
    if !runtime_path.starts_with(&root) || !runtime_path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Program je mimo složku pluginu",
        ));
    }
    let mut command = match runtime {
        Runtime::Exe(_) => Command::new(&runtime_path),
        Runtime::Node { .. } => {
            let node = node
                .path()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Chybí Node.js"))?;
            let mut command = Command::new(node);
            command.arg(&runtime_path);
            command
        }
        Runtime::Html(_) => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "HTML plugin vyžaduje hostitelský WebView",
            ))
        }
    };
    fs::create_dir_all(log_dir)?;
    let logs = log_dir.canonicalize()?;
    let log_path = logs.join(format!("{}.log", plugin.id));
    match fs::symlink_metadata(&log_path) {
        Ok(_) if !log_path.canonicalize()?.starts_with(&logs) => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Log je mimo složku logů",
            ))
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let stderr = stdout.try_clone()?;
    command
        .current_dir(root)
        .args([
            "-port",
            &port.to_string(),
            "-pluginUUID",
            uuid,
            "-registerEvent",
            "registerPlugin",
            "-info",
            info,
        ])
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    #[cfg(windows)]
    {
        use std::os::windows::{io::AsRawHandle, process::CommandExt};
        // No plugin code runs before successful job assignment, so descendants
        // cannot escape the job in the spawn/assign interval.
        command.creation_flags(0x08000000 | 0x00000004); // CREATE_NO_WINDOW | CREATE_SUSPENDED
        let job = win::job()?;
        let mut inner = command.spawn()?;
        let assign = unsafe {
            windows::Win32::System::JobObjects::AssignProcessToJobObject(
                job.raw(),
                windows::Win32::Foundation::HANDLE(inner.as_raw_handle()),
            )
        }
        .map_err(win::error);
        if let Err(error) = assign.and_then(|_| win::resume(inner.id())) {
            let _ = inner.kill();
            let _ = inner.wait();
            return Err(error);
        }
        Ok(Child {
            inner,
            job: Some(job),
        })
    }
    #[cfg(not(windows))]
    {
        command.spawn().map(|inner| Child { inner })
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{CloseHandle, HANDLE},
            System::{
                Diagnostics::ToolHelp::{
                    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                    THREADENTRY32,
                },
                JobObjects::{
                    CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
                    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                },
                Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
            },
        },
    };
    // Store the unique owned handle as an integer so workers can own it. Windows
    // kernel handles support transfer across threads; no alias is exposed publicly.
    pub(super) struct Handle(isize);
    impl Handle {
        fn new(handle: HANDLE) -> Self {
            Self(handle.0 as isize)
        }
        pub(super) fn raw(&self) -> HANDLE {
            HANDLE(self.0 as *mut _)
        }
    }
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.raw());
            }
        }
    }
    pub(super) fn error(error: windows::core::Error) -> io::Error {
        io::Error::other(error.to_string())
    }
    pub(super) fn job() -> io::Result<Handle> {
        unsafe {
            let job = Handle::new(CreateJobObjectW(None, PCWSTR::null()).map_err(error)?);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                std::mem::size_of_val(&limits) as u32,
            )
            .map_err(error)?;
            Ok(job)
        }
    }
    pub(super) fn resume(pid: u32) -> io::Result<()> {
        unsafe {
            let snapshot =
                Handle::new(CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).map_err(error)?);
            let mut entry = THREADENTRY32 {
                dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            Thread32First(snapshot.raw(), &mut entry).map_err(error)?;
            loop {
                if entry.th32OwnerProcessID == pid {
                    let thread = Handle::new(
                        OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)
                            .map_err(error)?,
                    );
                    if ResumeThread(thread.raw()) == u32::MAX {
                        return Err(io::Error::last_os_error());
                    }
                    return Ok(());
                }
                if Thread32Next(snapshot.raw(), &mut entry).is_err() {
                    break;
                }
            }
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Vlákno pluginu nebylo nalezeno",
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub struct SupervisorOptions {
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub stable_after: Duration,
    pub registration_timeout: Duration,
    pub poll_interval: Duration,
}
impl Default for SupervisorOptions {
    fn default() -> Self {
        Self {
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            stable_after: Duration::from_secs(60),
            registration_timeout: Duration::from_secs(5),
            poll_interval: Duration::from_millis(20),
        }
    }
}
struct Backoff {
    next: Duration,
    initial: Duration,
    max: Duration,
    stable: Duration,
}
impl Backoff {
    fn new(initial: Duration, max: Duration, stable: Duration) -> Self {
        let initial = initial.max(Duration::from_millis(1));
        Self {
            next: initial,
            initial,
            max: max.max(initial),
            stable,
        }
    }
    fn after_exit(&mut self, uptime: Duration) -> Duration {
        if uptime >= self.stable {
            self.next = self.initial;
        }
        let delay = self.next;
        self.next = self.next.saturating_mul(2).min(self.max);
        delay
    }
}
enum Control {
    Stop,
    Registered,
}
struct Worker {
    control: mpsc::Sender<Control>,
    status: Arc<Mutex<PluginStatus>>,
    thread: JoinHandle<()>,
}
/// A supervisor can be owned by a synchronous host; no tokio runtime is required.
/// Wire `mark_registered` from the WS server. Automatic restarts retain the
/// original launch arguments and host token until stop/reload/stop_all.
pub struct Supervisor {
    log_dir: PathBuf,
    node: NodeResolver,
    options: SupervisorOptions,
    workers: HashMap<String, Worker>,
    retired: Vec<(String, JoinHandle<()>)>,
}
impl Default for Supervisor {
    fn default() -> Self {
        Self::new(
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("GerzysDeck/logs/plugins"),
        )
    }
}
impl Supervisor {
    pub fn new(log_dir: impl Into<PathBuf>) -> Self {
        Self::with_options(
            log_dir,
            NodeResolver::default(),
            SupervisorOptions::default(),
        )
    }
    pub fn with_options(
        log_dir: impl Into<PathBuf>,
        node: NodeResolver,
        options: SupervisorOptions,
    ) -> Self {
        Self {
            log_dir: log_dir.into(),
            node,
            options,
            workers: HashMap::new(),
            retired: Vec::new(),
        }
    }
    pub fn start(&mut self, plugin: &InstalledPlugin, port: u16, uuid: &str, info: &str) {
        if let Some(old) = self.workers.remove(&plugin.id) {
            let _ = old.control.send(Control::Stop);
            self.retired.push((plugin.id.clone(), old.thread));
        }
        let mut pending = Vec::new();
        for (id, old) in self.retired.drain(..) {
            if old.is_finished() {
                let _ = old.join();
            } else {
                pending.push((id, old));
            }
        }
        self.retired = pending;
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(PluginStatus::Starting));
        let state = status.clone();
        let plugin = plugin.clone();
        let id = plugin.id.clone();
        let log_dir = self.log_dir.clone();
        let node = self.node.clone();
        let options = self.options.clone();
        let uuid = uuid.to_owned();
        let info = info.to_owned();
        match thread::Builder::new()
            .name(format!("plugin-{id}"))
            .spawn(move || supervise(plugin, port, uuid, info, log_dir, node, options, rx, state))
        {
            Ok(thread) => {
                self.workers.insert(
                    id,
                    Worker {
                        control: tx,
                        status,
                        thread,
                    },
                );
            }
            Err(error) => {
                log::error!("Nelze spustit dohled pluginu {id}: {error}");
            }
        }
    }
    pub fn mark_registered(&self, id: &str) {
        if let Some(worker) = self.workers.get(id) {
            let _ = worker.control.send(Control::Registered);
        }
    }
    pub fn status(&self, id: &str) -> PluginStatus {
        self.workers
            .get(id)
            .map(|worker| {
                worker
                    .status
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            })
            .unwrap_or(PluginStatus::Disabled)
    }
    pub fn stop(&mut self, id: &str) {
        if let Some(worker) = self.workers.remove(id) {
            let _ = worker.control.send(Control::Stop);
            if worker.thread.join().is_err() {
                log::error!("Dohled pluginu {id} selhal");
            }
        }
        let mut pending = Vec::new();
        for (old_id, old) in self.retired.drain(..) {
            if old_id == id {
                if old.join().is_err() {
                    log::error!("Předchozí dohled pluginu {id} selhal");
                }
            } else {
                pending.push((old_id, old));
            }
        }
        self.retired = pending;
    }
    pub fn stop_all(&mut self) {
        let workers: Vec<_> = self.workers.drain().collect();
        for (_, worker) in &workers {
            let _ = worker.control.send(Control::Stop);
        }
        for (id, worker) in workers {
            if worker.thread.join().is_err() {
                log::error!("Dohled pluginu {id} selhal");
            }
        }
        for (_, old) in self.retired.drain(..) {
            let _ = old.join();
        }
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop_all();
    }
}
fn set_status(state: &Mutex<PluginStatus>, value: PluginStatus) {
    *state.lock().unwrap_or_else(|e| e.into_inner()) = value;
}
fn canceled(rx: &mpsc::Receiver<Control>) -> bool {
    loop {
        match rx.try_recv() {
            Ok(Control::Stop) | Err(mpsc::TryRecvError::Disconnected) => return true,
            Ok(Control::Registered) => {}
            Err(mpsc::TryRecvError::Empty) => return false,
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn supervise(
    plugin: InstalledPlugin,
    port: u16,
    uuid: String,
    info: String,
    logs: PathBuf,
    node: NodeResolver,
    options: SupervisorOptions,
    rx: mpsc::Receiver<Control>,
    state: Arc<Mutex<PluginStatus>>,
) {
    let mut backoff = Backoff::new(
        options.initial_backoff,
        options.max_backoff,
        options.stable_after,
    );
    let mut restarts = 0u32;
    loop {
        if canceled(&rx) {
            break;
        }
        let runtime = match plugin.manifest.runtime(&plugin.dir) {
            Ok(runtime) => runtime,
            Err(error) => {
                log::warn!("Plugin {}: {error}", plugin.id);
                set_status(&state, PluginStatus::Crashed { restarts });
                return;
            }
        };
        if matches!(runtime, Runtime::Node { .. }) && node.path().is_none() {
            set_status(&state, PluginStatus::MissingNode);
            return;
        }
        if matches!(runtime, Runtime::Html(_)) {
            log::warn!("Plugin {} vyžaduje WebView hostitele", plugin.id);
            set_status(&state, PluginStatus::Disabled);
            return;
        }
        set_status(&state, PluginStatus::Starting);
        let launch = Instant::now();
        let mut registered = false;
        let mut running_since = None;
        match spawn(&plugin, runtime, port, &uuid, &info, &logs, &node) {
            Ok(mut child) => loop {
                match rx.recv_timeout(options.poll_interval.max(Duration::from_millis(1))) {
                    Ok(Control::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        drop(child);
                        set_status(&state, PluginStatus::Disabled);
                        return;
                    }
                    Ok(Control::Registered) => {
                        if !registered {
                            running_since = Some(Instant::now());
                        }
                        registered = true;
                        set_status(&state, PluginStatus::Running);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                match child.try_wait() {
                    Ok(Some(status)) => {
                        log::warn!("Plugin {} skončil: {status}", plugin.id);
                        break;
                    }
                    Err(error) => {
                        log::warn!("Plugin {}: {error}", plugin.id);
                        break;
                    }
                    Ok(None) => {}
                }
                if !registered && launch.elapsed() >= options.registration_timeout {
                    log::warn!("Plugin {} se nepřipojil včas", plugin.id);
                    break;
                }
            },
            Err(error) => {
                log::warn!("Plugin {} nelze spustit: {error}", plugin.id);
            }
        }
        restarts = restarts.saturating_add(1);
        set_status(&state, PluginStatus::Crashed { restarts });
        let delay = backoff.after_exit(
            running_since
                .map(|started| started.elapsed())
                .unwrap_or(Duration::ZERO),
        );
        let deadline = Instant::now() + delay;
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Control::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    set_status(&state, PluginStatus::Disabled);
                    return;
                }
                Ok(Control::Registered) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => break,
            }
        }
    }
    set_status(&state, PluginStatus::Disabled);
}

#[cfg(all(test, windows))]
fn process_alive(pid: u32) -> bool {
    use windows::Win32::{
        Foundation::CloseHandle,
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0;
        let ok = GetExitCodeProcess(handle, &mut code).is_ok();
        let _ = CloseHandle(handle);
        ok && code == 259
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{json, Value};
    use std::sync::OnceLock;
    use tokio::{
        net::TcpListener,
        time::{timeout, Duration},
    };

    fn plugin() -> InstalledPlugin {
        static HELPER: OnceLock<PathBuf> = OnceLock::new();
        let exe = HELPER.get_or_init(|| {
            let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let result = std::process::Command::new(env!("CARGO"))
                .args(["build", "--manifest-path"])
                .arg(manifest.join("Cargo.toml"))
                .args(["--offline", "--bin", "test_plugin"])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join(format!("test_plugin{}", std::env::consts::EXE_SUFFIX))
        });
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gd-process-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(exe, dir.join("plugin.exe")).unwrap();
        std::fs::write(dir.join("manifest.json"), r#"{"CodePathWin":"plugin.exe","Actions":[{"UUID":"test.action","Name":"Test","States":[{"Image":"icon"}]}]}"#).unwrap();
        InstalledPlugin {
            id: "test.plugin".into(),
            manifest: super::super::manifest::Manifest::load(&dir).unwrap(),
            dir,
        }
    }
    async fn connection(
        listener: &TcpListener,
    ) -> tokio_tungstenite::WebSocketStream<tokio::net::TcpStream> {
        let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        tokio_tungstenite::accept_async(stream).await.unwrap()
    }
    async fn message(ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Value {
        let text = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap();
        serde_json::from_str(&text).unwrap()
    }
    fn options() -> SupervisorOptions {
        SupervisorOptions {
            initial_backoff: Duration::from_millis(80),
            max_backoff: Duration::from_millis(320),
            stable_after: Duration::from_secs(60),
            registration_timeout: Duration::from_millis(600),
            poll_interval: Duration::from_millis(5),
        }
    }
    #[tokio::test]
    async fn spawn_registers_and_echoes_keydown() {
        let p = plugin();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut child = spawn(
            &p,
            p.manifest.runtime(&p.dir).unwrap(),
            listener.local_addr().unwrap().port(),
            "first-token",
            r#"{"application":{"name":"test app"}}"#,
            &p.dir.join("logs"),
            &NodeResolver::with_path(None),
        )
        .unwrap();
        let mut ws = connection(&listener).await;
        assert_eq!(
            message(&mut ws).await,
            json!({"event":"registerPlugin","uuid":"first-token"})
        );
        ws.send(
            json!({"event":"keyDown","context":"profile:3"})
                .to_string()
                .into(),
        )
        .await
        .unwrap();
        assert_eq!(
            message(&mut ws).await,
            json!({"event":"showOk","context":"profile:3"})
        );
        ws.send(
            json!({"event":"sendToPlugin","payload":{"exit":true}})
                .to_string()
                .into(),
        )
        .await
        .unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(3));
        let log = std::fs::read_to_string(p.dir.join("logs/test.plugin.log")).unwrap();
        let launch: Value =
            serde_json::from_str(log.lines().find(|line| line.starts_with('{')).unwrap()).unwrap();
        let cwd: PathBuf = serde_json::from_value(launch["cwd"].clone()).unwrap();
        assert_eq!(cwd.canonicalize().unwrap(), p.dir.canonicalize().unwrap());
        let args = launch["args"].as_array().unwrap();
        assert!(args
            .windows(2)
            .any(|a| a[0] == "-info" && a[1] == r#"{"application":{"name":"test app"}}"#));
        assert!(args
            .windows(2)
            .any(|a| a[0] == "-registerEvent" && a[1] == "registerPlugin"));
        assert!(log.contains("test_plugin stderr"));
        drop(child);
        drop(ws);
        std::fs::remove_dir_all(p.dir).unwrap();
    }
    #[tokio::test]
    async fn crash_restarts_with_backoff_and_original_arguments() {
        let p = plugin();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut supervisor =
            Supervisor::with_options(p.dir.join("logs"), NodeResolver::with_path(None), options());
        let start = Instant::now();
        supervisor.start(&p, listener.local_addr().unwrap().port(), "initial", "{}");
        assert!(start.elapsed() < Duration::from_millis(50));
        let mut ws = connection(&listener).await;
        assert_eq!(message(&mut ws).await["uuid"], "initial");
        supervisor.mark_registered(&p.id);
        ws.send(
            json!({"event":"sendToPlugin","payload":{"exit":true}})
                .to_string()
                .into(),
        )
        .await
        .unwrap();
        let exit_at = Instant::now();
        let mut second = connection(&listener).await;
        assert!(exit_at.elapsed() >= Duration::from_millis(75));
        assert_eq!(message(&mut second).await["uuid"], "initial");
        supervisor.mark_registered(&p.id);
        supervisor.stop(&p.id);
        assert_eq!(supervisor.status(&p.id), PluginStatus::Disabled);
        assert!(timeout(Duration::from_millis(200), listener.accept())
            .await
            .is_err());
        drop(supervisor);
        drop(ws);
        drop(second);
        std::fs::remove_dir_all(p.dir).unwrap();
    }
    #[tokio::test]
    async fn supervisor_drop_kills_job_and_descendant() {
        let p = plugin();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut supervisor =
            Supervisor::with_options(p.dir.join("logs"), NodeResolver::with_path(None), options());
        supervisor.start(&p, listener.local_addr().unwrap().port(), "token", "{}");
        let mut ws = connection(&listener).await;
        message(&mut ws).await;
        ws.send(
            json!({"event":"sendToPlugin","payload":{"spawnChild":true}})
                .to_string()
                .into(),
        )
        .await
        .unwrap();
        let response = message(&mut ws).await;
        let child_id = response["childPid"].as_u64().unwrap() as u32;
        let parent_id = response["parentPid"].as_u64().unwrap() as u32;
        assert!(process_alive(parent_id));
        assert!(process_alive(child_id));
        drop(supervisor);
        for _ in 0..100 {
            if !process_alive(child_id) && !process_alive(parent_id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!process_alive(parent_id));
        assert!(!process_alive(child_id));
        drop(ws);
        std::fs::remove_dir_all(p.dir).unwrap();
    }
    #[test]
    fn backoff_doubles_caps_and_resets() {
        let mut backoff = Backoff::new(
            Duration::from_secs(1),
            Duration::from_secs(60),
            Duration::from_secs(60),
        );
        let delays: Vec<_> = (0..8).map(|_| backoff.after_exit(Duration::ZERO)).collect();
        assert_eq!(
            delays,
            [1, 2, 4, 8, 16, 32, 60, 60].map(Duration::from_secs)
        );
        assert_eq!(
            backoff.after_exit(Duration::from_secs(60)),
            Duration::from_secs(1)
        );
    }
    #[test]
    fn node_absent_is_reported_and_html_is_not_spawned() {
        let p = plugin();
        let mut supervisor =
            Supervisor::with_options(p.dir.join("logs"), NodeResolver::with_path(None), options());
        let mut node = p.clone();
        node.manifest.code_path_win = Some("main.js".into());
        std::fs::write(p.dir.join("main.js"), "").unwrap();
        supervisor.start(&node, 1, "token", "{}");
        for _ in 0..100 {
            if supervisor.status(&p.id) == PluginStatus::MissingNode {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(supervisor.status(&p.id), PluginStatus::MissingNode);
        assert!(spawn(
            &p,
            Runtime::Html(p.dir.join("main.js")),
            1,
            "token",
            "{}",
            &p.dir.join("logs"),
            &NodeResolver::with_path(None)
        )
        .is_err());
        supervisor.stop_all();
        std::fs::remove_dir_all(p.dir).unwrap();
    }
    #[test]
    fn refuses_external_executable_and_unsafe_log_id() {
        let mut p = plugin();
        let outside = std::env::current_exe().unwrap();
        assert!(spawn(
            &p,
            Runtime::Exe(outside),
            1,
            "token",
            "{}",
            &p.dir.join("logs"),
            &NodeResolver::with_path(None)
        )
        .is_err());
        let runtime = p.manifest.runtime(&p.dir).unwrap();
        p.id = "../escape".into();
        assert!(spawn(
            &p,
            runtime,
            1,
            "token",
            "{}",
            &p.dir.join("logs"),
            &NodeResolver::with_path(None)
        )
        .is_err());
        std::fs::remove_dir_all(p.dir).unwrap();
    }
    #[tokio::test]
    async fn unregistered_plugin_times_out_and_restarts_with_backoff() {
        let p = plugin();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut supervisor =
            Supervisor::with_options(p.dir.join("logs"), NodeResolver::with_path(None), options());
        let start = Instant::now();
        supervisor.start(&p, listener.local_addr().unwrap().port(), "token", "{}");
        let mut ws = connection(&listener).await;
        message(&mut ws).await;
        // Deliberately do not notify the supervisor of registration.
        let mut second = connection(&listener).await;
        assert!(start.elapsed() >= Duration::from_millis(600));
        assert_eq!(message(&mut second).await["uuid"], "token");
        drop(supervisor);
        drop(ws);
        drop(second);
        std::fs::remove_dir_all(p.dir).unwrap();
    }
}
