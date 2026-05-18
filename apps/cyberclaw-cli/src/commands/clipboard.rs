//! `cyberclaw clipboard` — cross-platform clipboard read/write helper.
//!
//! Wraps the OS-native clipboard tooling so scripts can pipe Agent output
//! to/from the system clipboard without depending on a particular shell
//! environment.
//!
//! Backend selection (probed at runtime, first available wins):
//!   · macOS  → `pbcopy` / `pbpaste`
//!   · Linux  → `wl-copy` / `wl-paste` (Wayland) or `xclip` / `xsel` (X11)
//!   · Windows → `clip` (write) / PowerShell `Get-Clipboard` (read)
//!
//! No third-party crate dependency on purpose — every supported platform
//! has a system binary that does this correctly. Avoids the
//! arboard/clipboard-master/copypasta tangle that drags in OS-specific
//! C deps just to call the same binaries internally.

use anyhow::{anyhow, Context, Result};
use clap::Subcommand;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

#[derive(Subcommand, Debug)]
pub enum ClipboardCommand {
    /// Print the current clipboard contents to stdout.
    /// Exit 0 with empty output when the clipboard is empty.
    Read,
    /// Read stdin (or --text) and write it to the clipboard.
    Write {
        /// Inline text. If omitted, stdin is read.
        #[arg(long)]
        text: Option<String>,
    },
}

pub async fn handle_clipboard_command(cmd: ClipboardCommand) -> Result<()> {
    match cmd {
        ClipboardCommand::Read => {
            let out = read_clipboard()?;
            // Print without a trailing newline — the caller can decide.
            // tokio-aware: clipboard tools are short-lived shell-outs, fine to
            // run synchronously inside an async handler.
            print!("{}", out);
            std::io::stdout().flush().ok();
            Ok(())
        }
        ClipboardCommand::Write { text } => {
            let body = match text {
                Some(t) => t,
                None => {
                    let mut buf = String::new();
                    std::io::stdin()
                        .read_to_string(&mut buf)
                        .context("read stdin for clipboard write")?;
                    buf
                }
            };
            write_clipboard(&body)?;
            eprintln!("clipboard: wrote {} bytes", body.len());
            Ok(())
        }
    }
}

/// Pick the first available backend, return its (read_cmd, write_cmd) pair.
fn pick_backend() -> Result<Backend> {
    // macOS — always present.
    if cfg!(target_os = "macos") && which("pbcopy") {
        return Ok(Backend::Mac);
    }
    // Linux — prefer Wayland, then X11.
    #[cfg(target_os = "linux")]
    {
        if which("wl-copy") && which("wl-paste") {
            return Ok(Backend::Wayland);
        }
        if which("xclip") {
            return Ok(Backend::Xclip);
        }
        if which("xsel") {
            return Ok(Backend::Xsel);
        }
    }
    // Windows.
    #[cfg(target_os = "windows")]
    {
        if which("clip") {
            return Ok(Backend::Windows);
        }
    }
    Err(anyhow!(
        "no clipboard backend found.\n\
         · macOS  : pbcopy/pbpaste should be preinstalled\n\
         · Linux  : install one of wl-clipboard / xclip / xsel\n\
         · Windows: clip should be preinstalled"
    ))
}

#[derive(Debug, Clone, Copy)]
enum Backend {
    Mac,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Wayland,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Xclip,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Xsel,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Windows,
}

fn which(prog: &str) -> bool {
    Command::new("which")
        .arg(prog)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn read_clipboard() -> Result<String> {
    let backend = pick_backend()?;
    let (cmd, args): (&str, &[&str]) = match backend {
        Backend::Mac => ("pbpaste", &[]),
        Backend::Wayland => ("wl-paste", &["--no-newline"]),
        Backend::Xclip => ("xclip", &["-selection", "clipboard", "-o"]),
        Backend::Xsel => ("xsel", &["--clipboard", "--output"]),
        Backend::Windows => ("powershell", &["-NoProfile", "-Command", "Get-Clipboard"]),
    };
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("run {} {:?}", cmd, args))?;
    if !out.status.success() {
        return Err(anyhow!(
            "{} failed ({}): {}",
            cmd,
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn write_clipboard(body: &str) -> Result<()> {
    let backend = pick_backend()?;
    let (cmd, args): (&str, &[&str]) = match backend {
        Backend::Mac => ("pbcopy", &[]),
        Backend::Wayland => ("wl-copy", &[]),
        Backend::Xclip => ("xclip", &["-selection", "clipboard"]),
        Backend::Xsel => ("xsel", &["--clipboard", "--input"]),
        Backend::Windows => ("clip", &[]),
    };
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn {} {:?}", cmd, args))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(body.as_bytes())
            .with_context(|| format!("write to {} stdin", cmd))?;
    }
    let status = child.wait().with_context(|| format!("wait for {}", cmd))?;
    if !status.success() {
        return Err(anyhow!("{} exited with {}", cmd, status));
    }
    Ok(())
}
