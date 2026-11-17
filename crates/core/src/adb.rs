use crate::{
    log::Log,
    snapshot::{self, Item, Snapshot, BACKUP, META},
};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    io::{Read, Write},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub const SHARED_ROOT: &str = "/storage/emulated/0";
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug)]
pub struct Unavailable {
    pub phase: &'static str,
    pub detail: String,
}
impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}：{}", self.phase, self.detail)
    }
}
impl std::error::Error for Unavailable {}
fn unavailable(phase: &'static str, detail: impl Into<String>) -> anyhow::Error {
    Unavailable {
        phase,
        detail: detail.into(),
    }
    .into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub serial: String,
    pub name: String,
    pub state: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub serial: String,
    pub path: String,
    pub id: String,
    pub label: String,
    pub display: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub path: String,
    pub name: String,
    pub directory: bool,
    pub size: u64,
    pub modified: i64,
}

// Shell 参数始终作为单引号字面量；文件名中的引号不能变成命令。
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
pub fn join(root: &str, relative: &str) -> Result<String> {
    for part in relative.split('/').filter(|p| !p.is_empty()) {
        snapshot::valid_name(part)?;
    }
    ensure!(!relative.starts_with('/'), "只能使用目录内的相对路径");
    Ok(if relative.is_empty() {
        root.into()
    } else {
        format!("{}/{relative}", root.trim_end_matches('/'))
    })
}
fn absolute(path: &str) -> Result<()> {
    ensure!(
        path.starts_with('/')
            && !path.contains(['\0', '\n', '\r'])
            && !path.split('/').any(|p| p == ".." || p == "."),
        "平板路径格式无效：{path}"
    );
    Ok(())
}
fn executable() -> Result<PathBuf> {
    let path = match std::env::var_os("MSYNC_ADB_PATH") {
        Some(p) => PathBuf::from(p),
        None => std::env::current_exe()?
            .parent()
            .context("程序目录不可用")?
            .join("adb/adb.exe"),
    };
    ensure!(
        path.is_file(),
        "ADB 程序缺失，请完整解压绿色包；开发环境请设置 MSYNC_ADB_PATH：{}",
        path.display()
    );
    Ok(path)
}

