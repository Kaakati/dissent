//! A log file that rolls over at 10 MB. Rolled files are deleted after 7 days.
//!
//! The current file is `<stem>.log`. A rolled file is `<stem>-<unix millis>.log`. The current
//! handle is closed before a rename, which Windows requires.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Size at which the current file rolls over.
pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
/// How long a rolled file is kept.
pub const KEEP_FOR: Duration = Duration::from_secs(7 * 24 * 3600);

struct Inner {
    dir: PathBuf,
    stem: String,
    max_bytes: u64,
    keep_for: Duration,
    file: Option<File>,
    written: u64,
}

/// A rolling log. Clones share one file, so it can serve as a tracing writer.
#[derive(Clone)]
pub struct RollingLog(Arc<Mutex<Inner>>);

impl RollingLog {
    /// Opens `<dir>/<stem>.log` with the standard limits.
    pub fn open(dir: &Path, stem: &str) -> io::Result<Self> {
        Self::with_limits(dir, stem, MAX_BYTES, KEEP_FOR)
    }

    pub fn with_limits(dir: &Path, stem: &str, max_bytes: u64, keep_for: Duration) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let mut inner = Inner {
            dir: dir.to_path_buf(),
            stem: stem.to_string(),
            max_bytes,
            keep_for,
            file: None,
            written: 0,
        };
        inner.reopen()?;
        inner.prune();
        Ok(Self(Arc::new(Mutex::new(inner))))
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Inner {
    fn current(&self) -> PathBuf {
        self.dir.join(format!("{}.log", self.stem))
    }

    fn reopen(&mut self) -> io::Result<()> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.current())?;
        self.written = file.metadata()?.len();
        self.file = Some(file);
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Drop the handle first. Windows will not rename a file that is open.
        self.file = None;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let rolled = self.dir.join(format!("{}-{stamp}.log", self.stem));
        match fs::rename(self.current(), &rolled) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                self.reopen()?;
                return Err(e);
            }
        }
        self.reopen()?;
        self.prune();
        Ok(())
    }

    /// Deletes rolled files older than `keep_for`. The current file is never touched.
    fn prune(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let prefix = format!("{}-", self.stem);
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.starts_with(&prefix) && name.ends_with(".log")) {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            let age = SystemTime::now().duration_since(modified).unwrap_or(Duration::ZERO);
            if age > self.keep_for {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

impl Write for RollingLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut inner = self.lock();
        if inner.written > 0 && inner.written + buf.len() as u64 > inner.max_bytes {
            inner.rotate()?;
        }
        if inner.file.is_none() {
            inner.reopen()?;
        }
        let n = match inner.file.as_mut() {
            Some(file) => file.write(buf)?,
            None => return Err(io::Error::other("log file is not open")),
        };
        inner.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.lock().file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dissent-logs-{name}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn rolled(dir: &Path, stem: &str) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(&format!("{stem}-")) && n.ends_with(".log"))
            .collect()
    }

    #[test]
    fn rolls_over_at_the_size_limit() {
        let dir = scratch("roll");
        let mut log = RollingLog::with_limits(&dir, "svc", 100, KEEP_FOR).unwrap();
        for _ in 0..5 {
            log.write_all(&[b'x'; 60]).unwrap();
        }
        log.flush().unwrap();
        let current = fs::metadata(dir.join("svc.log")).unwrap().len();
        assert!(current <= 100, "current file holds {current} bytes");
        assert!(!rolled(&dir, "svc").is_empty(), "a rolled file exists");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn never_writes_more_than_the_limit_to_one_file() {
        let dir = scratch("limit");
        let mut log = RollingLog::with_limits(&dir, "svc", 1000, KEEP_FOR).unwrap();
        for _ in 0..50 {
            log.write_all(&[b'y'; 150]).unwrap();
        }
        for name in rolled(&dir, "svc").iter().chain(std::iter::once(&"svc.log".to_string())) {
            let len = fs::metadata(dir.join(name)).unwrap().len();
            assert!(len <= 1000 + 150, "{name} is {len} bytes");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rolled_files_past_their_age_are_removed() {
        let dir = scratch("prune");
        fs::write(dir.join("svc-1.log"), b"old").unwrap();
        // Keeping for zero time drops every rolled file on the next prune.
        let _log = RollingLog::with_limits(&dir, "svc", MAX_BYTES, Duration::ZERO).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let mut log = RollingLog::with_limits(&dir, "svc", MAX_BYTES, Duration::ZERO).unwrap();
        log.write_all(b"fresh").unwrap();
        assert!(rolled(&dir, "svc").is_empty(), "stale rolled files remain");
        fs::remove_dir_all(&dir).unwrap();
    }
}
