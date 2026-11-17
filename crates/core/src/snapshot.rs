use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    os::windows::fs::MetadataExt,
    path::Path,
    sync::atomic::AtomicBool,
};
use walkdir::WalkDir;

pub const BACKUP: &str = "_同步备份";
pub const META: &str = ".usbsync-meta";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Item {
    pub directory: bool,
    pub size: u64,
    pub modified: i64,
    pub hash: String,
}
pub type Snapshot = BTreeMap<String, Item>;
#[derive(Default)]
pub struct Scan {
    pub snapshot: Snapshot,
    pub failures: BTreeMap<String, String>,
    pub ignored: BTreeSet<String>,
}
#[derive(Default)]
pub struct IgnoreRules(Vec<regex::Regex>);
impl IgnoreRules {
    pub fn new(patterns: &[String]) -> Result<Self> {
        patterns
            .iter()
            .enumerate()
            .map(|(index, pattern)| {
                regex::Regex::new(pattern)
                    .with_context(|| format!("第 {} 行正则表达式无效", index + 1))
            })
            .collect::<Result<Vec<_>>>()
            .map(Self)
    }
    pub fn matches(&self, path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path);
        self.0.iter().any(|rule| rule.is_match(name))
    }
}
pub fn digest(path: &Path) -> Result<String> {
    let mut f = File::open(path).with_context(|| format!("无法读取 {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 128 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn reserved(path: &Path) -> bool {
    path.components().next().is_some_and(|p| {
        let s = p.as_os_str().to_string_lossy();
        s.eq_ignore_ascii_case(BACKUP)
            || s.eq_ignore_ascii_case(META)
            || s.to_ascii_lowercase().starts_with(".usbsync-access-")
    })
}
pub fn scan(root: &Path) -> Result<Snapshot> {
    let result = scan_inner(root, &IgnoreRules::default(), &mut |path| read_item(path))?;
    ensure!(
        result.failures.is_empty(),
        "文件读取失败：{:?}",
        result.failures
    );
    Ok(result.snapshot)
}

pub fn scan_for_sync(
    root: &Path,
    rules: &IgnoreRules,
    log: &crate::log::Log,
    stop: &AtomicBool,
) -> Result<Scan> {
    scan_inner(root, rules, &mut |path| {
        crate::retry::run(log, stop, &format!("读取 {}", path.display()), || {
            read_item(path)
        })
    })
}

pub fn read_item(path: &Path) -> Result<Option<Item>> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        meta.file_attributes() & 0x400 == 0,
        "不支持符号链接或 junction：{}",
        path.display()
    );
    ensure!(
        meta.file_attributes() & 0x4000 == 0,
        "不支持 EFS 加密文件：{}",
        path.display()
    );
    let directory = meta.is_dir();
    let hash = if directory {
        String::new()
    } else {
        digest(path)?
    };
    let after = std::fs::metadata(path)?;
    ensure!(
        directory || meta.len() == after.len() && meta.last_write_time() == after.last_write_time(),
        "文件正在保存，稍后自动重试：{}",
        path.display()
    );
    Ok(Some(Item {
        directory,
        size: if directory { 0 } else { meta.len() },
        modified: if directory {
            0
        } else {
            (meta.last_write_time() / 10000) as i64 - 11644473600000
        },
        hash,
    }))
}

fn scan_inner(
    root: &Path,
    rules: &IgnoreRules,
    read: &mut impl FnMut(&Path) -> Result<Option<Item>>,
) -> Result<Scan> {
    let mut result = Scan::default();
    let mut names = BTreeMap::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !reserved(e.path().strip_prefix(root).unwrap()))
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                let path = error.path().context("无法枚举本地目录")?;
                ensure!(path != root, "本地根目录不可访问：{error}");
                let relative = path
                    .strip_prefix(root)?
                    .to_str()
                    .context("文件名不是有效 Unicode")?
                    .replace('\\', "/");
                result.failures.insert(relative, error.to_string());
                continue;
            }
        };
        if entry.depth() == 0 {
            continue;
        }
        let path = entry.path();
        let relative = path
            .strip_prefix(root)?
            .to_str()
            .context("文件名不是有效 Unicode")?
            .replace('\\', "/");
        // 在打开文件前忽略，正在被办公软件占用的临时文件无需读取或重试。
        if !entry.file_type().is_dir() && rules.matches(&relative) {
            result.ignored.insert(relative);
            continue;
        }
        if let Err(error) = relative.split('/').try_for_each(valid_name) {
            result.failures.insert(relative, error.to_string());
            continue;
        }
        if let Some(other) = names.insert(relative.to_lowercase(), relative.clone()) {
            let reason = format!("文件名大小写冲突：{other} / {relative}");
            result.failures.insert(other, reason.clone());
            result.failures.insert(relative, reason);
            continue;
        }
        match read(path) {
            Ok(Some(item)) => {
                result.snapshot.insert(relative, item);
            }
            Ok(None) => {
                result
                    .failures
                    .insert(relative, "扫描期间文件被移动或删除，稍后重试".into());
            }
            Err(error) => {
                result.failures.insert(relative, format!("{error:#}"));
            }
        }
    }
    Ok(result)
}
pub fn same(a: Option<&Item>, b: Option<&Item>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.directory == b.directory && a.size == b.size && a.hash == b.hash,
        _ => false,
    }
}
pub fn equal(a: &Snapshot, b: &Snapshot) -> bool {
    a.len() == b.len() && a.iter().all(|(p, i)| same(Some(i), b.get(p)))
}

pub fn valid_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.ends_with([' ', '.'])
        || name.chars().any(|c| c < ' ' || "\\/:*?\"<>|".contains(c))
    {
        bail!("文件名不能在 Windows 上安全表示：{name}");
    }
    let stem = name.split('.').next().unwrap_or("").to_uppercase();
    anyhow::ensure!(
        ![
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9"
        ]
        .contains(&stem.as_str()),
        "Windows 保留文件名：{name}"
    );
    Ok(())
}
