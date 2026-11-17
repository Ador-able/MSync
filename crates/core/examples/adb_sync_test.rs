use anyhow::{ensure, Context, Result};
use msync_core::{
    adb::{self, Adb, Binding, Unavailable},
    log::Log,
    snapshot::{self, BACKUP},
    sync::Coordinator,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

fn run(co: &mut Coordinator, copies: usize, deletions: usize) -> Result<String> {
    let p = co.prepare()?;
    ensure!(
        p.conflicts.is_empty() && p.copies == copies && p.deletion_count == deletions,
        "同步计划不符：传输={}，删除={}",
        p.copies,
        p.deletion_count
    );
    let updates = std::cell::RefCell::new(Vec::new());
    let generation = co
        .execute(&p.token, BTreeMap::new(), |done, total| {
            println!("进度：{done}/{total}");
            updates.borrow_mut().push((done, total));
        })?
        .generation;
    let updates = updates.into_inner();
    let total = updates[0].1;
    ensure!(
        updates.first() == Some(&(0, total)) && updates.last() == Some(&(total, total)),
        "进度没有覆盖本轮任务"
    );
    ensure!(
        updates
            .iter()
            .enumerate()
            .all(|(index, &(done, count))| done == index && count == total),
        "进度跳变或总数不稳定"
    );
    Ok(generation)
}

fn verify_ignore_rules(
    co: &mut Coordinator,
    device: &Adb,
    local: &Path,
    remote: &str,
) -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let remote_dir = windows_path(remote);
    let pair = co.store.pair()?.unwrap();
    let mut old_settings = serde_json::to_value(&pair)?;
    old_settings
        .as_object_mut()
        .unwrap()
        .remove("ignore_patterns");
    let upgraded: msync_core::store::Pair = serde_json::from_value(old_settings)?;
    ensure!(
        upgraded.ignore_patterns == [r"^~\$"],
        "旧绑定未采用默认规则"
    );
    let local_temp = local.join("子目录/~$本地.doc");
    std::fs::write(&local_temp, "正在编辑")?;
    std::fs::write(remote_dir.join("~$远端.doc"), "平板临时资料")?;
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&local_temp)?;
    run(co, 2, 0)?;
    ensure!(
        co.store.failures(&pair.id)?.is_empty(),
        "被占用的忽略项仍产生失败"
    );
    ensure!(
        !remote_dir.join("子目录/~$本地.doc").exists() && !local.join("~$远端.doc").exists(),
        "默认忽略项被传输"
    );
    drop(held);
    co.set_ignore_patterns(vec![])?;
    run(co, 2, 0)?;
    ensure!(
        std::fs::read_to_string(local.join("~$远端.doc"))? == "平板临时资料",
        "移除规则后未重新纳入同步"
    );
    std::fs::remove_file(remote_dir.join("~$远端.doc"))?;
    co.set_ignore_patterns(vec![r"^~\$".into()])?;
    run(co, 0, 0)?;
    ensure!(
        local.join("~$远端.doc").is_file() && !remote_dir.join("~$远端.doc").exists(),
        "新增规则触发了删除或补传"
    );
    ensure!(
        !co.store.baseline(&pair.id)?.unwrap().snapshots[0].contains_key("~$远端.doc"),
        "忽略项残留在删除基线"
    );

    std::fs::create_dir(local.join("留存目录"))?;
    std::fs::write(local.join("留存目录/普通.md"), "正常资料")?;
    run(co, 1, 0)?;
    std::fs::write(local.join("留存目录/~$保留.doc"), "本地保留")?;
    std::fs::remove_file(remote_dir.join("留存目录/普通.md"))?;
    std::fs::remove_dir(remote_dir.join("留存目录"))?;
    run(co, 0, 1)?;
    ensure!(
        co.store.failures(&pair.id)?.is_empty()
            && std::fs::read_to_string(local.join("留存目录/~$保留.doc"))? == "本地保留",
        "目录删除影响了忽略项"
    );

    co.set_ignore_patterns(vec![
        r"^~\$".into(),
        r"^\[草稿\]".into(),
        r"^引号' \$".into(),
    ])?;
    std::fs::write(local.join("[草稿]本地.md"), "仅本地")?;
    std::fs::write(remote_dir.join("引号' $远端.md"), "仅平板")?;
    std::fs::write(local.join("中间~$正常.md"), "正常文件")?;
    std::fs::create_dir(local.join("[草稿]目录"))?;
    std::fs::write(local.join("[草稿]目录/正常.md"), "目录不按文件忽略")?;
    run(co, 2, 0)?;
    ensure!(
        !remote_dir.join("[草稿]本地.md").exists() && !local.join("引号' $远端.md").exists(),
        "正则规则未生效"
    );
    ensure!(
        std::fs::read_to_string(remote_dir.join("[草稿]目录/正常.md"))? == "目录不按文件忽略",
        "错误忽略了目录"
    );
    let reopened = msync_core::store::Store::open(&co.dir)?;
    ensure!(
        reopened.pair()?.unwrap().ignore_patterns == [r"^~\$", r"^\[草稿\]", r"^引号' \$"],
        "忽略规则没有持久化"
    );
    ensure!(
        device.scan(remote)?.contains_key("引号' $远端.md"),
        "忽略修改了源文件"
    );
    println!("忽略验证通过：旧版默认、占用跳过、双端保留、规则增删、父目录保留、正则规则、重启持久化与处理进度");
    Ok(())
}
fn shell_path(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    format!("/{}/{}", s[..1].to_lowercase(), &s[3..])
}
fn windows_path(path: &str) -> PathBuf {
    PathBuf::from(format!("{}:/{}", &path[1..2], &path[3..]))
}
fn bash() -> PathBuf {
    PathBuf::from(
        std::env::var_os("MSYNC_TEST_BASH")
            .expect("离线集成测试需要设置 MSYNC_TEST_BASH 为 Git Bash 路径"),
    )
}

