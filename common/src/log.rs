//! Append-only text log file with timestamp + level.
//!
//! Process-global via `OnceLock<Logger>`; once initialized further
//! `init()` calls are silently ignored. Safe to call `info/warn/error`
//! before init - those calls become no-ops. Every write is `flush`ed
//! immediately so a crashed install still leaves a complete log.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

static LOG: OnceLock<Logger> = OnceLock::new();

pub struct Logger {
    file: Mutex<Option<File>>,
    path: PathBuf,
}

#[derive(Copy, Clone)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Info => "INFO ",
            Level::Warn => "WARN ",
            Level::Error => "ERROR",
        }
    }
}

/// Open / create the log file. Re-init is silently ignored.
pub fn init(path: impl Into<PathBuf>) {
    let path = path.into();
    if LOG.get().is_some() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok();
    let _ = LOG.set(Logger {
        file: Mutex::new(file),
        path,
    });
}

/// Where the active log is being written. `None` if `init` hasn't run
/// or the open failed.
pub fn current_path() -> Option<PathBuf> {
    LOG.get().map(|l| l.path.clone())
}

pub fn info(msg: impl AsRef<str>) {
    write_line(Level::Info, msg.as_ref());
}
pub fn warn(msg: impl AsRef<str>) {
    write_line(Level::Warn, msg.as_ref());
}
pub fn error(msg: impl AsRef<str>) {
    write_line(Level::Error, msg.as_ref());
}

fn write_line(lvl: Level, msg: &str) {
    let Some(logger) = LOG.get() else { return };
    let mut guard = match logger.file.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(file) = guard.as_mut() else { return };
    let ts = iso_utc(SystemTime::now());
    let line = format!("{} {} {}\n", ts, lvl.tag(), msg);
    let _ = file.write_all(line.as_bytes());
    let _ = file.flush();
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` without pulling in chrono.
fn iso_utc(t: SystemTime) -> String {
    let dur = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let ms = dur.subsec_millis();
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400) as u32;
    let (y, mo, d) = days_to_ymd(days);
    let h = tod / 3600;
    let m = (tod % 3600) / 60;
    let s = tod % 60;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", y, mo, d, h, m, s, ms)
}

fn days_to_ymd(mut days: i64) -> (i32, u32, u32) {
    // Howard Hinnant's civil_from_days.
    days += 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = (days - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

#[allow(dead_code)]
pub fn log_path_for_install(install_dir: &Path) -> PathBuf {
    install_dir.join("install.log")
}

/// Installer log in `%TEMP%`, named by product (so support can tell which app
/// failed) + PID (uniqueness across concurrent runs). Used as the *live* log
/// target so diagnostics survive even when the chosen install dir isn't
/// writable. Copied into the install dir on success.
#[allow(dead_code)]
pub fn log_path_installer_temp(product: &str, pid: u32) -> PathBuf {
    let name = crate::paths::sanitize_component(product);
    std::env::temp_dir().join(format!("{}-install-{}.log", name, pid))
}

#[allow(dead_code)]
pub fn log_path_for_uninstall(install_dir: &Path) -> PathBuf {
    install_dir.join("uninstall.log")
}

#[allow(dead_code)]
pub fn log_path_for_stage2(product: &str, pid: u32) -> PathBuf {
    let name = crate::paths::sanitize_component(product);
    std::env::temp_dir().join(format!("{}-uninstall-{}.log", name, pid))
}
