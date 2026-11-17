use msync_core::{
    plan::{self, Action},
    snapshot::{self, Item, Snapshot},
};
use std::collections::BTreeMap;
fn file(hash: &str, time: i64) -> Item {
    Item {
        directory: false,
        size: 4,
        modified: time,
        hash: hash.into(),
    }
}
fn one(hash: &str, time: i64) -> Snapshot {
    BTreeMap::from([("工作.txt".into(), file(hash, time))])
}
#[test]
fn restored_old_version_and_preserved_timestamp_are_changes() {
    let previous = [one("original", 50000), one("original", 50000)];
    for time in [10000, 50000] {
        let current = [one("edited", time), previous[1].clone()];
        let p = plan::build(&current, Some(&previous), 2000, &BTreeMap::new()).unwrap();
        assert_eq!(p.actions["工作.txt"], Action::Copy { from: 0 });
        assert!(p.conflicts.is_empty());
    }
}
#[test]
fn first_merge_deletion_and_delete_edit_have_distinct_semantics() {
    let old = [one("original", 50000), one("original", 50000)];
    let current = [Snapshot::new(), old[1].clone()];
    let first = plan::build(&current, None, 2000, &BTreeMap::new()).unwrap();
    assert_eq!(first.actions["工作.txt"], Action::Copy { from: 1 });
    let deleted = plan::build(&current, Some(&old), 2000, &BTreeMap::new()).unwrap();
    assert_eq!(deleted.actions["工作.txt"], Action::Delete { side: 1 });
    let edited = [Snapshot::new(), one("edited", 50000)];
    let p = plan::build(&edited, Some(&old), 2000, &BTreeMap::new()).unwrap();
    assert_eq!(p.actions["工作.txt"], Action::Copy { from: 1 });
}
#[test]
fn ambiguous_double_edit_waits_for_the_selected_version() {
    let old = [one("original", 50000), one("original", 50000)];
    let current = [one("new-local", 60000), one("new-tablet", 61000)];
    let p = plan::build(&current, Some(&old), 2000, &BTreeMap::new()).unwrap();
    assert_eq!(p.conflicts.len(), 1);
    assert!(p.actions.is_empty());
    let p = plan::build(
        &current,
        Some(&old),
        2000,
        &BTreeMap::from([("工作.txt".into(), 1)]),
    )
    .unwrap();
    assert_eq!(p.actions["工作.txt"], Action::Copy { from: 1 });
    let equal = [one("same", 60000), one("same", 90000)];
    let p = plan::build(&equal, Some(&old), 2000, &BTreeMap::new()).unwrap();
    assert!(p.actions.is_empty());
}
#[test]
fn merge_requires_a_choice_for_different_content_regardless_of_timestamp() {
    let current = [one("电脑内容", 50000), one("平板内容", 500000)];
    let plan = plan::build(&current, None, 2000, &BTreeMap::new()).unwrap();
    assert_eq!(plan.conflicts.len(), 1);
    assert!(plan.actions.is_empty());
    let chosen = plan::build(
        &current,
        None,
        2000,
        &BTreeMap::from([("工作.txt".into(), 0)]),
    )
    .unwrap();
    assert_eq!(chosen.actions["工作.txt"], Action::Copy { from: 0 });
}
#[test]
fn filesystem_scan_reads_content_even_if_size_and_time_match() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("中文 空格.txt");
    std::fs::write(&path, "AAAA").unwrap();
    let time = filetime::FileTime::from_unix_time(100000, 0);
    filetime::set_file_mtime(&path, time).unwrap();
    let before = snapshot::scan(dir.path()).unwrap();
    std::fs::write(&path, "BBBB").unwrap();
    filetime::set_file_mtime(&path, time).unwrap();
    let after = snapshot::scan(dir.path()).unwrap();
    assert!(!snapshot::equal(&before, &after));
    assert_eq!(
        before["中文 空格.txt"].modified,
        after["中文 空格.txt"].modified
    );
}
