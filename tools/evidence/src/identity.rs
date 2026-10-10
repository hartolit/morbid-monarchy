//! Source, toolchain and host identity recorded in every report.
//!
//! A result is only comparable with another when both name the exact source, dirty changes,
//! dependency lock and compiler they were produced with. Values that cannot be read are recorded
//! as `None` rather than guessed.

use std::{
    fs,
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize)]
pub struct Identity {
    pub source: SourceIdentity,
    pub toolchain: ToolchainIdentity,
    pub host: HostIdentity,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceIdentity {
    pub commit: Option<String>,
    pub branch: Option<String>,
    /// True when tracked files differ from `commit` or untracked, non-ignored files exist.
    pub dirty: bool,
    /// SHA-256 of `git diff HEAD --binary`, present only when tracked files differ.
    pub dirty_diff_sha256: Option<String>,
    pub untracked_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolchainIdentity {
    pub rustc: Option<String>,
    pub rustc_commit: Option<String>,
    pub cargo: Option<String>,
    pub pinned_channel: Option<String>,
    pub lockfile_sha256: Option<String>,
    /// `release` for the runner itself when built through the `cargo evidence` alias.
    pub runner_profile: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostIdentity {
    pub os: &'static str,
    pub kernel: Option<String>,
    pub cpu: Option<String>,
    pub logical_cpus: Option<usize>,
    pub memory_bytes: Option<u64>,
}

impl Identity {
    pub fn collect(root: &Path) -> Self {
        Self {
            source: SourceIdentity::collect(root),
            toolchain: ToolchainIdentity::collect(root),
            host: HostIdentity::collect(),
        }
    }
}

impl SourceIdentity {
    fn collect(root: &Path) -> Self {
        let commit = git(root, &["rev-parse", "HEAD"]);
        let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]);
        let diff = git_bytes(root, &["diff", "HEAD", "--binary"]);
        let dirty_diff_sha256 = diff
            .filter(|bytes| !bytes.is_empty())
            .map(|bytes| sha256_hex(&bytes));
        let untracked_files: Vec<String> =
            git(root, &["ls-files", "--others", "--exclude-standard"])
                .map(|list| list.lines().map(str::to_owned).collect())
                .unwrap_or_default();
        Self {
            commit,
            branch,
            dirty: dirty_diff_sha256.is_some() || !untracked_files.is_empty(),
            dirty_diff_sha256,
            untracked_files,
        }
    }
}

impl ToolchainIdentity {
    fn collect(root: &Path) -> Self {
        let verbose = command_output(root, "rustc", &["-vV"]);
        let field = |name: &str| {
            verbose.as_deref().and_then(|text| {
                text.lines()
                    .find_map(|line| line.strip_prefix(name).map(|value| value.trim().to_owned()))
            })
        };
        let pinned_channel = fs::read_to_string(root.join("rust-toolchain.toml"))
            .ok()
            .and_then(|text| toml::from_str::<toml::Table>(&text).ok())
            .and_then(|table| {
                table
                    .get("toolchain")?
                    .get("channel")?
                    .as_str()
                    .map(str::to_owned)
            });
        Self {
            rustc: verbose
                .as_deref()
                .and_then(|text| text.lines().next())
                .map(str::to_owned),
            rustc_commit: field("commit-hash:"),
            cargo: command_output(root, "cargo", &["-V"]).map(|text| text.trim().to_owned()),
            pinned_channel,
            lockfile_sha256: fs::read(root.join("Cargo.lock"))
                .ok()
                .map(|bytes| sha256_hex(&bytes)),
            runner_profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
        }
    }
}

impl HostIdentity {
    fn collect() -> Self {
        let cpuinfo = fs::read_to_string("/proc/cpuinfo").ok();
        let meminfo = fs::read_to_string("/proc/meminfo").ok();
        Self {
            os: std::env::consts::OS,
            kernel: fs::read_to_string("/proc/sys/kernel/osrelease")
                .ok()
                .map(|text| text.trim().to_owned()),
            cpu: cpuinfo.as_deref().and_then(|text| {
                text.lines()
                    .find(|line| line.starts_with("model name"))
                    .and_then(|line| line.split_once(':'))
                    .map(|(_, value)| value.trim().to_owned())
            }),
            logical_cpus: std::thread::available_parallelism().ok().map(usize::from),
            memory_bytes: meminfo.as_deref().and_then(|text| {
                let line = text.lines().find(|line| line.starts_with("MemTotal:"))?;
                let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
                kib.checked_mul(1024)
            }),
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    fs::read(path)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| format!("read {}: {error}", path.display()))
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let bytes = git_bytes(root, args)?;
    String::from_utf8(bytes)
        .ok()
        .map(|text| text.trim().to_owned())
}

fn git_bytes(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn command_output(root: &Path, program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Seconds since the Unix epoch, or zero on a clock set before 1970.
pub fn now_unix_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}

/// Formats Unix seconds as an RFC 3339 UTC timestamp, for example `2026-10-10T21:58:03Z`.
pub fn rfc3339_utc(unix_seconds: f64) -> String {
    let (date, time) = civil_utc(unix_seconds);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        date.0, date.1, date.2, time.0, time.1, time.2
    )
}

/// A compact timestamp for run directory names, for example `20261010T215803Z`.
pub fn compact_utc(unix_seconds: f64) -> String {
    let (date, time) = civil_utc(unix_seconds);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        date.0, date.1, date.2, time.0, time.1, time.2
    )
}

/// Converts Unix seconds to a proleptic Gregorian UTC date and time of day.
///
/// Uses Howard Hinnant's days-from-civil inverse, which is exact for every day representable
/// here; a dependency on a calendar crate is not justified for timestamps alone.
fn civil_utc(unix_seconds: f64) -> ((i64, u32, u32), (u32, u32, u32)) {
    let seconds = unix_seconds.floor() as i64;
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400) as u32;

    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    (
        (year, month, day),
        (
            second_of_day / 3_600,
            second_of_day % 3_600 / 60,
            second_of_day % 60,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_formatting_matches_known_instants() {
        assert_eq!(rfc3339_utc(0.0), "1970-01-01T00:00:00Z");
        // 2000-02-29 is a leap day in a century divisible by 400.
        assert_eq!(rfc3339_utc(951_782_400.0), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339_utc(1_791_669_483.9), "2026-10-10T21:58:03Z");
        assert_eq!(compact_utc(1_791_669_483.0), "20261010T215803Z");
    }

    #[test]
    fn sha256_matches_the_standard_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