// 仅供离线验证：由真实子进程执行相同 shell 脚本，传输落到隔离目录。
// 不模拟 Android 驱动、授权弹窗或锁屏策略；这些必须在真机另行验收。
fn driver(args: &[String]) -> Result<()> {
    let mode = if let Some(path) = std::env::var_os("MSYNC_TEST_MODE_FILE") {
        std::fs::read_to_string(path)?.trim().to_owned()
    } else {
        std::env::var("MSYNC_TEST_MODE").unwrap_or_default()
    };
    if args.first().is_some_and(|s| s == "version") {
        println!("MSync 离线测试驱动（非 Android 真机）");
        return Ok(());
    }
    if mode == "hang" {
        std::thread::sleep(Duration::from_secs(60));
    }
    if args.first().is_some_and(|s| s == "devices") {
        println!("List of devices attached");
        if mode != "disconnected" {
            println!(
                "test-serial\t{} model:测试平板 transport_id:1",
                if mode == "unauthorized" {
                    "unauthorized"
                } else {
                    "device"
                }
            );
        }
        return Ok(());
    }
    ensure!(
        args.first().is_some_and(|s| s == "-s") && args.get(1).is_some_and(|s| s == "test-serial"),
        "测试命令未明确选择设备"
    );
    match args.get(2).map(String::as_str) {
        Some("shell") => {
            if mode == "storage-locked" {
                eprintln!("共享目录不可访问");
                std::process::exit(73);
            }
            let script = args.last().context("没有 shell 脚本")?;
            let script = match std::env::var("MSYNC_TEST_SHARED_ROOT") {
                Ok(root) => script.replace("/storage/emulated/0", &root),
                Err(_) => script.clone(),
            };
            if mode == "scan-failure" && script.contains("find ") {
                print!("81a4 1 1700000000\0/partial\0");
                eprintln!("读取中断");
                std::process::exit(1);
            }
            let lose_reply = fault("replace", &script)?;
            if lose_reply.as_deref() == Some("before") {
                anyhow::bail!("adb: error: connect failed: closed");
            }
            let status = Command::new(bash()).args(["-c", &script]).status()?;
            if status.success() && mode == "filename-errors" && script.contains("xargs -0") {
                let root = std::env::var("MSYNC_TEST_SCAN_ROOT")?;
                // 模拟 Android 可返回而 Windows 无法区分的路径，保留其余真实扫描记录。
                for name in ["CON.txt", "同名.md", "同名.MD"] {
                    print!("81a4 4 1700000000\0{root}/{name}\0{}\0", "a".repeat(64));
                }
            }
            if status.success() && lose_reply.as_deref() == Some("after") {
                anyhow::bail!("adb: error: connect failed: closed");
            }
            std::process::exit(status.code().unwrap_or(1));
        }
        Some("push") => {
            if fault("push", &args[4])?.is_some() {
                std::fs::write(windows_path(&args[4]), "传输中断留下的半个文件")?;
                anyhow::bail!("adb: error: connect failed: closed");
            }
            std::fs::copy(&args[3], windows_path(&args[4]))?;
        }
        Some("pull") => {
            // ADB 下载会先删除旧目标再创建，不能用原地覆盖掩盖 Windows 句柄占用。
            use std::os::windows::fs::OpenOptionsExt;
            unsafe {
                windows_sys::Win32::Storage::FileSystem::DeleteFileW(
                    msync_core::endpoint::wide(Path::new(&args[4])).as_ptr(),
                );
            }
            let mut target = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .share_mode(3)
                .open(&args[4])?;
            let mut source = std::fs::File::open(windows_path(&args[3]))?;
            std::io::copy(&mut source, &mut target)?;
        }
        other => anyhow::bail!("未知模拟命令：{other:?}"),
    }
    Ok(())
}

