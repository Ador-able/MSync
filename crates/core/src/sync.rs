use crate::{
    adb::{self, Adb, Binding},
    endpoint::{self, Endpoint},
    log::Log,
    plan::{self, Action, Conflict, Plan},
    retry,
    snapshot::{self, Snapshot, BACKUP, META},
    store::{Baseline, FileFailure, Pair, Store},
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
};
use uuid::Uuid;

#[derive(Clone, Serialize)]
pub struct Preview {
    pub token: String,
    pub conflicts: Vec<Conflict>,
    pub deletion_count: usize,
    pub copies: usize,
    pub merge_recovery: bool,
}
struct Pending {
    preview: Preview,
    snapshots: [Snapshot; 2],
    previous: Option<Baseline>,
    failures: BTreeMap<String, String>,
}
struct Scanned {
    snapshots: [Snapshot; 2],
    failures: BTreeMap<String, String>,
    ignored: BTreeSet<String>,
}
struct Session<'a> {
    adb: &'a Adb,
    local: &'a Path,
    root: &'a str,
    archive_root: String,
    staging: tempfile::TempDir,
}
#[derive(Serialize, Deserialize, PartialEq)]
struct Marker {
    pair: String,
    endpoint: String,
    role: String,
}
pub struct Coordinator {
    pub store: Store,
    pub log: Log,
    pub dir: PathBuf,
    pub stop: Arc<AtomicBool>,
    pending: Option<Pending>,
}
impl Coordinator {
    pub fn open(dir: PathBuf, log: Log, stop: Arc<AtomicBool>) -> Result<Self> {
        Ok(Self {
            store: Store::open(&dir)?,
            log,
            dir,
            stop,
            pending: None,
        })
    }
    pub fn bind(&mut self, local: &Path, remote: Binding) -> Result<Pair> {
        self.log.write(
            "INFO",
            format!("开始绑定：电脑 {} ↔ {}", local.display(), remote.display),
        );
        let endpoint = Endpoint::inspect(local, "电脑")?;
        let local_canonical = local.canonicalize()?;
        ensure!(
            !self.dir.canonicalize()?.starts_with(&local_canonical),
            "不能把程序状态目录包含在同步范围内"
        );
        let adb = Adb::open(&remote.serial, self.log.clone(), self.stop.clone())?;
        adb.bound_root(&remote)?;
        let pair = Pair {
            id: Uuid::new_v4().to_string(),
            local: endpoint,
            remote,
            automatic: true,
            ignore_patterns: self
                .store
                .pair()?
                .map(|p| p.ignore_patterns)
                .unwrap_or_else(crate::store::default_ignore_patterns),
        };
        // 绑定只保存配置；已有文件和备份保留，身份标记等合并成功后再写入。
        self.store.save_pair(&pair)?;
        self.log.write(
            "INFO",
            format!(
                "绑定成功 pair={}；平板={}；目录={}",
                pair.id, pair.remote.serial, pair.remote.display
            ),
        );
        Ok(pair)
    }
    fn pair(&self) -> Result<Pair> {
        self.store.pair()?.context("尚未设置同步文件夹")
    }
    pub fn set_ignore_patterns(&mut self, patterns: Vec<String>) -> Result<()> {
        let mut pair = self.pair()?;
        let mut seen = BTreeSet::new();
        let patterns: Vec<_> = patterns
            .into_iter()
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty() && seen.insert(p.clone()))
            .collect();
        snapshot::IgnoreRules::new(&patterns)?;
        pair.ignore_patterns = patterns;
        self.store.save_pair(&pair)?;
        // 旧选版来自上一组规则，修改后必须重新扫描。
        self.pending = None;
        self.log.write(
            "INFO",
            format!("忽略文件正则规则：{:?}", pair.ignore_patterns),
        );
        Ok(())
    }
    pub fn cleanup_legacy_markers(&self) {
        let pair = match self.store.pair() {
            Ok(Some(pair)) => pair,
            Ok(None) => return,
            Err(error) => {
                self.log
                    .write("WARN", format!("旧版标记清理暂缓：{error:#}"));
                return;
            }
        };
        if let Err(error) = endpoint::cleanup_legacy_markers(&pair.local.root()) {
            self.log
                .write("WARN", format!("电脑旧版标记清理暂缓：{error:#}"));
        }
        let result = Adb::open(&pair.remote.serial, self.log.clone(), self.stop.clone())
            .and_then(|adb| adb.cleanup_legacy_markers(&pair.remote.path));
        if let Err(error) = result {
            self.log
                .write("WARN", format!("平板旧版标记清理暂缓：{error:#}"));
        }
    }
    fn validate(&self, pair: &Pair, adb: &Adb) -> Result<(PathBuf, String, bool)> {
        let (local, merge_local) = pair.local.validate(&pair.id)?;
        let root = adb.bound_root(&pair.remote)?;
        let marker = adb.resolve(&root, &format!("{META}/endpoint.json"))?;
        let merge_remote = if let Some(marker) = &marker {
            let actual = serde_json::from_slice::<Marker>(&adb.bytes(&marker.path)?);
            actual.map_or(true, |actual| {
                actual
                    != Marker {
                        pair: pair.id.clone(),
                        endpoint: pair.remote.id.clone(),
                        role: "平板".into(),
                    }
            })
        } else {
            true
        };
        Ok((local, root, merge_local || merge_remote))
    }
    fn restore_meta(&mut self, pair: &Pair, adb: &Adb, root: &str) -> Result<()> {
        adb.canonical_root(root)?;
        let path = adb::join(root, &format!("{META}/endpoint.json"))?;
        let previous = adb.read_item(root, &format!("{META}/endpoint.json"))?;
        adb.mkdir_all(&adb::join(root, META)?)?;
        let bytes = serde_json::to_vec_pretty(&Marker {
            pair: pair.id.clone(),
            endpoint: pair.remote.id.clone(),
            role: "平板".into(),
        })?;
        // 正式标记只在合并成功后替换，失败重启仍会继续合并。
        let staged = format!("{path}.{}.tmp", Uuid::new_v4());
        adb.write_new(&staged, &bytes)?;
        adb.replace(
            &staged,
            &path,
            previous.as_ref().map(|item| item.hash.as_str()),
            chrono::Utc::now().timestamp_millis(),
        )?;
        let mut restored = pair.clone();
        restored.local = pair.local.restore_meta(&pair.id)?;
        self.store.save_pair(&restored)?;
        self.log
            .write("INFO", "双向合并完成，已补全同步标记，后续恢复正常增量同步");
        Ok(())
    }
    fn snapshots(
        &self,
        adb: &Adb,
        local: &Path,
        root: &str,
        rules: &snapshot::IgnoreRules,
    ) -> Result<Scanned> {
        self.log
            .write("INFO", "预检：并行读取电脑与平板 SHA-256 内容摘要");
        let (a, b) = std::thread::scope(|scope| {
            let remote = scope.spawn(|| adb.scan_for_sync(root, rules));
            let local = snapshot::scan_for_sync(local, rules, &self.log, &self.stop);
            (local, remote.join().unwrap())
        });
        self.check_stop()?;
        let (a, b) = (a?, b?);
        let mut failures = a.failures;
        failures.extend(b.failures);
        for (path, item) in &a.snapshot {
            if b.snapshot
                .get(path)
                .is_some_and(|other| other.directory != item.directory)
            {
                failures.insert(path.clone(), "同一路径分别是文件和目录，请调整名称".into());
            }
        }
        self.log.write(
            "INFO",
            format!(
                "扫描完成：电脑 {} 项，平板 {} 项，暂缓 {} 项",
                a.snapshot.len(),
                b.snapshot.len(),
                failures.len()
            ),
        );
        let ignored = a.ignored.into_iter().chain(b.ignored).collect();
        Ok(Scanned {
            snapshots: [a.snapshot, b.snapshot],
            failures,
            ignored,
        })
    }
    pub fn prepare(&mut self) -> Result<Preview> {
        // 启动时离线或被占用的旧标记，在下次同步补做；清理不影响同步结果。
        self.cleanup_legacy_markers();
        self.pending = None;
        let pair = self.pair()?;
        let rules = snapshot::IgnoreRules::new(&pair.ignore_patterns)?;
        let (
            Scanned {
                snapshots,
                failures,
                ignored,
            },
            merge_recovery,
        ) = retry::run(&self.log, &self.stop, "检查同步目录", || {
            let adb = Adb::open(&pair.remote.serial, self.log.clone(), self.stop.clone())?;
            let (local, root, merge_recovery) = self.validate(&pair, &adb)?;
            let _lock = endpoint::lock(&local)?;
            let scanned = self.snapshots(&adb, &local, &root, &rules)?;
            Ok((scanned, merge_recovery))
        })?;
        // 普通中断沿用逐文件进度；标记缺失时按现有文件合并，暂不采用历史删除意图。
        let mut previous = self.store.baseline(&pair.id)?;
        if let Some(baseline) = &mut previous {
            let mut retained_dirs = BTreeSet::new();
            for path in &ignored {
                let mut parent = path.as_str();
                while let Some((dir, _)) = parent.rsplit_once('/') {
                    retained_dirs.insert(dir.to_owned());
                    parent = dir;
                }
            }
            // 忽略表示退出同步范围，不能把缺失的扫描项当成删除；其父目录也须保留。
            for old in &mut baseline.snapshots {
                old.retain(|path, item| {
                    if item.directory {
                        !retained_dirs.contains(path)
                    } else {
                        !rules.matches(path)
                    }
                });
            }
        }
        let mut usable = snapshots.clone();
        let mut previous_usable = previous
            .as_ref()
            .filter(|_| !merge_recovery)
            .map(|b| b.snapshots.clone());
        exclude_failed(&mut usable, &failures);
        if let Some(old) = &mut previous_usable {
            exclude_failed(old, &failures);
        }
        let plan = plan::build(&usable, previous_usable.as_ref(), 2000, &BTreeMap::new())?;
        let preview = Preview {
            token: Uuid::new_v4().to_string(),
            conflicts: plan.conflicts.clone(),
            deletion_count: plan
                .actions
                .values()
                .filter(|a| matches!(a, Action::Delete { .. }))
                .count(),
            copies: plan
                .actions
                .values()
                .filter(|a| matches!(a, Action::Copy { .. }))
                .count(),
            merge_recovery,
        };
        if merge_recovery {
            self.log.write(
                "INFO",
                "目录或同步标记变化：合并两边现有文件，不传播删除；同名不同内容等待选择",
            );
        }
        self.log.write(
            "INFO",
            format!(
                "预检完成：传输 {}，冲突 {}，删除 {}",
                preview.copies,
                plan.conflicts.len(),
                preview.deletion_count
            ),
        );
        self.pending = Some(Pending {
            preview: preview.clone(),
            snapshots,
            previous,
            failures,
        });
        Ok(preview)
    }
    pub fn preview(&self) -> Option<Preview> {
        self.pending.as_ref().map(|p| p.preview.clone())
    }
    pub fn execute(
        &mut self,
        token: &str,
        choices: BTreeMap<String, usize>,
        progress: impl Fn(usize, usize),
    ) -> Result<Baseline> {
        ensure!(
            self.pending
                .as_ref()
                .is_some_and(|p| p.preview.token == token),
            "文件状态已更新，请重新选择版本"
        );
        let pending = self.pending.take().unwrap();
        let pair = self.pair()?;
        let attempt = format!(
            "{}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
            Uuid::new_v4()
        );
        let (adb, local, root) = retry::run(&self.log, &self.stop, "连接同步目录", || {
            let adb = Adb::open(&pair.remote.serial, self.log.clone(), self.stop.clone())?;
            let (local, root, missing_meta) = self.validate(&pair, &adb)?;
            ensure!(
                !missing_meta || pending.preview.merge_recovery,
                "同步标记在预检后丢失，将重新检查并合并文件"
            );
            Ok((adb, local, root))
        })?;
        let _lock = endpoint::lock(&local)?;
        let mut snapshots = pending.snapshots;
        let mut current = snapshots.clone();
        let merge_recovery = pending.preview.merge_recovery;
        let mut previous = pending
            .previous
            .as_ref()
            .filter(|_| !merge_recovery)
            .map(|b| b.snapshots.clone());
        let mut failures = pending.failures;
        exclude_failed(&mut current, &failures);
        if let Some(old) = &mut previous {
            exclude_failed(old, &failures);
        }
        let plan = plan::build(&current, previous.as_ref(), 2000, &choices)?;
        let conflicts: BTreeSet<_> = plan.conflicts.iter().map(|c| c.path.clone()).collect();
        let session = Session {
            adb: &adb,
            local: &local,
            root: &root,
            archive_root: format!("{BACKUP}/{}/{attempt}", pair.id),
            staging: tempfile::tempdir_in(&self.dir)?,
        };
        let mut baseline = pending.previous.unwrap_or(Baseline {
            generation: attempt.clone(),
            snapshots: [Snapshot::new(), Snapshot::new()],
            completed: String::new(),
        });
        baseline.generation = attempt.clone();
        let expected = expected(&current, previous.as_ref(), &plan);
        let paths: BTreeSet<_> = current
            .iter()
            .chain(baseline.snapshots.iter())
            .flat_map(|s| s.keys().cloned())
            .filter(|p| !blocked(p, &failures) && !conflicts.contains(p))
            .collect();
        let mut changed = BTreeSet::new();
        for path in paths {
            if snapshot::same(current[0].get(&path), current[1].get(&path)) {
                update_path(
                    &mut baseline.snapshots,
                    &path,
                    [current[0].get(&path), current[1].get(&path)],
                );
            } else {
                changed.insert(path);
            }
        }
        // 逐文件核验防止长时间同步覆盖后来保存的内容。
        self.store.checkpoint(&pair.id, &baseline)?;
        self.log.write(
            "INFO",
            format!(
                "任务开始 pair={} attempt={attempt}，待处理 {} 项",
                pair.id,
                changed.len()
            ),
        );
        let mut revalidate = false;
        let total = changed.len();
        let mut completed = 0;
        progress(completed, total);
        for (path, action) in &plan.actions {
            self.check_stop()?;
            let mut tries = 0;
            let result = retry::run(&self.log, &self.stop, path, || {
                tries += 1;
                if revalidate || tries > 1 {
                    let (_, _, missing_meta) = self.validate(&pair, &adb)?;
                    ensure!(
                        !missing_meta || merge_recovery,
                        "同步标记丢失，将在下一轮合并文件"
                    );
                }
                self.apply_file(&session, path, &current, *action)
            });
            self.check_stop()?;
            match result {
                Ok(after) => {
                    revalidate = false;
                    update_path(&mut snapshots, path, [after[0].as_ref(), after[1].as_ref()]);
                    update_path(
                        &mut baseline.snapshots,
                        path,
                        [after[0].as_ref(), after[1].as_ref()],
                    );
                    self.store.checkpoint_path(&pair.id, path, &after)?;
                    self.log.write("INFO", format!("文件同步完成：{path}"));
                }
                Err(error) => {
                    revalidate = true;
                    let reason = format!("已尝试 {} 次：{error:#}", retry::ATTEMPTS);
                    self.log
                        .write("ERROR", format!("文件失败，继续其余文件：{path}；{reason}"));
                    failures.insert(path.clone(), reason);
                }
            }
            completed += 1;
            progress(completed, total);
        }
        // 目录只在确有变化时操作；从深到浅删除空目录，用户新增文件会让删除自然失败。
        for path in changed
            .iter()
            .rev()
            .filter(|p| !plan.actions.contains_key(*p))
        {
            self.check_stop()?;
            let result = retry::run(&self.log, &self.stop, path, || {
                if expected.contains_key(path) {
                    for (side, snapshot) in current.iter().enumerate() {
                        if !snapshot.contains_key(path) {
                            if side == 0 {
                                std::fs::create_dir_all(local.join(path))?;
                            } else {
                                adb.mkdir_all(&adb::join(&root, path)?)?;
                            }
                        }
                    }
                } else {
                    if local.join(path).is_dir() {
                        std::fs::remove_dir(local.join(path))?;
                    }
                    if let Some(entry) = adb.resolve(&root, path)? {
                        adb.delete(&entry.path, None)?;
                    }
                }
                let after = [
                    snapshot::read_item(&local.join(path))?,
                    adb.read_item(&root, path)?,
                ];
                ensure!(
                    snapshot::same(after[0].as_ref(), after[1].as_ref())
                        && snapshot::same(after[0].as_ref(), expected.get(path)),
                    "目录在同步期间变化：{path}"
                );
                Ok(after)
            });
            self.check_stop()?;
            match result {
                Ok(after) => {
                    update_path(&mut snapshots, path, [after[0].as_ref(), after[1].as_ref()]);
                    update_path(
                        &mut baseline.snapshots,
                        path,
                        [after[0].as_ref(), after[1].as_ref()],
                    );
                    self.store.checkpoint_path(&pair.id, path, &after)?;
                }
                Err(error) => {
                    failures.insert(
                        path.clone(),
                        format!("已尝试 {} 次：{error:#}", retry::ATTEMPTS),
                    );
                }
            }
            completed += 1;
            progress(completed, total);
        }
        self.check_stop()?;
        let file_failures: Vec<_> = failures
            .iter()
            .map(|(path, reason)| FileFailure {
                path: path.clone(),
                reason: reason.clone(),
            })
            .collect();
        if failures.is_empty() && conflicts.is_empty() {
            // 文件全部处理后再更新标记，失败或待选版时继续合并。
            if merge_recovery {
                self.restore_meta(&pair, &adb, &root)?;
            }
            baseline.completed = chrono::Utc::now().to_rfc3339();
        }
        self.store.finish(&pair.id, &baseline, &file_failures)?;
        if !conflicts.is_empty() {
            // 未选版的文件保留原快照，其他成功路径更新后不再重复传输。
            self.pending = Some(Pending {
                preview: Preview {
                    conflicts: plan.conflicts,
                    copies: 0,
                    deletion_count: 0,
                    ..pending.preview
                },
                snapshots,
                previous: Some(baseline.clone()),
                failures,
            });
        }
        self.log.write(
            "INFO",
            format!(
                "本轮完成：失败 {} 项，等待选版 {} 项；已保存逐文件进度",
                file_failures.len(),
                conflicts.len()
            ),
        );
        Ok(baseline)
    }
    fn apply_file(
        &self,
        session: &Session<'_>,
        path: &str,
        current: &[Snapshot; 2],
        action: Action,
    ) -> Result<[Option<snapshot::Item>; 2]> {
        let Session {
            adb, local, root, ..
        } = session;
        let read = || -> Result<_> {
            Ok([
                snapshot::read_item(&local.join(path))?,
                adb.read_item(root, path)?,
            ])
        };
        let before = read()?;
        let from = match action {
            Action::Copy { from } => Some(from),
            Action::Delete { .. } => None,
        };
        let desired = from.map(|side| &current[side][path]);
        // ADB 可能已完成替换却丢失响应；重试先确认结果，不能重复覆盖或重复删除。
        if before.iter().all(|i| snapshot::same(i.as_ref(), desired)) {
            return Ok(before);
        }
        for side in 0..2 {
            ensure!(
                snapshot::same(before[side].as_ref(), current[side].get(path)),
                "文件在同步期间变化，将在下一轮重新检查：{path}"
            );
        }
        let staged = if let Some(from) = from {
            // ADB 会删除再创建下载目标；只保留清理路径，先释放 Windows 文件句柄。
            let stage = tempfile::NamedTempFile::new_in(session.staging.path())?.into_temp_path();
            if from == 0 {
                std::fs::copy(local.join(path), &stage)?;
            } else {
                adb.pull(&adb::join(root, path)?, &stage)?;
            }
            std::fs::OpenOptions::new()
                .write(true)
                .open(&stage)?
                .sync_all()?;
            ensure!(
                snapshot::digest(&stage)? == current[from][path].hash,
                "源文件正在保存：{path}"
            );
            Some(stage)
        } else {
            None
        };
        let archived_side = match action {
            Action::Copy { from } => 1 - from,
            Action::Delete { side } => side,
        };
        if let Some(item) = current[archived_side].get(path) {
            let archive_path = format!("{}/files/{path}", session.archive_root);
            if archived_side == 0 {
                let dest = local.join(&archive_path);
                if !dest.exists() {
                    std::fs::create_dir_all(dest.parent().unwrap())?;
                    let temp = tempfile::NamedTempFile::new_in(dest.parent().unwrap())?;
                    std::fs::copy(local.join(path), temp.path())?;
                    temp.as_file().sync_all()?;
                    ensure!(
                        snapshot::digest(temp.path())? == item.hash,
                        "归档期间文件变化：{path}"
                    );
                    temp.persist_noclobber(&dest)?;
                }
                ensure!(
                    snapshot::digest(&dest)? == item.hash,
                    "电脑归档校验失败：{path}"
                );
            } else {
                adb.archive(
                    &adb::join(root, path)?,
                    &adb::join(root, &archive_path)?,
                    &item.hash,
                )?;
            }
        }
        self.check_stop()?;
        if let Some(from) = from {
            let item = &current[from][path];
            let stage = staged.as_ref().unwrap();
            if from == 0 {
                let folder = adb::join(root, &format!("{META}/transfers"))?;
                adb.mkdir_all(&folder)?;
                let remote_stage = adb::join(&folder, &format!("{}.tmp", Uuid::new_v4()))?;
                adb.push(stage, &remote_stage)?;
                ensure!(
                    adb.hash(&remote_stage)? == item.hash,
                    "平板上传摘要校验失败：{path}"
                );
                adb.mkdir_all(&adb::join(root, split(path).0)?)?;
                ensure!(
                    snapshot::same(snapshot::read_item(&local.join(path))?.as_ref(), Some(item)),
                    "源文件正在保存：{path}"
                );
                adb.replace(
                    &remote_stage,
                    &adb::join(root, path)?,
                    current[1].get(path).map(|i| i.hash.as_str()),
                    item.modified,
                )?;
            } else {
                let dest = local.join(path);
                std::fs::create_dir_all(dest.parent().unwrap())?;
                let temp = tempfile::NamedTempFile::new_in(local.join(META))?;
                std::fs::copy(stage, temp.path())?;
                temp.as_file()
                    .set_times(std::fs::FileTimes::new().set_modified(
                        std::time::UNIX_EPOCH
                            + std::time::Duration::from_millis(item.modified.max(0) as u64),
                    ))?;
                temp.as_file().sync_all()?;
                let now = read()?;
                for side in 0..2 {
                    ensure!(
                        snapshot::same(now[side].as_ref(), current[side].get(path)),
                        "替换前文件变化：{path}"
                    );
                }
                let temp = temp.into_temp_path();
                unsafe {
                    let flags = windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH
                        | if current[0].contains_key(path) {
                            windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                        } else {
                            0
                        };
                    ensure!(
                        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                            endpoint::wide(&temp).as_ptr(),
                            endpoint::wide(&dest).as_ptr(),
                            flags
                        ) != 0,
                        "电脑文件替换失败：{}",
                        std::io::Error::last_os_error()
                    );
                }
            }
        } else if let Action::Delete { side } = action {
            let now = read()?;
            for side in 0..2 {
                ensure!(
                    snapshot::same(now[side].as_ref(), current[side].get(path)),
                    "删除前文件变化：{path}"
                );
            }
            if side == 0 {
                std::fs::remove_file(local.join(path))?;
            } else {
                adb.delete(&adb::join(root, path)?, Some(&current[side][path].hash))?;
            }
        }
        let after = read()?;
        ensure!(
            after.iter().all(|i| snapshot::same(i.as_ref(), desired)),
            "文件在同步后继续变化，下一轮重新检查：{path}"
        );
        Ok(after)
    }
    fn check_stop(&self) -> Result<()> {
        retry::check(&self.stop)
    }
}
fn blocked(path: &str, failures: &BTreeMap<String, String>) -> bool {
    failures
        .keys()
        .any(|p| path == p || path.starts_with(&format!("{p}/")))
}
fn exclude_failed(snapshots: &mut [Snapshot; 2], failures: &BTreeMap<String, String>) {
    for snapshot in snapshots {
        snapshot.retain(|path, _| !blocked(path, failures));
    }
}
fn update_path(snapshots: &mut [Snapshot; 2], path: &str, items: [Option<&snapshot::Item>; 2]) {
    for side in 0..2 {
        if let Some(item) = items[side] {
            snapshots[side].insert(path.into(), item.clone());
        } else {
            snapshots[side].remove(path);
        }
    }
}

fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}
fn expected(current: &[Snapshot; 2], previous: Option<&[Snapshot; 2]>, plan: &Plan) -> Snapshot {
    let mut out = current[0].clone();
    for (path, item) in &current[1] {
        out.entry(path.clone()).or_insert_with(|| item.clone());
    }
    for (path, action) in &plan.actions {
        match action {
            Action::Copy { from } => {
                out.insert(path.clone(), current[*from][path].clone());
            }
            Action::Delete { .. } => {
                out.remove(path);
            }
        }
    }
    if let Some(old) = previous {
        let dirs: Vec<_> = out
            .iter()
            .filter(|(_, i)| i.directory)
            .map(|(p, _)| p.clone())
            .collect();
        for path in dirs.iter().rev() {
            let deleted =
                (0..2).any(|i| old[i].contains_key(path) && !current[i].contains_key(path));
            if deleted && !out.keys().any(|p| p.starts_with(&format!("{path}/"))) {
                out.remove(path);
            }
        }
    }
    out
}
