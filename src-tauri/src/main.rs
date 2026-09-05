#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod dsh;
#[cfg(windows)]
mod job;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Suppress the console window Windows would otherwise allocate for console
/// children (taskkill/powershell) of this GUI process.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, RunEvent, WindowEvent};

#[derive(Clone, serde::Serialize, Default)]
struct Status {
    stage: String,
    message: Option<String>,
    url: Option<String>,
}

#[derive(Default)]
struct ShellState {
    child: Mutex<Option<Child>>,
    url: Mutex<Option<String>>,
    // "" = not started, "launching"/"waiting" in flight, then ready/error.
    last_status: Mutex<Status>,
    spawn_started: Mutex<bool>,
}

/// True when the loopback port already answers HTTP.
fn port_ready(port: u16) -> bool {
    let mut stream = match TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let req = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 64];
    let mut n = 0;
    while n < buf.len() {
        match stream.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => break,
        }
    }
    // Any HTTP response means a server owns the port.
    buf[..n].starts_with(b"HTTP/")
}

/// Pick the first port in 3080..3090 that is either free or already serving
/// HTTP (in which case DSH is up from a previous run and we just reuse it).
fn pick_port(start: u16) -> Result<u16, String> {
    for port in start..start + 10 {
        if port_ready(port) {
            return Ok(port);
        }
        match TcpStream::connect(("127.0.0.1", port)) {
            // Something else owns the port, skip it.
            Ok(_) => continue,
            Err(_) => return Ok(port),
        }
    }
    Err(format!("no free port found in {start}..{}", start + 10))
}

/// Resolve the working directory DSH should be rooted at:
/// 1. First non-flag command-line argument that exists as a directory
///    (`dsh-shell.exe C:\path\to\project`), else
/// 2. the user's home directory.
///
/// DSH derives its default workspace root from the process cwd; without this,
/// double-click launches inherit the exe's folder (or C:\Windows\System32),
/// which is why picked-up project directories never became the default.
fn resolve_dsh_cwd() -> PathBuf {
    for arg in std::env::args().skip(1) {
        let p = PathBuf::from(&arg);
        if !arg.starts_with('-') && p.is_dir() {
            return p;
        }
    }
    dirs_home()
}

fn dirs_home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Spawn `npx @deepseek-ai/dsh web` on `port` unless it is already serving.
fn spawn_dsh(state: &ShellState) -> Result<(), String> {
    let port = pick_port(dsh::DEFAULT_PORT)?;
    let url = format!("http://127.0.0.1:{port}");

    if !port_ready(port) {
        let cwd = resolve_dsh_cwd();
        eprintln!("[dsh-shell] dsh cwd = {}", cwd.display());
        let mut child = dsh::web_command(port, &cwd)
            .spawn()
            .map_err(|e| format!("failed to launch npx dsh: {e}"))?;
        // If npx or the package is broken the process exits within seconds;
        // detect that early instead of waiting on a port that never opens.
        std::thread::sleep(Duration::from_secs(3));
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                return Err(format!(
                    "dsh exited immediately (exit {status}). stderr: {}",
                    err.trim()
                ));
            }
            Ok(None) => {
                // Bind the whole child tree to our lifetime: when the shell
                // exits, Windows kills cmd/npx/node via the job object.
                #[cfg(windows)]
                {
                    job::bind(child.id());
                    eprintln!(
                        "[dsh-shell] job bind pid={} bound={}",
                        child.id(),
                        job::is_bound(child.id())
                    );
                }
                // Keep pipes drained so a chatty server never blocks on a full
                // pipe buffer.
                if let Some(out) = child.stdout.take() {
                    drain_pipe(out);
                }
                if let Some(err) = child.stderr.take() {
                    drain_pipe(err);
                }
            }
            Err(e) => return Err(format!("failed to poll dsh process: {e}")),
        }
        *state.child.lock().unwrap() = Some(child);
    }

    *state.url.lock().unwrap() = Some(url);
    Ok(())
}

/// Poll until the port answers HTTP, or give up after `timeout`.
fn wait_ready(state: &ShellState, timeout: Duration) -> Result<String, String> {
    let url = state
        .url
        .lock()
        .unwrap()
        .clone()
        .ok_or("dsh not spawned yet")?;
    let port = dsh::url_port(&url);
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if port_ready(port) {
            return Ok(url);
        }
        // Surface early crashes instead of stalling the whole timeout.
        if let Some(child) = state.child.lock().unwrap().as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("dsh exited during startup (exit {status})"));
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!("dsh not ready after {}s at {url}", timeout.as_secs()))
}

fn kill_dsh(state: &ShellState) {
    let url = state.url.lock().unwrap().clone();
    if let Some(mut child) = state.child.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    // npx spawns deeper descendants (cmd→npx→cmd→node) that may break away
    // from the job object, so also kill whoever still listens on our port.
    if let Some(url) = url {
        let port = dsh::url_port(&url);
        for pid in port_listeners(port) {
            let mut cmd = Command::new("taskkill");
            cmd.args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .stdin(Stdio::null());
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);
            let _ = cmd.status();
        }
    }
    #[cfg(windows)]
    job::drop_dsh_job();
}

/// PIDs of processes with a listening socket on `port` (via Get-NetTCPConnection).
fn port_listeners(port: u16) -> Vec<u32> {
    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-WindowStyle",
        "Hidden",
        "-Command",
        &format!(
            "(Get-NetTCPConnection -LocalPort {port} -State Listen -ErrorAction SilentlyContinue).OwningProcess | Sort-Object -Unique"
        ),
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Drain a child's output pipe in the background; without this the pipe
/// buffer fills up and blocks the child once it logs enough.
fn drain_pipe(mut pipe: impl Read + Send + 'static) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });
}

/// Kick off the DSH supervisor thread once, from the first status poll.
fn ensure_spawned(handle: &AppHandle) {
    let state = handle.state::<ShellState>();
    let mut spawned = state.spawn_started.lock().unwrap();
    if *spawned {
        return;
    }
    *spawned = true;
    drop(spawned);

    let handle = handle.clone();
    std::thread::spawn(move || {
        let state: &ShellState = handle.state::<ShellState>().inner();

        let result = spawn_dsh(state).and_then(|_| wait_ready(state, Duration::from_secs(180)));
        *state.last_status.lock().unwrap() = match &result {
            Ok(url) => Status {
                stage: "ready".into(),
                message: None,
                url: Some(url.clone()),
            },
            Err(e) => Status {
                stage: "error".into(),
                message: Some(e.clone()),
                url: None,
            },
        };
    });
}

#[tauri::command]
fn dsh_status(handle: tauri::AppHandle) -> Status {
    ensure_spawned(&handle);
    handle.state::<ShellState>().last_status.lock().unwrap().clone()
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![dsh_status])
        .setup(|app| {
            let state = ShellState::default();
            app.manage(state);
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::Destroyed) {
                if let Some(state) = window.app_handle().try_state::<ShellState>() {
                    kill_dsh(&state);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // The Destroyed handler above covers the window closing, but the
            // process itself must also clean up on any exit path (e.g. dev
            // server dying, Ctrl+C).
            if let RunEvent::ExitRequested { .. } = event {
                if let Some(state) = app.try_state::<ShellState>() {
                    kill_dsh(&state);
                }
            }
        });
}