fn fault(operation: &str, target: &str) -> Result<Option<String>> {
    let Some(path) = std::env::var_os("MSYNC_TEST_FAULT_FILE") else {
        return Ok(None);
    };
    let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    let remaining = value["remaining"].as_u64().unwrap_or(0);
    if remaining == 0
        || value["operation"] != operation
        || !target.contains(value["path"].as_str().unwrap_or(""))
        || operation == "replace" && !target.contains("mv -f")
    {
        return Ok(None);
    }
    value["remaining"] = (remaining - 1).into();
    std::fs::write(path, serde_json::to_vec(&value)?)?;
    Ok(Some(value["when"].as_str().unwrap_or("before").into()))
}

fn check_meta_recovery(co: &mut Coordinator, local: &Path, remote: &Path) -> Result<()> {
    let pair = co.store.pair()?.context("缺少测试绑定")?;
    for missing_side in ["电脑", "平板", "两边", "损坏", "不匹配", "磁盘变化"] {
        let local_only = format!("{missing_side}-电脑独有.md");
        let remote_only = format!("{missing_side}-平板独有.md");
        let conflict = format!("{missing_side}-同名.md");
        for path in [&local_only, &remote_only, &conflict] {
            std::fs::write(local.join(path), "原来两边都有")?;
        }
        run(co, 3, 0)?;
        std::fs::remove_file(remote.join(&local_only))?;
        std::fs::remove_file(local.join(&remote_only))?;
        std::fs::write(local.join(&conflict), "用户选择的电脑版")?;
        std::fs::write(remote.join(&conflict), "时间较新的平板版")?;
        filetime::set_file_mtime(
            local.join(&conflict),
            filetime::FileTime::from_unix_time(1700000000, 0),
        )?;
        filetime::set_file_mtime(
            remote.join(&conflict),
            filetime::FileTime::from_unix_time(1700000600, 0),
        )?;
        for (side, root) in [("电脑", local), ("平板", remote)] {
            if missing_side == side || missing_side == "两边" {
                let meta = root.join(snapshot::META).canonicalize()?;
                ensure!(
                    meta.parent() == Some(root.canonicalize()?.as_path()),
                    "测试清理路径越界"
                );
                std::fs::remove_dir_all(meta)?;
            }
            if missing_side == "损坏" || missing_side == "不匹配" {
                std::fs::write(
                    root.join(snapshot::META).join("endpoint.json"),
                    if missing_side == "损坏" {
                        "损坏的 JSON"
                    } else {
                        r#"{"pair":"其他绑定","endpoint":"other","role":"other"}"#
                    },
                )?;
            }
        }
        if missing_side == "磁盘变化" {
            let mut changed = co.store.pair()?.context("缺少绑定")?;
            changed.local.serial ^= 1;
            co.store.save_pair(&changed)?;
        }
        let preview = co.prepare()?;
        ensure!(
            preview.merge_recovery && preview.deletion_count == 0 && preview.conflicts.len() == 1,
            "标记丢失没有进入合并选版"
        );
        ensure!(preview.conflicts[0].path == conflict, "冲突文件不符");
        ensure!(
            std::fs::read_to_string(remote.join(&conflict))? == "时间较新的平板版",
            "确认前发生覆盖"
        );
        // 未选版时先处理其余文件；重启后继续合并，不能提前传播删除。
        if missing_side == "损坏" {
            co.execute(&preview.token, BTreeMap::new(), |_, _| {})?;
            ensure!(co.preview().is_some(), "冲突没有保留");
            ensure!(
                std::fs::read_to_string(remote.join(&local_only))? == "原来两边都有",
                "冲突阻断了无冲突文件"
            );
            ensure!(
                std::fs::read_to_string(local.join(&conflict))? == "用户选择的电脑版"
                    && std::fs::read_to_string(remote.join(&conflict))? == "时间较新的平板版",
                "未选版就修改了冲突文件"
            );
        }
        *co = Coordinator::open(co.dir.clone(), co.log.clone(), co.stop.clone())?;
        let preview = co.prepare()?;
        ensure!(
            preview.merge_recovery && preview.conflicts.len() == 1,
            "重启后丢失合并语义"
        );
        let fault_path = co.dir.join("合并故障.json");
        if missing_side == "两边" {
            std::fs::write(
                &fault_path,
                serde_json::to_vec(&serde_json::json!({
                    "operation":"replace", "path":format!("/{local_only}"), "remaining":3, "when":"before"
                }))?,
            )?;
            std::env::set_var("MSYNC_TEST_FAULT_FILE", &fault_path);
        }
        let generation = co
            .execute(
                &preview.token,
                BTreeMap::from([(conflict.clone(), 0)]),
                |_, _| {},
            )?
            .generation;
        if missing_side == "两边" {
            ensure!(
                !local.join(snapshot::META).join("endpoint.json").exists()
                    && !remote.join(snapshot::META).join("endpoint.json").exists(),
                "部分完成提前补全了标记"
            );
            ensure!(co.store.failures(&pair.id)?.len() == 1, "失败文件没有保留");
            std::env::remove_var("MSYNC_TEST_FAULT_FILE");
            *co = Coordinator::open(co.dir.clone(), co.log.clone(), co.stop.clone())?;
            let next = co.prepare()?;
            ensure!(
                next.merge_recovery
                    && next.copies == 1
                    && next.conflicts.is_empty()
                    && next.deletion_count == 0,
                "重启后没有只补合并失败的文件"
            );
            co.execute(&next.token, BTreeMap::new(), |_, _| {})?;
        }
        for root in [local, remote] {
            ensure!(
                root.join(snapshot::META).join("endpoint.json").is_file(),
                "合并完成没有补全标记"
            );
            ensure!(
                std::fs::read_to_string(root.join(&local_only))? == "原来两边都有",
                "合并错误传播了删除"
            );
            ensure!(
                std::fs::read_to_string(root.join(&remote_only))? == "原来两边都有",
                "合并未保留平板独有文件"
            );
            ensure!(
                std::fs::read_to_string(root.join(&conflict))? == "用户选择的电脑版",
                "未按用户选择处理冲突"
            );
        }
        ensure!(
            std::fs::read_to_string(remote.join(format!(
                "{BACKUP}/{}/{generation}/files/{conflict}",
                pair.id
            )))? == "时间较新的平板版",
            "未备份冲突另一版"
        );
        std::fs::write(remote.join(&conflict), "合并后的正常修改")?;
        ensure!(!co.prepare()?.merge_recovery, "标记补全后仍停留在合并模式");
        run(co, 1, 0)?;
        ensure!(
            std::fs::read_to_string(local.join(&conflict))? == "合并后的正常修改",
            "没有恢复正常增量同步"
        );
        std::fs::remove_file(local.join(&local_only))?;
        run(co, 0, 1)?;
        ensure!(
            !remote.join(&local_only).exists(),
            "合并后没有恢复原删除规则"
        );
        println!("{missing_side}恢复：独有文件保留、选版备份、补标记及后续增量同步通过");
    }
    Ok(())
}

