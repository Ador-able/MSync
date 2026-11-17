use anyhow::{ensure, Context, Result};
use msync_core::{
    adb::{self, Adb},
    log::Log,
    plan::Conflict,
    snapshot,
    store::Pair,
};
use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
};

pub fn copy_version(
    previews: &Path,
    pair: &Pair,
    conflict: &Conflict,
    side: usize,
    log: Log,
) -> Result<PathBuf> {
    ensure!(side < 2, "无效的文件版本");
    let source = Path::new(&conflict.path);
    let stem = source.file_stem().context("文件名为空")?.to_string_lossy();
    let extension = source
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let label = if side == 0 { "电脑" } else { "平板" };
    let directory = tempfile::tempdir_in(previews)?;
    let file = directory
        .path()
        .join(format!("{stem}（{label}）{extension}"));
    // 两边都打开经过校验的副本，查看或编辑副本不会改动同步源文件。
    let expected = if side == 0 {
        std::fs::copy(pair.local.root().join(source), &file).context("读取电脑版失败")?;
        &conflict.local
    } else {
        let adb = Adb::open(&pair.remote.serial, log, Arc::new(AtomicBool::new(false)))?;
        let root = adb.canonical_root(&pair.remote.path)?;
        adb.pull(&adb::join(&root, &conflict.path)?, &file)?;
        &conflict.usb
    };
    ensure!(
        snapshot::digest(&file)? == expected.hash,
        "文件已变化，请等待重新检查后打开"
    );
    // 外部程序异步读取；副本保留到本次 MSync 进程结束。
    let _ = directory.keep();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use msync_core::{adb::Binding, endpoint::Endpoint};

    #[test]
    fn preview_is_an_independent_copy_of_the_displayed_version() -> Result<()> {
        let data = tempfile::tempdir()?;
        let local = data.path().join("电脑");
        let previews = data.path().join("预览");
        std::fs::create_dir_all(&local)?;
        std::fs::create_dir_all(&previews)?;
        let source = local.join("工作记录.md");
        std::fs::write(&source, "原始版本")?;
        let item = snapshot::read_item(&source)?.unwrap();
        let conflict = Conflict {
            path: "工作记录.md".into(),
            local: item.clone(),
            usb: item,
        };
        let pair = Pair {
            id: "preview-test".into(),
            local: Endpoint::inspect(&local, "电脑")?,
            automatic: false,
            ignore_patterns: msync_core::store::default_ignore_patterns(),
            remote: Binding {
                serial: "unused".into(),
                path: "/unused".into(),
                id: "unused".into(),
                display: "平板".into(),
                label: "平板".into(),
            },
        };
        let log = Log::new(&data.path().join("日志"))?;
        let copy = copy_version(&previews, &pair, &conflict, 0, log.clone())?;
        assert_eq!(std::fs::read_to_string(&copy)?, "原始版本");
        std::fs::write(&copy, "修改预览副本")?;
        assert_eq!(std::fs::read_to_string(&source)?, "原始版本");
        std::fs::write(&source, "源文件已另行保存")?;
        assert!(copy_version(&previews, &pair, &conflict, 0, log)
            .unwrap_err()
            .to_string()
            .contains("文件已变化"));
        assert_eq!(std::fs::read_to_string(&source)?, "源文件已另行保存");
        Ok(())
    }
}
