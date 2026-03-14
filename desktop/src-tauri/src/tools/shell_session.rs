use anyhow::Result;
use std::fs;
use std::process::Command;
use std::time::Duration;
use tokio::time::timeout;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Fix commands where an LLM squashed multiple statements onto one line with spaces.
/// Inserts newlines before `$identifier =` patterns that appear after a space
/// but only when the command has no existing newlines or semicolons.
fn normalize_ps_command(cmd: &str) -> String {
    if cmd.contains('\n') || cmd.contains(';') {
        return cmd.to_string();
    }
    // Replace ` $word =` and ` $word +=` with `\n$word =` using a simple pass
    let bytes = cmd.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + 64);
    let mut i = 0;
    while i < len {
        // Look for pattern: space(s) followed by `$` then word chars then optional space then `=`
        if bytes[i] == b' ' {
            // Peek ahead past spaces
            let mut j = i + 1;
            while j < len && bytes[j] == b' ' { j += 1; }
            if j < len && bytes[j] == b'$' {
                // Find end of identifier
                let id_start = j + 1;
                let mut k = id_start;
                while k < len && (bytes[k].is_ascii_alphanumeric() || bytes[k] == b'_') { k += 1; }
                if k > id_start {
                    // Skip optional space
                    let mut m = k;
                    while m < len && bytes[m] == b' ' { m += 1; }
                    if m < len && bytes[m] == b'=' && (m + 1 >= len || bytes[m + 1] != b'=') {
                        // This looks like an assignment — insert newline instead of space
                        out.push('\n');
                        i = j; // skip the spaces, emit from `$` onwards
                        continue;
                    }
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

pub struct ShellSessionManager;

impl ShellSessionManager {
    pub fn new() -> Self {
        Self
    }

    /// Execute a PowerShell command silently by writing to a temp script file
    pub async fn execute(&self, command: &str) -> Result<String> {
        // Create temp script file
        let temp_dir = std::env::temp_dir();
        let script_file = temp_dir.join(format!("bow_{}.ps1", uuid::Uuid::new_v4()));

        // Normalize command: insert newline before $var= patterns squashed onto one line
        // (local LLMs sometimes emit `$a = 1 $b = 2` instead of `$a = 1; $b = 2`)
        let normalized = normalize_ps_command(command);

        // Write command directly — output captured from process stdout
        let script = format!(
            "$ErrorActionPreference = 'Continue'\r\n{}",
            normalized
        );

        fs::write(&script_file, &script)?;

        // Execute script silently
        let fut = tokio::task::spawn_blocking({
            let script = script_file.clone();
            move || {
                let mut cmd = Command::new("powershell.exe");
                cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", script.to_str().unwrap()]);

                #[cfg(target_os = "windows")]
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

                let result = cmd.output();

                // Clean up script file
                let _ = fs::remove_file(&script);

                result
            }
        });

        // Wait for execution with timeout
        let exec_result = timeout(Duration::from_secs(120), fut)
            .await
            .map_err(|_| anyhow::anyhow!("shell_exec timed out after 120 seconds"))?
            .map_err(|e| anyhow::anyhow!("Failed to spawn PowerShell: {}", e))?
            .map_err(|e| anyhow::anyhow!("PowerShell execution error: {}", e))?;

        let stdout = String::from_utf8_lossy(&exec_result.stdout).to_string();
        let stderr = String::from_utf8_lossy(&exec_result.stderr).to_string();

        if !exec_result.status.success() && !stderr.is_empty() && stdout.is_empty() {
            return Err(anyhow::anyhow!("PowerShell error: {}", stderr));
        }

        // Strip UTF-8 BOM if present, then trim
        let output = stdout.trim_start_matches('\u{feff}').trim().to_string();

        if output.is_empty() {
            // Fall back to stderr if stdout was empty (e.g. Write-Error output)
            let err_trimmed = stderr.trim_start_matches('\u{feff}').trim().to_string();
            if !err_trimmed.is_empty() {
                Ok(err_trimmed)
            } else {
                Ok("(no output)".to_string())
            }
        } else {
            Ok(output)
        }
    }

    pub fn clone_handle(&self) -> Self {
        Self
    }
}

impl Clone for ShellSessionManager {
    fn clone(&self) -> Self {
        self.clone_handle()
    }
}
