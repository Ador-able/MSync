use anyhow::{ensure, Context, Result};
use chrono::{DateTime, Local};
use serde::Serialize;
use std::{
    collections::VecDeque,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

// 按十进制 MB 计量，启动器与主程序共用同一个目录预算。
const FILE_LIMIT: u64 = 10_000_000;
const TOTAL_LIMIT: u64 = 100_000_000;
const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Clone, Serialize)]
pub struct Entry {
    pub seq: u64,
    pub time: String,
    pub level: String,
    pub message: String,
}

struct Inner {
    entries: VecDeque<Entry>,
    seq: u64,
    part: u64,
}

#[derive(Clone)]
pub struct Log {
    inner: Arc<Mutex<Inner>>,
    pub path: PathBuf,
}

struct DiskLog {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

fn disk_logs(dir: &Path) -> Result<Vec<DiskLog>> {
    let mut logs = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|ext| ext == "log") && entry.file_type()?.is_file()
        {
            let meta = entry.metadata()?;
            logs.push(DiskLog {
                path: entry.path(),
                size: meta.len(),
                modified: meta.modified()?,
            });
        }
    }
    logs.sort_by(|a, b| a.modified.cmp(&b.modified).then(a.path.cmp(&b.path)));
    Ok(logs)
}

fn lock_directory(dir: &Path) -> Result<File> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(".writer.lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    Ok(lock)
}

fn prune(dir: &Path, now: SystemTime, incoming: u64, current: &Path) -> Result<()> {
    let logs = disk_logs(dir)?;
    let mut kept = Vec::new();
    let mut total = 0;
    for log in logs {
        if log.size > FILE_LIMIT
            || now.duration_since(log.modified).unwrap_or_default() >= RETENTION
        {
            std::fs::remove_file(&log.path)
                .with_context(|| format!("无法清理日志 {}", log.path.display()))?;
        } else {
            total += log.size;
            kept.push(log);
        }
    }
    for log in kept {
        if total + incoming <= TOTAL_LIMIT {
            break;
        }
        if log.path != current {
            std::fs::remove_file(&log.path)
                .with_context(|| format!("无法清理日志 {}", log.path.display()))?;
            total -= log.size;
        }
    }
    ensure!(
        total + incoming <= TOTAL_LIMIT,
        "日志空间不足，已跳过本次落盘"
    );
    Ok(())
}

impl Log {
    pub fn new(dir: &Path) -> Result<Self> {
        Self::named(dir, "msync")
    }

    pub fn launcher(dir: &Path) -> Result<Self> {
        Self::named(dir, "launcher")
    }

    fn named(dir: &Path, name: &str) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!(
            "{}-{}-{}-{}.log",
            name,
            Local::now().format("%Y%m%d-%H%M%S"),
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let log = Self {
            path,
            inner: Arc::new(Mutex::new(Inner {
                entries: VecDeque::new(),
                seq: 0,
                part: 0,
            })),
        };
        log.write("INFO", "日志保留：单文件 10 MB，总量 100 MB，最近 7 天");
        Ok(log)
    }

    pub fn write(&self, level: &str, message: impl AsRef<str>) {
        self.write_at(level, message.as_ref(), SystemTime::now());
    }

    fn write_at(&self, level: &str, message: &str, now: SystemTime) {
        let mut inner = self.inner.lock().unwrap();
        inner.seq += 1;
        let time = DateTime::<Local>::from(now)
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string();
        let prefix = format!("{time} [{level}] ");
        let suffix = "…（单条日志过长，已截断）";
        let mut message = message.to_owned();
        if prefix.len() + message.len() + 1 > FILE_LIMIT as usize {
            let mut end = (FILE_LIMIT as usize).saturating_sub(prefix.len() + suffix.len() + 1);
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
            message.push_str(suffix);
        }
        let entry = Entry {
            seq: inner.seq,
            time,
            level: level.to_owned(),
            message,
        };
        let line = format!("{prefix}{}\n", entry.message);
        let result = self.append(&mut inner, line.as_bytes(), now);
        inner.entries.push_back(entry);
        if let Err(error) = result {
            let message = format!("日志写入失败：{error:#}");
            eprintln!("{message}");
            inner.seq += 1;
            let seq = inner.seq;
            inner.entries.push_back(Entry {
                seq,
                time: Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
                level: "ERROR".into(),
                message,
            });
        }
        while inner.entries.len() > 2000 {
            inner.entries.pop_front();
        }
    }

    fn append(&self, inner: &mut Inner, line: &[u8], now: SystemTime) -> Result<()> {
        ensure!(line.len() as u64 <= FILE_LIMIT, "单条日志超出大小限制");
        let dir = self.path.parent().unwrap();
        // 文件只在持有目录锁期间打开，另一进程清理后不会继续向已删除文件写入。
        let _lock = lock_directory(dir)?;
        prune(dir, now, line.len() as u64, &self.path)?;
        if let Ok(meta) = std::fs::metadata(&self.path) {
            let previous_day = DateTime::<Local>::from(meta.modified()?).date_naive();
            let today = DateTime::<Local>::from(now).date_naive();
            if meta.len() + line.len() as u64 > FILE_LIMIT || previous_day != today {
                let stem = self.path.file_stem().unwrap().to_string_lossy();
                let archive = dir.join(format!("{stem}-{:06}.log", inner.part));
                std::fs::rename(&self.path, archive)?;
                inner.part += 1;
            }
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line)?;
        file.flush()?;
        Ok(())
    }

