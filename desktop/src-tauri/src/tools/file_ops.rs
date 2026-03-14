use anyhow::Result;
use std::path::Path;
use std::fs;

pub fn file_read(path: &str) -> Result<String> {
    let p = Path::new(path);
    let content = std::fs::read_to_string(p)
        .map_err(|e| anyhow::anyhow!("file_read failed for '{}': {}", path, e))?;
    Ok(content)
}

pub fn file_list(dir: &str) -> Result<String> {
    let entries = fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("file_list failed for '{}': {}", dir, e))?;

    let mut lines: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let meta = entry.metadata().ok();
        let name = entry.file_name().to_string_lossy().to_string();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let kind = if meta.as_ref().map(|m| m.is_dir()).unwrap_or(false) { "[dir]" } else { "[file]" };
        lines.push(format!("{} {} ({} bytes)", kind, name, size));
    }

    if lines.is_empty() {
        return Ok(format!("{} is empty", dir));
    }
    lines.sort();
    Ok(format!("{} ({} entries):\n{}", dir, lines.len(), lines.join("\n")))
}

pub fn file_write(path: &str, content: &str) -> Result<String> {
    let p = Path::new(path);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("Failed to create directories for '{}': {}", path, e))?;
    }
    std::fs::write(p, content)
        .map_err(|e| anyhow::anyhow!("file_write failed for '{}': {}", path, e))?;
    Ok(format!("Written {} bytes to {}", content.len(), path))
}