#[derive(Clone)]
struct Runner {
    exe: PathBuf,
    log: Log,
    stop: Arc<AtomicBool>,
}
impl Runner {
    fn run(
        &self,
        args: &[OsString],
        label: &str,
        timeout: Duration,
        input: Option<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        ensure!(!self.stop.load(Ordering::Relaxed), "用户已停止同步");
        let started = Instant::now();
        self.log.write("DEBUG", format!("ADB 开始：{label}"));
        let mut child = Command::new(&self.exe)
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x08000000)
            .spawn()
            .with_context(|| format!("无法启动 ADB：{}", self.exe.display()))?;
        let mut stdout = child.stdout.take().context("ADB 标准输出不可用")?;
        let mut stderr = child.stderr.take().context("ADB 错误输出不可用")?;
        // 路径清单通过标准输入传输，避免 Windows 命令行长度限制；写入与读取并行。
        let input = input.map(|bytes| {
            let mut stdin = child.stdin.take().unwrap();
            std::thread::spawn(move || stdin.write_all(&bytes))
        });
        // 同时排空两个管道，避免大目录或传输进度填满管道后互相等待。
        let out = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let err = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map(|_| bytes)
        });
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if !self.stop.load(Ordering::Relaxed) && started.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(25))
                }
                result => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break match result {
                        Err(error) => Err(anyhow::Error::from(error)),
                        _ if self.stop.load(Ordering::Relaxed) => {
                            Err(anyhow::anyhow!("用户已停止同步"))
                        }
                        _ => Err(anyhow::anyhow!("ADB 操作超时：{label}")),
                    };
                }
            }
        };
        let output = out
            .join()
            .map_err(|_| anyhow::anyhow!("ADB 输出读取线程失败"))??;
        let written = match input {
            Some(writer) => writer
                .join()
                .map_err(|_| anyhow::anyhow!("ADB 输入线程失败"))?,
            None => Ok(()),
        };
        let error = String::from_utf8_lossy(
            &err.join()
                .map_err(|_| anyhow::anyhow!("ADB 错误读取线程失败"))??,
        )
        .trim()
        .to_owned();
        let result = (|| {
            let status = status?;
            if !status.success() {
                let detail = format!("{label}；退出码={:?}；{error}", status.code());
                if error.contains("unauthorized") {
                    return Err(unavailable(
                        "等待授权",
                        format!("请解锁平板并允许这台电脑进行 USB 调试。{detail}"),
                    ));
                }
                if error.contains("offline")
                    || error.contains("connect failed: closed")
                    || error.contains("connection reset")
                    || error.contains("no devices")
                    || error.contains("not found") && error.contains("device")
                {
                    return Err(unavailable(
                        "等待设备",
                        format!("请连接平板并检查 USB 调试。{detail}"),
                    ));
                }
                if status.code() == Some(73) {
                    return Err(unavailable(
                        "存储暂不可访问",
                        format!("设备已连接，请解锁平板或确认目录访问权限。{detail}"),
                    ));
                }
                bail!("ADB 操作失败：{detail}");
            }
            ensure!(!self.stop.load(Ordering::Relaxed), "用户已停止同步");
            written.context("发送扫描路径失败")?;
            Ok(output)
        })();
        match &result {
            Ok(_) => self.log.write(
                "DEBUG",
                format!("ADB 完成：{label}，{} ms", started.elapsed().as_millis()),
            ),
            Err(e) => self.log.write(
                "ERROR",
                format!(
                    "ADB 中断：{label}，{} ms；{e:#}",
                    started.elapsed().as_millis()
                ),
            ),
        }
        result
    }
}
fn parse_devices(bytes: &[u8]) -> Result<Vec<DeviceInfo>> {
    let mut devices = vec![];
    for line in std::str::from_utf8(bytes)?
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with("List of devices"))
    {
        let mut fields = line.split_whitespace();
        let serial = fields.next().context("ADB 设备行缺少序列号")?;
        let state = fields.next().context("ADB 设备行缺少状态")?;
        let name = fields
            .find_map(|f| f.strip_prefix("model:"))
            .unwrap_or(serial)
            .replace('_', " ");
        devices.push(DeviceInfo {
            serial: serial.into(),
            name,
            state: state.into(),
        });
    }
    Ok(devices)
}
pub fn devices(log: Log, stop: Arc<AtomicBool>) -> Result<Vec<DeviceInfo>> {
    let runner = Runner {
        exe: executable()?,
        log,
        stop,
    };
    parse_devices(&runner.run(
        &["devices".into(), "-l".into()],
        "枚举设备及授权状态",
        CONTROL_TIMEOUT,
        None,
    )?)
}
pub fn version(log: Log) -> Result<String> {
    let runner = Runner {
        exe: executable()?,
        log,
        stop: Arc::new(AtomicBool::new(false)),
    };
    Ok(String::from_utf8(runner.run(
        &["version".into()],
        "读取 ADB 版本与路径",
        CONTROL_TIMEOUT,
        None,
    )?)?
    .trim()
    .to_owned())
}
pub struct Adb {
    runner: Runner,
    serial: String,
}
impl Adb {
    pub fn open(serial: &str, log: Log, stop: Arc<AtomicBool>) -> Result<Self> {
        let found = devices(log.clone(), stop.clone())?
            .into_iter()
            .find(|d| d.serial == serial)
            .ok_or_else(|| unavailable("等待设备", format!("请连接已绑定平板：{serial}")))?;
        match found.state.as_str() {
            "device" => (),
            "unauthorized" => {
                return Err(unavailable(
                    "等待授权",
                    "请解锁平板，勾选始终允许并确认 USB 调试授权",
                ))
            }
            state => {
                return Err(unavailable(
                    "等待设备",
                    format!("平板状态为 {state}，请检查 USB 调试连接"),
                ))
            }
        }
        Ok(Self {
            runner: Runner {
                exe: executable()?,
                log,
                stop,
            },
            serial: serial.into(),
        })
    }
    fn command(&self, args: &[OsString], label: &str, timeout: Duration) -> Result<Vec<u8>> {
        let mut selected = vec![OsString::from("-s"), self.serial.clone().into()];
        selected.extend_from_slice(args);
        self.runner.run(
            &selected,
            &format!("{label}；设备={}", self.serial),
            timeout,
            None,
        )
    }
    fn shell(&self, script: &str, label: &str, timeout: Duration) -> Result<Vec<u8>> {
        self.command(
            &["shell".into(), "-T".into(), script.into()],
            label,
            timeout,
        )
    }
    pub fn canonical_root(&self, path: &str) -> Result<String> {
        absolute(path)?;
        let output = self.shell(&format!("set -eu; p={}; [ -d \"$p\" ] && [ -r \"$p\" ] && [ -x \"$p\" ] || exit 73; cd \"$p\" || exit 73; pwd -P", quote(path)), &format!("检查目录访问：{path}"), CONTROL_TIMEOUT)?;
        let root = std::str::from_utf8(&output)?
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        absolute(&root)?;
        Ok(root)
    }
    pub fn bound_root(&self, binding: &Binding) -> Result<String> {
        ensure!(binding.serial == self.serial, "设备序列号不匹配");
        self.mkdir_all(&binding.path)?;
        self.canonical_root(&binding.path)
    }
    pub fn cleanup_legacy_markers(&self, root: &str) -> Result<()> {
        absolute(root)?;
        let script = format!(
            "set -eu; root={}; for p in \"$root\"/.usbsync-access-* \"$root\"/{BACKUP}/.msync-owner; do if [ -f \"$p\" ] && [ ! -L \"$p\" ]; then rm -- \"$p\"; fi; done",
            quote(root)
        );
        self.shell(&script, "清理旧版辅助标记", CONTROL_TIMEOUT)?;
        Ok(())
    }
    pub fn list(&self, root: &str) -> Result<Vec<Entry>> {
        self.canonical_root(root)?;
        let script = format!(
            "set -eu; find {} -mindepth 1 -maxdepth 1 -exec sh -c {} sh {{}} +",
            quote(root),
            quote(RECORD)
        );
        parse_records(
            &self.shell(&script, &format!("浏览目录：{root}"), CONTROL_TIMEOUT)?,
            false,
        )
        .map(|v| v.into_iter().map(|(e, _)| e).collect())
    }
    pub fn stat(&self, path: &str) -> Result<Option<Entry>> {
        absolute(path)?;
        // 先确认父目录可访问；权限失败不能解释为文件不存在。
        let parent = path.rsplit_once('/').context("路径无父目录")?.0;
        let parent = if parent.is_empty() { "/" } else { parent };
        let script = format!("set -eu; [ -d {0} ] && [ -r {0} ] && [ -x {0} ] || exit 73; p={1}; if [ ! -e \"$p\" ] && [ ! -L \"$p\" ]; then exit 0; fi; set -- \"$p\"; {RECORD}", quote(parent), quote(path));
        let entries = parse_records(
            &self.shell(&script, &format!("读取属性：{path}"), CONTROL_TIMEOUT)?,
            false,
        )?;
        ensure!(entries.len() <= 1, "文件属性返回数量错误");
        Ok(entries.into_iter().next().map(|(e, _)| e))
    }
    pub fn resolve(&self, root: &str, relative: &str) -> Result<Option<Entry>> {
        absolute(root)?;
        let mut paths = vec![root.to_owned()];
        for part in relative.split('/').filter(|p| !p.is_empty()) {
            paths.push(join(paths.last().unwrap(), part)?);
        }
        // 一次远端调用检查整条路径，仍逐层区分缺失、无权限和符号链接。
        let script = format!("set -eu; set -- {}; for p do [ ! -L \"$p\" ] || exit 74; if [ ! -e \"$p\" ]; then exit 0; fi; if [ -d \"$p\" ]; then [ -r \"$p\" ] && [ -x \"$p\" ] || exit 73; fi; done; set -- \"$p\"; {RECORD}", paths.iter().map(|p| quote(p)).collect::<Vec<_>>().join(" "));
        let entries = parse_records(
            &self.shell(
                &script,
                &format!("读取路径：{}", paths.last().unwrap()),
                CONTROL_TIMEOUT,
            )?,
            false,
        )?;
        Ok(entries.into_iter().next().map(|(entry, _)| entry))
    }
    pub fn scan(&self, root: &str) -> Result<Snapshot> {
        let result = self.scan_for_sync(root, &snapshot::IgnoreRules::default())?;
        ensure!(
            result.failures.is_empty(),
            "平板文件读取失败：{:?}",
            result.failures
        );
        Ok(result.snapshot)
    }
    pub fn scan_for_sync(
        &self,
        root: &str,
        rules: &snapshot::IgnoreRules,
    ) -> Result<snapshot::Scan> {
        self.canonical_root(root)?;
        // 先读文件名，在电脑端统一匹配正则；只对未忽略的路径计算内容摘要。
        let names = r#"for p do if [ -d "$p" ]; then t=d; else t=f; fi; printf '%s\000%s\000' "$t" "$p"; done"#;
        let script = format!("set -eu; find {} \\( -path {} -o -path {} -o -path {} \\) -prune -o -mindepth 1 -exec sh -c {} sh {{}} +", quote(root), quote(&join(root,META)?), quote(&join(root,BACKUP)?), quote(&format!("{root}/.usbsync-access-*")), quote(names));
        let names = self.shell(&script, &format!("枚举同步文件：{root}"), TRANSFER_TIMEOUT)?;
        ensure!(
            names.is_empty() || names.last() == Some(&0),
            "设备文件名响应不完整"
        );
        let fields: Vec<_> = if names.is_empty() {
            vec![]
        } else {
            names[..names.len() - 1].split(|b| *b == 0).collect()
        };
        ensure!(fields.len() % 2 == 0, "设备文件名响应字段缺失");
        let prefix = format!("{}/", root.trim_end_matches('/'));
        let mut paths = Vec::new();
        let mut ignored = std::collections::BTreeSet::new();
        for record in fields.chunks_exact(2) {
            ensure!(
                record[0] == b"d" || record[0] == b"f",
                "设备文件类型响应无效"
            );
            let path = std::str::from_utf8(record[1])?;
            let relative = path.strip_prefix(&prefix).context("设备返回目录外路径")?;
            if record[0] == b"f" && rules.matches(relative) {
                ignored.insert(relative.to_owned());
            } else {
                paths.extend_from_slice(record[1]);
                paths.push(0);
            }
        }
        let bytes = if paths.is_empty() {
            vec![]
        } else {
            let script = format!("xargs -0 sh -c {} sh", quote(SCAN_RECORD));
            self.runner.run(
                &[
                    "-s".into(),
                    self.serial.clone().into(),
                    "shell".into(),
                    "-T".into(),
                    script.into(),
                ],
                &format!("完整扫描及 SHA-256：{root}；设备={}", self.serial),
                TRANSFER_TIMEOUT,
                Some(paths),
            )?
        };
        ensure!(
            bytes.is_empty() || bytes.last() == Some(&0),
            "设备目录响应不完整"
        );
        let fields: Vec<_> = if bytes.is_empty() {
            vec![]
        } else {
            bytes[..bytes.len() - 1].split(|b| *b == 0).collect()
        };
        ensure!(fields.len() % 3 == 0, "设备目录响应字段缺失");
        let mut failures = std::collections::BTreeMap::new();
        let mut records = vec![];
        let prefix = format!("{}/", root.trim_end_matches('/'));
        for record in fields.chunks_exact(3) {
            if record[0] == b"!" {
                let path = std::str::from_utf8(record[1])?
                    .strip_prefix(&prefix)
                    .context("设备返回目录外路径")?;
                failures.insert(path.into(), std::str::from_utf8(record[2])?.into());
            } else {
                let mut bytes = vec![];
                for field in record {
                    bytes.extend_from_slice(field);
                    bytes.push(0);
                }
                records.extend(parse_records(&bytes, true)?);
            }
        }
        let mut result = Snapshot::new();
        let mut names = std::collections::BTreeMap::new();
        let prefix = format!("{}/", root.trim_end_matches('/'));
        for (entry, hash) in records {
            let path = entry
                .path
                .strip_prefix(&prefix)
                .context("设备返回目录外路径")?;
            if let Err(error) = path.split('/').try_for_each(snapshot::valid_name) {
                failures.insert(path.into(), error.to_string());
                continue;
            }
            if let Some(other) = names.insert(path.to_lowercase(), path.to_string()) {
                let reason = format!("文件名大小写冲突：{other} / {path}");
                failures.insert(other, reason.clone());
                failures.insert(path.into(), reason);
                continue;
            }
            result.insert(
                path.into(),
                Item {
                    directory: entry.directory,
                    size: entry.size,
                    modified: entry.modified,
                    hash,
                },
            );
        }
        Ok(snapshot::Scan {
            snapshot: result,
            failures,
            ignored,
        })
    }
    pub fn bytes(&self, path: &str) -> Result<Vec<u8>> {
        let entry = self.stat(path)?.context("标识文件不存在")?;
        ensure!(
            !entry.directory && entry.size < 1024 * 1024,
            "标识文件类型或大小不正确"
        );
        // shell 输出可能转换换行；标识与清单通过文件传输保持原始字节。
        let file = tempfile::NamedTempFile::new()?.into_temp_path();
        self.pull(path, &file)?;
        Ok(std::fs::read(&file)?)
    }
    pub fn hash(&self, path: &str) -> Result<String> {
        absolute(path)?;
        let script = format!("set -eu; set -- {}; {HASH_RECORD}", quote(path));
        let records = parse_records(
            &self.shell(&script, &format!("校验 SHA-256：{path}"), TRANSFER_TIMEOUT)?,
            true,
        )?;
        ensure!(
            records.len() == 1 && !records[0].0.directory,
            "待校验对象不是文件"
        );
        Ok(records[0].1.clone())
    }
    pub fn read_item(&self, root: &str, path: &str) -> Result<Option<Item>> {
        let Some(entry) = self.resolve(root, path)? else {
            return Ok(None);
        };
        let hash = if entry.directory {
            String::new()
        } else {
            self.hash(&entry.path)?
        };
        Ok(Some(Item {
            directory: entry.directory,
            size: entry.size,
            modified: entry.modified,
            hash,
        }))
    }
    pub fn archive(&self, source: &str, target: &str, hash: &str) -> Result<()> {
        absolute(source)?;
        absolute(target)?;
        let temp = format!("{target}.{}.tmp", uuid::Uuid::new_v4());
        // 旧版直接在平板内复制；校验后才发布归档，不经 USB 来回传输。
        let script = format!("set -eu; src={}; dst={}; tmp={}; mkdir -p \"$(dirname \"$dst\")\"; if [ ! -e \"$dst\" ]; then cp \"$src\" \"$tmp\"; h=$(sha256sum < \"$tmp\"); [ \"${{h%% *}}\" = {} ] || exit 74; mv \"$tmp\" \"$dst\"; fi; h=$(sha256sum < \"$dst\"); [ \"${{h%% *}}\" = {} ]", quote(source), quote(target), quote(&temp), quote(hash), quote(hash));
        self.shell(
            &script,
            &format!("保存平板旧版：{source}"),
            TRANSFER_TIMEOUT,
        )?;
        Ok(())
    }
    pub fn mkdir_all(&self, path: &str) -> Result<()> {
        absolute(path)?;
        self.shell(
            &format!("mkdir -p {}", quote(path)),
            &format!("创建目录：{path}"),
            CONTROL_TIMEOUT,
        )?;
        Ok(())
    }
    pub fn push(&self, local: &Path, remote: &str) -> Result<()> {
        ensure!(self.stat(remote)?.is_none(), "不覆盖已有暂存文件：{remote}");
        self.command(
            &["push".into(), local.as_os_str().into(), remote.into()],
            &format!("上传：{} → {remote}", local.display()),
            TRANSFER_TIMEOUT,
        )?;
        Ok(())
    }
    pub fn pull(&self, remote: &str, local: &Path) -> Result<()> {
        absolute(remote)?;
        self.command(
            &["pull".into(), remote.into(), local.as_os_str().into()],
            &format!("下载：{remote} → {}", local.display()),
            TRANSFER_TIMEOUT,
        )?;
        Ok(())
    }
    pub fn write_new(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        std::fs::write(file.path(), bytes)?;
        self.push(file.path(), path)?;
        ensure!(
            self.bytes(path)? == bytes,
            "标识或清单写入后校验失败：{path}"
        );
        Ok(())
    }
    pub fn replace(
        &self,
        staged: &str,
        target: &str,
        old_hash: Option<&str>,
        modified: i64,
    ) -> Result<()> {
        absolute(staged)?;
        absolute(target)?;
        let time = chrono::DateTime::from_timestamp_millis(modified)
            .context("文件修改时间无效")?
            .format("%Y%m%d%H%M.%S")
            .to_string();
        let check = match old_hash {
            Some(hash) => format!("[ -f \"$dst\" ] && [ ! -L \"$dst\" ] || exit 74; h=$(sha256sum < \"$dst\") || exit 74; [ \"${{h%% *}}\" = {} ] || exit 74;",quote(hash)),
            None => "[ ! -e \"$dst\" ] && [ ! -L \"$dst\" ] || exit 74;".into(),
        };
        // 暂存与目标必须在同一文件系统，mv 直接替换，避免先删掉旧文件。
        // 共享存储在替换后刷新目标属性，修改时间必须设置在最终路径上。
        let script = format!("set -eu; src={}; dst={}; [ \"$(stat -c %d \"$src\")\" = \"$(stat -c %d \"$(dirname \"$dst\")\")\" ]; {check} mv -f \"$src\" \"$dst\"; TZ=UTC touch -m -t {} \"$dst\"",quote(staged),quote(target),quote(&time));
        self.shell(
            &script,
            &format!("核对目标并替换：{target}"),
            TRANSFER_TIMEOUT,
        )?;
        Ok(())
    }
    pub fn delete(&self, path: &str, expected_hash: Option<&str>) -> Result<()> {
        absolute(path)?;
        let script = if let Some(hash) = expected_hash {
            format!("set -eu; p={}; [ -f \"$p\" ] && [ ! -L \"$p\" ]; h=$(sha256sum < \"$p\"); [ \"${{h%% *}}\" = {} ]; rm \"$p\"",quote(path),quote(hash))
        } else {
            format!("rmdir {}", quote(path))
        };
        self.shell(
            &script,
            &format!("删除已归档文件或空目录：{path}"),
            TRANSFER_TIMEOUT,
        )?;
        Ok(())
    }
    pub fn free_space(&self, root: &str) -> Result<u64> {
        let result = self.shell(
            &format!("stat -f -c '%a %S' {}", quote(root)),
            "读取平板剩余空间",
            CONTROL_TIMEOUT,
        )?;
        let values: Vec<_> = std::str::from_utf8(&result)?
            .split_whitespace()
            .map(str::parse::<u64>)
            .collect::<std::result::Result<_, _>>()?;
        ensure!(values.len() == 2, "剩余空间返回格式错误");
        values[0].checked_mul(values[1]).context("剩余空间溢出")
    }
}