    pub fn entries(&self) -> Vec<Entry> {
        self.inner.lock().unwrap().entries.iter().cloned().collect()
    }

    pub fn text(&self) -> Result<String> {
        let _inner = self.inner.lock().unwrap();
        let dir = self.path.parent().unwrap();
        let _lock = lock_directory(dir)?;
        let prefix = format!("{}-", self.path.file_stem().unwrap().to_string_lossy());
        let mut parts: Vec<_> = disk_logs(dir)?
            .into_iter()
            .filter(|log| {
                log.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&prefix)
            })
            .map(|log| log.path)
            .collect();
        parts.sort();
        if self.path.exists() {
            parts.push(self.path.clone());
        }
        let mut text = String::new();
        for part in parts {
            text.push_str(&std::fs::read_to_string(part)?);
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(dir: &Path, name: &str, size: u64, modified: SystemTime) -> PathBuf {
        let path = dir.join(name);
        File::create(&path).unwrap().set_len(size).unwrap();
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(modified)).unwrap();
        path
    }

    fn assert_limits(dir: &Path) {
        let logs = disk_logs(dir).unwrap();
        assert!(logs.iter().all(|log| log.size <= FILE_LIMIT));
        assert!(logs.iter().map(|log| log.size).sum::<u64>() <= TOTAL_LIMIT);
    }

    #[test]
    fn rotates_and_exports_retained_parts_without_breaking_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path()).unwrap();
        log.write("INFO", "轮转前");
        log.write("INFO", "汉".repeat(FILE_LIMIT as usize / 3 + 1));
        log.write("ERROR", "轮转后");
        let text = log.text().unwrap();
        assert!(text.find("轮转前").unwrap() < text.find("已截断").unwrap());
        assert!(text.find("已截断").unwrap() < text.find("轮转后").unwrap());
        assert_eq!(disk_logs(dir.path()).unwrap().len(), 3);
        assert_limits(dir.path());
    }

    #[test]
    fn startup_prunes_legacy_logs_by_age_size_and_combined_budget_only() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let expired = seed(dir.path(), "expired.log", 1, now - RETENTION);
        let oversized = seed(dir.path(), "launcher.log", FILE_LIMIT + 1, now);
        let unrelated = seed(dir.path(), "state.sqlite3", 1, now - RETENTION);
        std::fs::create_dir(dir.path().join("nested.log")).unwrap();
        for n in 0..10 {
            seed(
                dir.path(),
                &format!("old-{n}.log"),
                FILE_LIMIT,
                now - Duration::from_secs(20 - n),
            );
        }
        let _log = Log::launcher(dir.path()).unwrap();
        assert!(!expired.exists());
        assert!(!oversized.exists());
        assert!(!dir.path().join("old-0.log").exists());
        assert!(dir.path().join("old-1.log").exists());
        assert!(unrelated.exists());
        assert!(dir.path().join("nested.log").is_dir());
        assert_limits(dir.path());
    }

    #[test]
    fn running_process_rolls_daily_and_recovers_after_other_writer_prunes_it() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path()).unwrap();
        log.write("INFO", "昨日内容");
        let yesterday = SystemTime::now() - Duration::from_secs(24 * 60 * 60);
        filetime::set_file_mtime(&log.path, filetime::FileTime::from_system_time(yesterday))
            .unwrap();
        log.write("INFO", "今日内容");
        assert_eq!(disk_logs(dir.path()).unwrap().len(), 2);
        assert!(!std::fs::read_to_string(&log.path)
            .unwrap()
            .contains("昨日内容"));
        let expired = SystemTime::now() - RETENTION;
        for file in disk_logs(dir.path()).unwrap() {
            filetime::set_file_mtime(file.path, filetime::FileTime::from_system_time(expired))
                .unwrap();
        }
        let launcher = Log::launcher(dir.path()).unwrap();
        assert!(!log.path.exists());
        std::thread::scope(|s| {
            for writer in [&log, &launcher] {
                s.spawn(move || {
                    for n in 0..50 {
                        writer.write("INFO", format!("继续写入 {n}"));
                    }
                });
            }
        });
        assert!(!log.text().unwrap().contains("昨日内容"));
        assert!(log.text().unwrap().contains("继续写入 49"));
        assert!(launcher.text().unwrap().contains("继续写入 49"));
        assert_limits(dir.path());
    }

    #[test]
    fn disk_failure_is_reported_without_stopping_caller_and_next_write_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path()).unwrap();
        std::fs::remove_file(&log.path).unwrap();
        std::fs::create_dir(&log.path).unwrap();
        log.write("INFO", "落盘失败时继续");
        assert!(log.entries().iter().any(|entry| entry.level == "ERROR"));
        std::fs::remove_dir(&log.path).unwrap();
        log.write("INFO", "恢复落盘");
        assert!(log.text().unwrap().contains("恢复落盘"));
    }
}