fn check_directory_recovery(co: &mut Coordinator, local: &Path, remote: &Path) -> Result<()> {
    let expected = snapshot::scan(local)?;
    for (side, root) in [("电脑", local), ("平板", remote)] {
        let root = root.canonicalize()?;
        let saved = root.with_file_name(format!("{side}-移走后保留"));
        ensure!(
            saved.parent() == root.parent() && !saved.exists(),
            "测试移动路径不正确"
        );
        std::fs::rename(&root, &saved)?;
        let preview = co.prepare()?;
        ensure!(
            root.is_dir() && preview.merge_recovery && preview.deletion_count == 0,
            "目录没有在配置位置自动重建"
        );
        co.execute(&preview.token, BTreeMap::new(), |_, _| {})?;
        ensure!(
            snapshot::equal(&expected, &snapshot::scan(&root)?),
            "重建目录没有完整同步回来"
        );
        ensure!(
            snapshot::equal(&expected, &snapshot::scan(&saved)?),
            "移走的文件被改动"
        );
        ensure!(!co.prepare()?.merge_recovery, "恢复后没有回到增量同步");
        println!("{side}目录被移走：原位置重建、另一端补回、移走的文件保留通过");
    }
    // 备份和旧辅助标记都不参与绑定准入，新绑定也能直接采用现有目录。
    for root in [local, remote] {
        std::fs::create_dir_all(root.join(BACKUP))?;
        std::fs::write(root.join(BACKUP).join("历史备份.md"), "必须保留的旧备份")?;
        std::fs::write(root.join(BACKUP).join(".msync-owner"), "另一个绑定")?;
        std::fs::write(root.join(".usbsync-access-old"), "废弃标记")?;
    }
    let pair = co.store.pair()?.context("缺少绑定")?;
    co.bind(local, pair.remote)?;
    run(co, 0, 0)?;
    for root in [local, remote] {
        ensure!(
            std::fs::read_to_string(root.join(BACKUP).join("历史备份.md"))? == "必须保留的旧备份",
            "重新绑定改动了旧备份"
        );
        ensure!(
            !root.join(BACKUP).join(".msync-owner").exists()
                && !root.join(".usbsync-access-old").exists(),
            "旧版辅助标记未清理"
        );
    }
    std::fs::write(local.join("辅助标记丢失后.md"), "仍正常同步")?;
    run(co, 1, 0)?;
    ensure!(
        std::fs::read_to_string(remote.join("辅助标记丢失后.md"))? == "仍正常同步",
        "辅助标记丢失阻断同步"
    );
    use std::os::windows::fs::OpenOptionsExt;
    let busy_marker = local.join(".usbsync-access-busy");
    std::fs::write(&busy_marker, "正在占用")?;
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&busy_marker)?;
    std::fs::write(local.join("清理失败时.md"), "同步仍继续")?;
    run(co, 1, 0)?;
    ensure!(busy_marker.exists(), "占用文件不应已被删除");
    ensure!(
        std::fs::read_to_string(remote.join("清理失败时.md"))? == "同步仍继续",
        "清理失败阻断同步"
    );
    drop(held);
    co.cleanup_legacy_markers();
    ensure!(!busy_marker.exists(), "解除占用后没有补做清理");
    println!("已有目录直接绑定、旧备份保留、双端旧标记清理通过");
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args
        .first()
        .is_some_and(|s| s == "devices" || s == "-s" || s == "version")
    {
        return driver(&args);
    }
    let simulated = args.first().is_some_and(|s| s == "--simulated");
    ensure!(
        simulated || args.len() == 3 && args[0] == "--write-test",
        "用法：--simulated 或 --write-test <设备序列号> <平板测试父目录>"
    );
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/adb-sync")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&base)?;
    let base = base.canonicalize()?;
    // Git Bash 使用普通盘符路径，去掉 Windows 扩展长度前缀。
    let base = PathBuf::from(base.to_string_lossy().trim_start_matches("\\\\?\\"));
    let local = base.join("电脑");
    std::fs::create_dir_all(local.join("子目录"))?;
    let data = base.join("状态");
    let log = Log::new(&data.join("logs"))?;
    println!("验证记录：{}；模拟设备={simulated}", base.display());
    let stop = Arc::new(AtomicBool::new(false));
    let serial = if simulated {
        ensure!(bash().is_file(), "Git Bash 不存在");
        std::env::set_var("MSYNC_ADB_PATH", std::env::current_exe()?);
        std::env::remove_var("MSYNC_TEST_MODE");
        "test-serial".to_string()
    } else {
        args[1].clone()
    };
    let remote = if simulated {
        let dir = base.join("平板");
        std::fs::create_dir(&dir)?;
        shell_path(&dir)
    } else {
        adb::join(&args[2], &format!("MSync验证-{}", uuid::Uuid::new_v4()))?
    };
    let device = Adb::open(&serial, log.clone(), stop.clone())?;
    device.mkdir_all(&remote)?;
    let remote = device.canonical_root(&remote)?;
    println!("平板独立测试目录：{remote}");
    // 引号、空格、美元符号和中文同时覆盖远端 shell 引用与传输参数。
    let name = "中文 ' $笔记.md";
    std::fs::write(local.join(name), "电脑初版")?;
    std::fs::write(local.join("子目录").join(name), "子目录初版")?;
    let binding = Binding {
        serial: serial.clone(),
        path: remote.clone(),
        id: uuid::Uuid::new_v4().to_string(),
        label: "验证设备".into(),
        display: remote.clone(),
    };
    let mut co = Coordinator::open(data, log.clone(), stop.clone())?;
    let pair = co.bind(&local, binding)?;
    if simulated && args.iter().any(|arg| arg == "--ignore-rules") {
        return verify_ignore_rules(&mut co, &device, &local, &remote);
    }
    if simulated && args.iter().any(|arg| arg == "--filename-errors") {
        std::fs::write(local.join("同名.md"), "冲突路径保留原内容")?;
        std::fs::write(
            windows_path(&remote).join("正常远端.md"),
            "可正常读取的远端文件",
        )?;
        std::env::set_var("MSYNC_TEST_MODE", "filename-errors");
        std::env::set_var("MSYNC_TEST_SCAN_ROOT", &remote);
        let preview = co.prepare()?;
        co.execute(&preview.token, BTreeMap::new(), |_, _| {})?;
        let failures = co.store.failures(&pair.id)?;
        ensure!(
            failures.len() == 3 && failures.iter().any(|f| f.path == "CON.txt"),
            "未隔离文件名问题"
        );
        ensure!(
            std::fs::read_to_string(local.join("同名.md"))? == "冲突路径保留原内容",
            "大小写冲突覆盖了原文件"
        );
        ensure!(
            std::fs::read_to_string(windows_path(&remote).join(name))? == "电脑初版",
            "文件名问题阻断其他文件"
        );
        ensure!(
            !local.join(snapshot::META).join("endpoint.json").exists(),
            "未完成时提前结束了合并"
        );
        println!("Windows 保留名和大小写冲突仅暂缓对应路径，其余真实文件完成同步通过");
        return Ok(());
    }
    run(&mut co, 2, 0)?;
    let external = base.join("回读.md");
    device.pull(&adb::join(&remote, name)?, &external)?;
    ensure!(
        std::fs::read(&external)? == "电脑初版".as_bytes(),
        "上传后回读错误"
    );
    println!("首次合并、中文特殊字符及同名子目录文件通过");
    std::fs::write(local.join(name), "电脑新版")?;
    let generation = run(&mut co, 1, 0)?;
    device.pull(
        &adb::join(
            &remote,
            &format!("{BACKUP}/{}/{generation}/files/{name}", pair.id),
        )?,
        &external,
    )?;
    ensure!(
        std::fs::read(&external)? == "电脑初版".as_bytes(),
        "平板归档错误"
    );
    println!("电脑修改传播、目标替换和平板归档通过");
    let changed = adb::join(&remote, &format!("子目录/{name}"))?;
    device.delete(&changed, Some(&device.hash(&changed)?))?;
    std::fs::write(&external, "来自电脑 B 的新版")?;
    device.push(&external, &changed)?;
    let generation = run(&mut co, 1, 0)?;
    ensure!(
        std::fs::read(local.join("子目录").join(name))? == "来自电脑 B 的新版".as_bytes(),
        "平板修改未同步回电脑"
    );
    ensure!(
        std::fs::read(local.join(format!(
            "{BACKUP}/{}/{generation}/files/子目录/{name}",
            pair.id
        )))? == "子目录初版".as_bytes(),
        "电脑归档错误"
    );
    println!("平板外部修改传回电脑、电脑归档通过");
    // 无论是授权、离线、锁定还是读到一半失败，都必须保留先前成功基线。
    if simulated {
        let baseline = co.store.baseline(&pair.id)?.context("基线丢失")?.generation;
        for mode in [
            "disconnected",
            "unauthorized",
            "storage-locked",
            "scan-failure",
        ] {
            std::env::set_var("MSYNC_TEST_MODE", mode);
            let failure = co.prepare().err().context("读取失败却返回了同步计划")?;
            if mode != "scan-failure" {
                ensure!(
                    failure.downcast_ref::<Unavailable>().is_some(),
                    "没有明确的连接/存储状态"
                );
            }
            ensure!(
                co.store.baseline(&pair.id)?.context("基线丢失")?.generation == baseline,
                "失败覆盖了成功基线"
            );
            ensure!(
                std::fs::read(local.join(name))? == "电脑新版".as_bytes(),
                "读取失败删除了电脑文件"
            );
        }
        std::env::set_var("MSYNC_TEST_MODE", "hang");
        let cancel = stop.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            cancel.store(true, Ordering::Relaxed);
        });
        let now = Instant::now();
        ensure!(co.prepare().is_err(), "取消没有结束操作");
        worker.join().unwrap();
        ensure!(
            now.elapsed() < Duration::from_secs(5),
            "ADB 子进程取消未及时完成"
        );
        stop.store(false, Ordering::Relaxed);
        std::env::remove_var("MSYNC_TEST_MODE");
        println!("离线、未授权、存储不可访问、部分扫描失败与取消均保留电脑文件及成功基线");
    }
    if simulated {
        let fault_path = base.join("故障.json");
        let inject = |operation: &str, path: &str, remaining: u64, when: &str| -> Result<()> {
            std::fs::write(
                &fault_path,
                serde_json::to_vec(
                    &serde_json::json!({"operation":operation,"path":path,"remaining":remaining,"when":when}),
                )?,
            )?;
            std::env::set_var("MSYNC_TEST_FAULT_FILE", &fault_path);
            Ok(())
        };
        std::fs::write(local.join("重试上传.md"), "重试后完整内容")?;
        inject("push", "transfers/", 1, "before")?;
        run(&mut co, 1, 0)?;
        ensure!(
            std::fs::read_to_string(windows_path(&remote).join("重试上传.md"))? == "重试后完整内容",
            "半传输重试没有恢复"
        );
        ensure!(
            co.store.failures(&pair.id)?.is_empty(),
            "自动重试成功却标记失败"
        );
        println!("传输中断留下暂存文件后，自动重试完整上传通过");

        std::fs::write(local.join("响应丢失.md"), "远端已经完成替换")?;
        inject("replace", "/响应丢失.md", 1, "after")?;
        run(&mut co, 1, 0)?;
        ensure!(
            co.store.failures(&pair.id)?.is_empty(),
            "已执行但响应丢失无法恢复"
        );
        ensure!(
            std::fs::read_to_string(windows_path(&remote).join("响应丢失.md"))?
                == "远端已经完成替换",
            "响应丢失后文件错误"
        );
        println!("替换已执行但响应丢失时，核验结果并恢复通过");

        let last_success = co.store.baseline(&pair.id)?.unwrap().completed;
        std::fs::write(local.join("a-失败.md"), "稍后补同步")?;
        std::fs::write(local.join("z-继续.md"), "不能被前一个失败阻断")?;
        inject("replace", "/a-失败.md", 3, "before")?;
        run(&mut co, 2, 0)?;
        let failures = co.store.failures(&pair.id)?;
        ensure!(
            failures.len() == 1
                && failures[0].path == "a-失败.md"
                && failures[0].reason.contains("3 次"),
            "未准确记录单文件失败"
        );
        ensure!(
            std::fs::read_to_string(windows_path(&remote).join("z-继续.md"))?
                == "不能被前一个失败阻断",
            "失败阻断了后续文件"
        );
        ensure!(
            co.store.baseline(&pair.id)?.unwrap().completed == last_success,
            "部分完成错误更新了整轮成功时间"
        );
        std::env::remove_var("MSYNC_TEST_FAULT_FILE");
        let state_dir = co.dir.clone();
        drop(co);
        co = Coordinator::open(state_dir, log.clone(), stop.clone())?;
        ensure!(co.store.failures(&pair.id)?.len() == 1, "重启丢失失败原因");
        run(&mut co, 1, 0)?;
        ensure!(
            co.store.failures(&pair.id)?.is_empty(),
            "成功补同步未清除失败"
        );
        println!("连续三次失败只跳过当前文件、保留原因，重启后只补同步失败文件通过");

        std::fs::write(local.join("边编辑边同步.md"), "预检时的版本")?;
        std::fs::write(local.join("不受编辑影响.md"), "正常同步")?;
        let preview = co.prepare()?;
        std::fs::write(local.join("边编辑边同步.md"), "用户在预检之后继续编辑")?;
        co.execute(&preview.token, BTreeMap::new(), |_, _| {})?;
        ensure!(
            co.store
                .failures(&pair.id)?
                .iter()
                .any(|f| f.path == "边编辑边同步.md"),
            "持续编辑文件未暂缓"
        );
        ensure!(
            std::fs::read_to_string(windows_path(&remote).join("不受编辑影响.md"))? == "正常同步",
            "编辑阻断了其他文件"
        );
        ensure!(
            !windows_path(&remote).join("边编辑边同步.md").exists(),
            "传输了过期版本"
        );
        run(&mut co, 1, 0)?;
        ensure!(
            std::fs::read_to_string(windows_path(&remote).join("边编辑边同步.md"))?
                == "用户在预检之后继续编辑",
            "未补同步新保存的内容"
        );
        println!("同步期间继续编辑，当前文件暂缓且下一轮自动采用新内容通过");

        // Windows 编辑器的独占句柄不得被当成删除，也不能阻断别的文件。
        use std::os::windows::fs::OpenOptionsExt;
        let held = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(local.join("边编辑边同步.md"))?;
        std::fs::write(local.join("锁定时新增.md"), "锁定期间其他文件正常")?;
        run(&mut co, 1, 0)?;
        ensure!(
            co.store
                .failures(&pair.id)?
                .iter()
                .any(|f| f.path == "边编辑边同步.md"),
            "扫描占用没有记录失败"
        );
        ensure!(
            windows_path(&remote).join("边编辑边同步.md").exists(),
            "占用被误判成删除"
        );
        ensure!(
            std::fs::read_to_string(windows_path(&remote).join("锁定时新增.md"))?
                == "锁定期间其他文件正常",
            "文件占用阻断其他文件"
        );
        drop(held);
        run(&mut co, 0, 0)?;
        ensure!(
            co.store.failures(&pair.id)?.is_empty(),
            "解除占用后失败没有恢复"
        );
        println!("扫描时文件独占不传播删除，其他文件继续，解除占用后自动恢复通过");
        check_meta_recovery(&mut co, &local, &windows_path(&remote))?;
        check_directory_recovery(&mut co, &local, &windows_path(&remote))?;
    }
    let pair = co.store.pair()?.context("缺少当前绑定")?;
    std::fs::remove_file(local.join(name))?;
    let generation = run(&mut co, 0, 1)?;
    ensure!(
        device.stat(&adb::join(&remote, name)?)?.is_none(),
        "删除未传播"
    );
    device.pull(
        &adb::join(
            &remote,
            &format!("{BACKUP}/{}/{generation}/files/{name}", pair.id),
        )?,
        &external,
    )?;
    ensure!(
        std::fs::read(&external)? == "电脑新版".as_bytes(),
        "删除归档错误"
    );
    ensure!(
        snapshot::equal(
            &co.store.baseline(&pair.id)?.context("缺少基线")?.snapshots[0],
            &device.scan(&remote)?
        ),
        "最终设备内容不同"
    );
    println!("删除传播、备份回读与最终完整摘要通过；验证目录已保留");
    Ok(())
}