// NUL 分隔属性和路径；不使用 ls 的显示格式解析中文、空格或引号文件名。
const SCAN_RECORD: &str = r#"set -eu
for p do
 ok=0
 for attempt in 1 2 3; do
   if [ ! -L "$p" ] && s=$(stat -c '%f %s %Y' "$p" 2>/dev/null); then
     h=''
     if [ -d "$p" ]; then ok=1; break; fi
     if [ -f "$p" ] && h=$(sha256sum < "$p" 2>/dev/null) && after=$(stat -c '%f %s %Y' "$p" 2>/dev/null) && [ "$s" = "$after" ]; then
       h=${h%% *}; ok=1; break
     fi
   fi
   [ "$attempt" = 3 ] || sleep "$attempt"
 done
 if [ "$ok" = 1 ]; then printf '%s\000%s\000%s\000' "$s" "$p" "$h"
 else printf '!\000%s\000%s\000' "$p" '读取三次仍失败：文件正在保存、被占用或类型不支持'; fi
done"#;
const RECORD: &str = r#"set -eu
for p do
 [ ! -L "$p" ] || { echo "不支持符号链接：$p" >&2; exit 74; }
 s=$(stat -c '%f %s %Y' "$p")
 printf '%s\000%s\000' "$s" "$p"
done"#;
const HASH_RECORD: &str = r#"set -eu
for p do
 [ ! -L "$p" ] || { echo "不支持符号链接：$p" >&2; exit 74; }
 s=$(stat -c '%f %s %Y' "$p")
 h=''
 if [ -f "$p" ]; then
   h=$(sha256sum < "$p"); h=${h%% *}
   after=$(stat -c '%f %s %Y' "$p")
   [ "$s" = "$after" ] || { echo "文件正在变化：$p；读取前=$s；读取后=$after" >&2; exit 74; }
 elif [ ! -d "$p" ]; then echo "不支持的文件类型：$p" >&2; exit 74
 fi
 printf '%s\000%s\000%s\000' "$s" "$p" "$h"
