//! Utility functions

use std::path::Path;
use std::time::Duration;
use tokio::process::Command;
use crate::{Error, Result};

/// Run a command with timeout and capture output
pub async fn run_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
    working_dir: Option<&Path>,
) -> Result<CommandOutput> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    cmd.kill_on_drop(true);

    let output = tokio::time::timeout(timeout, cmd.output()).await
        .map_err(|_| Error::Timeout(format!("Command timed out after {:?}: {} {}", timeout, program, args.join(" "))))?
        .map_err(|e| Error::Io(e))?;

    Ok(CommandOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

/// Command output
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.status == 0
    }

    pub fn ensure_success(&self) -> Result<()> {
        if !self.success() {
            return Err(Error::Sandbox(format!(
                "Command failed with status {}: stdout={} stderr={}",
                self.status, self.stdout, self.stderr
            )));
        }
        Ok(())
    }
}

/// Calculate percentage delta
pub fn pct_delta(before: f64, after: f64) -> f64 {
    if before == 0.0 {
        return if after == 0.0 { 0.0 } else { 100.0 };
    }
    ((after - before) / before) * 100.0
}

/// Format bytes human readable
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    format!("{:.2} {}", size, UNITS[unit_idx])
}

/// Format duration human readable
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Generate a short hash for display
pub fn short_hash(data: &[u8], len: usize) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    format!("{:x}", hasher.finish())[..len.min(16)].to_string()
}

/// Parse human-readable size (e.g., "512M", "2G")
pub fn parse_size(s: &str) -> Result<u64> {
    let s = s.trim().to_uppercase();
    let (num_str, unit) = if s.ends_with("K") || s.ends_with("M") || s.ends_with("G") || s.ends_with("T") {
        (&s[..s.len()-1], s.chars().last().unwrap())
    } else {
        (s.as_str(), 'B')
    };
    let num: f64 = num_str.parse().map_err(|_| Error::InvalidState(format!("Invalid size: {}", s)))?;
    let multiplier = match unit {
        'K' => 1024,
        'M' => 1024 * 1024,
        'G' => 1024 * 1024 * 1024,
        'T' => 1024 * 1024 * 1024 * 1024,
        'B' => 1,
        _ => return Err(Error::InvalidState(format!("Unknown unit: {}", unit))),
    };
    Ok((num * multiplier as f64) as u64)
}

/// Ensure directory exists
pub fn ensure_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
    }
    Ok(())
}

/// Atomic write to file
pub fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, content)?;
    std::fs::rename(tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pct_delta() {
        assert_eq!(pct_delta(100.0, 110.0), 10.0);
        assert_eq!(pct_delta(100.0, 90.0), -10.0);
        assert_eq!(pct_delta(0.0, 10.0), 100.0);
        assert_eq!(pct_delta(0.0, 0.0), 0.0);
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(512), "512.00 B");
        assert_eq!(format_bytes(1024), "1.00 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MiB");
    }

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("512M").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_size("2G").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("1024").unwrap(), 1024);
    }
}