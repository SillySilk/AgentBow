//! Persistent PowerShell session.
//!
//! A single long-lived `powershell.exe` process is kept alive and fed commands
//! over its stdin. State persists across calls — `cd`, `$env:` vars, and
//! PowerShell variables set in one `shell_exec` are visible in the next, just
//! like a real terminal.
//!
//! Each command is bracketed by unique start/end sentinel lines so we can read
//! exactly that command's output back. A per-command timeout kills and respawns
//! the session if a command hangs, so one bad command can't wedge the agent.

use anyhow::{anyhow, Result};
use base64::Engine as _;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// Fix commands where an LLM squashed multiple statements onto one line with
/// spaces (`$a = 1 $b = 2` → `$a = 1` / `$b = 2`). Only fires when the command
/// has no existing newlines or semicolons.
fn normalize_ps_command(cmd: &str) -> String {
    if cmd.contains('\n') || cmd.contains(';') {
        return cmd.to_string();
    }
    let bytes = cmd.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + 64);
    let mut i = 0;
    while i < len {
        if bytes[i] == b' ' {
            let mut j = i + 1;
            while j < len && bytes[j] == b' ' { j += 1; }
            if j < len && bytes[j] == b'$' {
                let id_start = j + 1;
                let mut k = id_start;
                while k < len && (bytes[k].is_ascii_alphanumeric() || bytes[k] == b'_') { k += 1; }
                if k > id_start {
                    let mut m = k;
                    while m < len && bytes[m] == b' ' { m += 1; }
                    if m < len && bytes[m] == b'=' && (m + 1 >= len || bytes[m + 1] != b'=') {
                        out.push('\n');
                        i = j;
                        continue;
                    }
                }
            }
        }
        // Push the byte as a char. Command text is effectively ASCII here; the
        // normalizer is a no-op for anything with newlines/semicolons anyway.
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Filled by a background task draining the child's stderr.
    stderr_buf: Arc<Mutex<String>>,
    /// Unique per session; used to build the start/end sentinel lines.
    sentinel: String,
}

/// Cheaply-cloneable handle to the shared persistent shell. All clones drive the
/// same underlying PowerShell process.
#[derive(Clone)]
pub struct ShellSessionManager {
    inner: Arc<Mutex<Option<Session>>>,
}

impl ShellSessionManager {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(None)) }
    }

    /// Run `command` in the persistent shell, returning its combined output.
    /// Spawns the session on first use and respawns it if a previous command
    /// timed out or the process died.
    pub async fn execute(&self, command: &str) -> Result<String> {
        let normalized = normalize_ps_command(command);

        let mut guard = self.inner.lock().await;
        if guard.is_none() {
            *guard = Some(spawn_session().await?);
        }

        let result = {
            let sess = guard.as_mut().unwrap();
            run_one(sess, &normalized).await
        };

        if result.is_err() {
            // The session is likely poisoned (hung command killed by timeout, or
            // a broken pipe). Drop it so the next call starts a fresh shell.
            if let Some(mut sess) = guard.take() {
                let _ = sess.child.start_kill();
            }
        }
        result
    }
}

impl Default for ShellSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

async fn spawn_session() -> Result<Session> {
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NoLogo"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    #[cfg(target_os = "windows")]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW

    let mut child = command
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn PowerShell: {}", e))?;

    let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
    let stdout = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);
    let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;

    // Background-drain stderr so it never blocks the child and we can fold it
    // into command output.
    let stderr_buf = Arc::new(Mutex::new(String::new()));
    {
        let buf = stderr_buf.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut b = buf.lock().await;
                b.push_str(&line);
                b.push('\n');
            }
        });
    }

    let sentinel = format!("BOW_{}", uuid::Uuid::new_v4().simple());

    let mut sess = Session { child, stdin, stdout, stderr_buf, sentinel };

    // Initialise the shell: continue past errors, no progress bars, UTF-8 out.
    sess.stdin
        .write_all(
            b"$ErrorActionPreference='Continue'\n\
              $ProgressPreference='SilentlyContinue'\n\
              [Console]::OutputEncoding=[System.Text.Encoding]::UTF8\n",
        )
        .await
        .map_err(|e| anyhow!("Failed to initialise PowerShell session: {}", e))?;
    sess.stdin.flush().await?;

    Ok(sess)
}