done"#;
fn parse_records(bytes: &[u8], with_hash: bool) -> Result<Vec<(Entry, String)>> {
    if bytes.is_empty() {
        return Ok(vec![]);
    }
    ensure!(bytes.last() == Some(&0), "设备目录响应不完整");
    let fields: Vec<_> = bytes[..bytes.len() - 1].split(|b| *b == 0).collect();
    let width = if with_hash { 3 } else { 2 };
    ensure!(fields.len() % width == 0, "设备目录响应字段缺失");
    let mut result = vec![];
    for record in fields.chunks_exact(width) {
        let metadata: Vec<_> = std::str::from_utf8(record[0])?.split_whitespace().collect();
        ensure!(metadata.len() == 3, "设备属性格式错误");
        let kind = u32::from_str_radix(metadata[0], 16)? & 0xf000;
        ensure!(kind == 0x4000 || kind == 0x8000, "不支持符号链接或特殊文件");
        let path = std::str::from_utf8(record[1])?.to_owned();
        absolute(&path)?;
        let directory = kind == 0x4000;
        let hash = if with_hash {
            std::str::from_utf8(record[2])?.to_owned()
        } else {
            String::new()
        };
        ensure!(
            !with_hash
                || directory
                || hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
            "文件摘要响应无效：{path}"
        );
        result.push((
            Entry {
                name: path.rsplit('/').next().context("文件名丢失")?.into(),
                path,
                directory,
                size: if directory { 0 } else { metadata[1].parse()? },
                modified: if directory {
                    0
                } else {
                    metadata[2]
                        .parse::<i64>()?
                        .checked_mul(1000)
                        .context("时间溢出")?
                },
            },
            hash,
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timed_out_child_is_reaped_and_logged() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let log = Log::new(dir.path())?;
        let runner = Runner {
            exe: PathBuf::from(std::env::var_os("WINDIR").context("WINDIR 缺失")?)
                .join("System32/WindowsPowerShell/v1.0/powershell.exe"),
            log: log.clone(),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let start = Instant::now();
        let result = runner.run(
            &[
                "-NoProfile".into(),
                "-Command".into(),
                "Start-Sleep -Seconds 10".into(),
            ],
            "超时回归",
            Duration::from_millis(150),
            None,
        );
        ensure!(
            result
                .err()
                .context("超时未失败")?
                .to_string()
                .contains("超时"),
            "未报告超时"
        );
        ensure!(start.elapsed() < Duration::from_secs(5), "子进程未退出");
        ensure!(log.text()?.contains("ADB 中断：超时回归"), "中断日志未落盘");
        Ok(())
    }
    #[test]
    fn directory_protocol_rejects_partial_output_and_preserves_names() {
        let bytes = b"81a4 12 1700000000\0/storage/test/a ' $(x).txt\0";
        let result = parse_records(bytes, false).unwrap();
        assert_eq!(result[0].0.name, "a ' $(x).txt");
        assert!(parse_records(&bytes[..bytes.len() - 1], false).is_err());
        assert!(parse_records(b"a1ff 1 1700000000\0/storage/link\0", false).is_err());
        assert!(join("/storage/test", "../outside").is_err());
    }
    #[test]
    fn device_states_remain_distinct() {
        let devices = parse_devices(b"List of devices attached\r\none\tdevice model:Xiaomi_Pad_7_Pro transport_id:1\r\ntwo\tunauthorized transport_id:2\r\nthree\toffline\r\n").unwrap();
        assert_eq!(devices[0].name, "Xiaomi Pad 7 Pro");
        assert_eq!(devices[1].state, "unauthorized");
        assert_eq!(devices[2].state, "offline");
    }
}
