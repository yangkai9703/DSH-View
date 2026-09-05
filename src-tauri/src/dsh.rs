use std::path::PathBuf;
use std::process::{Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Suppress the console window Windows would otherwise allocate for console
/// children of this GUI process.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const DEFAULT_PORT: u16 = 3080;

/// Locate the globally installed dsh CLI entry (`npm i -g @deepseek-ai/dsh`)
/// so we can run `node <bin.js>` directly — a single-process chain that our
/// job object fully covers. Returns None when only npx is available.
fn global_bin_js() -> Option<PathBuf> {
    let candidates = [
        // npm global prefix on Windows
        std::env::var("APPDATA")
            .ok()
            .map(|appdata| PathBuf::from(appdata).join("npm/node_modules/@deepseek-ai/dsh/lib/bin.js")),
    ];
    for c in candidates.into_iter().flatten() {
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

/// Build the command that serves `dsh web` on `port`, with the DSH process
/// rooted at `cwd` — DSH derives its default workspace from the process cwd,
/// so this decides where "default" sessions work.
///
/// Preferred: `node <global bin.js>` (one process, job object covers it).
/// Fallback: `cmd /c npx -y @deepseek-ai/dsh web` (npx resolves on PATH;
/// its grandchildren may break away from the job, but kill_dsh additionally
/// kills the port owner).
pub fn web_command(port: u16, cwd: &PathBuf) -> Command {
    let port_arg = port.to_string();
    if let Some(bin) = global_bin_js() {
        let mut cmd = Command::new("node");
        cmd.arg(bin)
            .arg("web")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(&port_arg)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        #[cfg(windows)]
        // The shell is a GUI process; without CREATE_NO_WINDOW every console
        // child (node) pops up its own console window.
        cmd.creation_flags(CREATE_NO_WINDOW);
        return cmd;
    }

    let args = [
        "npx",
        "-y",
        "@deepseek-ai/dsh",
        "web",
        "--host",
        "127.0.0.1",
        "--port",
        &port_arg.as_str(),
    ];
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new("cmd");
        cmd.arg("/c").args(args);
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.current_dir(cwd);
        cmd
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut cmd = Command::new(args[0]);
        cmd.args(&args[1..]);
        cmd.current_dir(cwd);
        cmd
    }
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .stdin(Stdio::null())
}

pub fn url_port(url: &str) -> u16 {
    url.rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}