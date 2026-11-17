use msync_core::{
    log::Log,
    snapshot::Snapshot,
    store::{Baseline, Store},
};
#[test]
fn legacy_cleanup_only_removes_generated_marker_files() {
    let dir = tempfile::tempdir().unwrap();
    for folder in ["_同步备份", ".usbsync-meta", "工作", ".usbsync-access-目录"] {
        std::fs::create_dir(dir.path().join(folder)).unwrap();
    }
    let removed = [
        ".usbsync-access-old",
        ".usbsync-access-中文 ' $",
        "_同步备份/.msync-owner",
    ];
    let retained = [
        "_同步备份/历史.md",
        ".usbsync-meta/endpoint.json",
        "工作/.usbsync-access-old",
        ".msync-owner",
        ".usbsync-access-目录/资料.md",
    ];
    for name in removed.iter().chain(&retained) {
        std::fs::write(dir.path().join(name), name).unwrap();
    }
    msync_core::endpoint::cleanup_legacy_markers(dir.path()).unwrap();
    msync_core::endpoint::cleanup_legacy_markers(dir.path()).unwrap();
    for name in removed {
        assert!(!dir.path().join(name).exists());
    }
    for name in retained {
        assert_eq!(
            std::fs::read_to_string(dir.path().join(name)).unwrap(),
            name
        );
    }
}

#[test]
fn diagnostic_log_is_on_disk_before_process_exit() {
    let dir = tempfile::tempdir().unwrap();
    let log = Log::new(dir.path()).unwrap();
    log.write("INFO", "任务开始 attempt=示例");
    log.write("ERROR", "ADB 设备离线；未提交成功状态");
    let text = std::fs::read_to_string(&log.path).unwrap();
    assert!(text.contains("[INFO] 任务开始"));
    assert!(text.contains("[ERROR] ADB 设备离线"));
    assert_eq!(text, log.text().unwrap());
}

#[test]
fn local_endpoint_recovers_markers_and_recreates_the_configured_folder() {
    let dir = tempfile::tempdir().unwrap();
    let endpoint = msync_core::endpoint::Endpoint::inspect(dir.path(), "电脑").unwrap();
    assert!(endpoint.guid.starts_with("\\\\?\\Volume{"));
    let endpoint = endpoint.restore_meta("test-pair").unwrap();
    let (root, missing) = endpoint.validate("test-pair").unwrap();
    assert!(root.is_dir() && !missing);
    std::fs::write(
        dir.path().join(".usbsync-meta/endpoint.json"),
        r#"{"pair":"another-pair","endpoint":"other","role":"电脑"}"#,
    )
    .unwrap();
    assert!(endpoint.validate("test-pair").unwrap().1);
    std::fs::write(dir.path().join(".usbsync-meta/endpoint.json"), "损坏内容").unwrap();
    assert!(endpoint.validate("test-pair").unwrap().1);
    let mut endpoint = endpoint.restore_meta("test-pair").unwrap();
    endpoint.serial ^= 1;
    assert!(endpoint.validate("test-pair").unwrap().1);
    let endpoint = endpoint.restore_meta("test-pair").unwrap();
    assert!(!endpoint.validate("test-pair").unwrap().1);
    let work = dir.path().join("工作文件");
    std::fs::create_dir(&work).unwrap();
    let endpoint = msync_core::endpoint::Endpoint::inspect(&work, "电脑").unwrap();
    std::fs::write(work.join("保留.md"), "原始文件").unwrap();
    std::fs::rename(&work, dir.path().join("移动后的文件夹")).unwrap();
    let (root, merge) = endpoint.validate("test-pair").unwrap();
    assert!(merge && root == work && root.is_dir());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("移动后的文件夹/保留.md")).unwrap(),
        "原始文件"
    );
}

#[test]
fn file_progress_survives_restart_without_committing_failed_paths() {
    use msync_core::{snapshot::Item, store::FileFailure};
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let item = Item {
        directory: false,
        size: 1,
        modified: 1,
        hash: "old".into(),
    };
    let snapshot = [
        ("已删除".into(), item.clone()),
        ("尚未同步".into(), item.clone()),
    ]
    .into();
    let mut baseline = Baseline {
        generation: "first".into(),
        snapshots: [snapshot, Snapshot::new()],
        completed: "上次成功".into(),
    };
    baseline.snapshots[1] = baseline.snapshots[0].clone();
    store.finish("pair", &baseline, &[]).unwrap();
    store
        .checkpoint_path("pair", "已删除", &[None, None])
        .unwrap();
    drop(store);
    let mut store = Store::open(dir.path()).unwrap();
    let mut resumed = store.baseline("pair").unwrap().unwrap();
    assert!(resumed
        .snapshots
        .iter()
        .all(|s| !s.contains_key("已删除") && s["尚未同步"] == item));
    assert_eq!(resumed.completed, "上次成功");
    resumed.generation = "next".into();
    store
        .finish(
            "pair",
            &resumed,
            &[FileFailure {
                path: "尚未同步".into(),
                reason: "被占用".into(),
            }],
        )
        .unwrap();
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.failures("pair").unwrap()[0].reason, "被占用");
    assert_eq!(
        store.baseline("pair").unwrap().unwrap().snapshots[0],
        resumed.snapshots[0]
    );
}