async fn run_one(sess: &mut Session, cmd: &str) -> Result<String> {
    let start = format!("{}_START", sess.sentinel);
    let end = format!("{}_END", sess.sentinel);

    // Clear any stderr left over from a previous command.
    sess.stderr_buf.lock().await.clear();

    // Everything must go on ONE input line: PowerShell echoes each input line it
    // reads, and a multi-line payload would interleave those echoes with the
    // command's real output between our sentinels. We base64-encode the command
    // and run it via Invoke-Expression, which (a) lets arbitrary multi-line
    // commands ride inside a single line and (b) executes in the *current* scope
    // so `$vars`, `$env:`, and `cd` persist across calls. The echoed input line
    // contains the sentinel only as a substring, so exact-line matching on the
    // read side still isolates the genuine sentinel output.
    let b64 = base64::engine::general_purpose::STANDARD.encode(cmd.as_bytes());
    let payload = format!(
        "Write-Output \"{start}\"; \
         Invoke-Expression ([System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String(\"{b64}\"))); \
         Write-Output \"{end}\"\n"
    );
    sess.stdin
        .write_all(payload.as_bytes())
        .await
        .map_err(|e| anyhow!("shell write failed (session lost): {}", e))?;
    sess.stdin.flush().await?;

    let lines = timeout(COMMAND_TIMEOUT, read_until_end(&mut sess.stdout, &start, &end))
        .await
        .map_err(|_| anyhow!("shell_exec timed out after {}s", COMMAND_TIMEOUT.as_secs()))??;

    // Let the async stderr drain catch up to this command before we read it.
    tokio::time::sleep(Duration::from_millis(40)).await;
    let err = sess.stderr_buf.lock().await.trim().to_string();

    let mut out = lines.join("\n").trim().to_string();
    if !err.is_empty() {
        out = if out.is_empty() { err } else { format!("{out}\n{err}") };
    }
    if out.is_empty() {
        out = "(no output)".to_string();
    }
    Ok(out)
}

/// Read stdout lines, discarding everything up to the exact `start` sentinel and
/// collecting lines until the exact `end` sentinel.
async fn read_until_end(
    stdout: &mut BufReader<ChildStdout>,
    start: &str,
    end: &str,
) -> Result<Vec<String>> {
    let mut lines: Vec<String> = Vec::new();
    let mut seen_start = false;
    let mut line = String::new();
    loop {
        line.clear();
        let n = stdout
            .read_line(&mut line)
            .await
            .map_err(|e| anyhow!("shell read failed: {}", e))?;
        if n == 0 {
            return Err(anyhow!("shell session closed unexpectedly"));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if !seen_start {
            if trimmed == start {
                seen_start = true;
            }
            continue;
        }
        if trimmed == end {
            break;
        }
        lines.push(trimmed.to_string());
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn state_persists_across_commands() {
        let mgr = ShellSessionManager::new();
        let a = mgr.execute("$bowtest = 123").await.unwrap();
        // assignment produces no output
        assert!(a.contains("no output") || a.trim().is_empty(), "got: {a:?}");
        let b = mgr.execute("$bowtest + 1").await.unwrap();
        assert_eq!(b.trim(), "124");
    }

    #[tokio::test]
    async fn cwd_persists_across_commands() {
        let mgr = ShellSessionManager::new();
        mgr.execute("Set-Location C:\\Windows").await.unwrap();
        let pwd = mgr.execute("(Get-Location).Path").await.unwrap();
        assert_eq!(pwd.trim().to_lowercase(), "c:\\windows");
    }

    #[tokio::test]
    async fn captures_simple_output() {
        let mgr = ShellSessionManager::new();
        let out = mgr.execute("Write-Output 'hello world'").await.unwrap();
        assert_eq!(out.trim(), "hello world");
    }

    #[tokio::test]
    async fn handles_multiline_command() {
        // Exercises the base64 + Invoke-Expression path with embedded newlines.
        let mgr = ShellSessionManager::new();
        let out = mgr.execute("$a = 2\n$b = 3\n$a * $b").await.unwrap();
        assert_eq!(out.trim(), "6");
    }

    #[tokio::test]
    async fn captures_stderr() {
        let mgr = ShellSessionManager::new();
        let out = mgr.execute("Write-Error 'boom'").await.unwrap();
        assert!(out.contains("boom"), "stderr not captured: {out:?}");
    }
}
