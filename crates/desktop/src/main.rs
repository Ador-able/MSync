#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod preview;

use anyhow::{Context, Result};
use msync_core::{
    adb::{self, Adb, Binding, Entry as RemoteEntry, Unavailable},
    log::{Entry, Log},
    store::{FileFailure, Pair},
    sync::{Coordinator, Preview},
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager, State,
};

#[derive(Clone, Serialize)]
struct Progress {
    completed: usize,
    total: usize,
}
#[derive(Clone, Serialize)]
struct Status {
    phase: String,
    detail: String,
    busy: bool,
    pair: Option<Pair>,
    last_success: Option<String>,
    preview: Option<Preview>,
    failures: Vec<FileFailure>,
    progress: Option<Progress>,
    elapsed_ms: u64,
    #[serde(skip)]
    started: Option<Instant>,
}
struct App {
    coordinator: Mutex<Coordinator>,
    status: Mutex<Status>,
    log: Log,
    stop: Arc<AtomicBool>,
    previews: tempfile::TempDir,
}
impl App {
    fn phase(&self, s: &str) {
        self.status.lock().unwrap().phase = s.into();
        self.log.write("INFO", format!("阶段：{s}"));
    }
    fn refresh(&self, co: &Coordinator) -> Result<()> {
        let pair = co.store.pair()?;
        let mut s = self.status.lock().unwrap();
        if let Some(ref p) = pair {
            s.last_success = co
                .store
                .baseline(&p.id)?
                .map(|b| b.completed)
                .filter(|time| !time.is_empty());
            s.failures = co.store.failures(&p.id)?;
        }
        s.pair = pair;
        s.preview = co.preview();
        Ok(())
    }
    fn begin(&self) -> std::result::Result<(), String> {
        let mut s = self.status.lock().unwrap();
        if s.busy {
            return Err("已有任务正在运行".into());
        }
        s.busy = true;
        s.started = Some(Instant::now());
        s.elapsed_ms = 0;
        s.progress = None;
        s.detail = "可以继续使用文件，正在保存或被占用的文件会自动重试。".into();
        self.stop.store(false, Ordering::Relaxed);
        Ok(())
    }
    fn finish(&self, result: Result<()>) {
        let mut s = self.status.lock().unwrap();
        s.elapsed_ms = s.started.map_or(0, |t| t.elapsed().as_millis() as u64);
        s.busy = false;
        if let Err(e) = result {
            s.phase = if self.stop.load(Ordering::Relaxed) {
                "已暂停"
            } else if let Some(issue) = e.downcast_ref::<Unavailable>() {
                issue.phase
            } else {
                "等待重试"
            }
            .into();
            s.detail = if self.stop.load(Ordering::Relaxed) {
                "已暂停；点击立即同步继续。".into()
            } else {
                format!("{e:#}；自动同步开启时，稍后继续。")
            };
            self.log.write("ERROR", &s.detail);
        }
    }
    fn progress(&self, completed: usize, total: usize) {
        let mut s = self.status.lock().unwrap();
        s.phase = "同步中".into();
        s.progress = Some(Progress { completed, total });
        self.log
            .write("INFO", format!("同步进度：已处理 {completed}/{total}"));
    }
    fn completed(&self) {
        let mut s = self.status.lock().unwrap();
        if let Some(preview) = &s.preview {
            s.detail = format!(
                "{} 项需要选择版本，其余文件已处理。",
                preview.conflicts.len()
            );
            s.phase = "有文件待选版".into();
        } else if s.failures.is_empty() {
            s.phase = "已完成".into();
            s.detail = "本轮同步完成，可以继续使用文件。".into();
        } else {
            s.phase = "部分文件未完成".into();
            s.detail = format!(
                "{} 项失败，其余已完成。自动同步开启时会继续重试。",
                s.failures.len()
            );
        }
    }
}
#[tauri::command]
fn get_status(app: State<Arc<App>>) -> Status {
    let mut status = app.status.lock().unwrap().clone();
    if status.busy {
        status.elapsed_ms = status.started.map_or(0, |t| t.elapsed().as_millis() as u64);
    }
    status
}
#[tauri::command]
fn get_logs(app: State<Arc<App>>) -> Vec<Entry> {
    app.log.entries()
}
#[tauri::command]
fn copy_logs(app: State<Arc<App>>) -> std::result::Result<(), String> {
    (move || -> Result<()> {
        arboard::Clipboard::new()?.set_text(app.log.text()?)?;
        Ok(())
    })()
    .map_err(|e| format!("复制日志失败：{e:#}"))
}
fn open_path(path: PathBuf) -> Result<()> {
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()?;
    Ok(())
}
#[tauri::command]
fn open_logs(app: State<Arc<App>>) -> std::result::Result<(), String> {
    open_path(app.log.path.parent().unwrap().to_path_buf()).map_err(|e| e.to_string())
}
#[tauri::command]
fn open_backups(app: State<Arc<App>>) -> std::result::Result<(), String> {
    let co = app.coordinator.try_lock().map_err(|_| "同步正在运行")?;
    let pair = co
        .store
        .pair()
        .map_err(|e| e.to_string())?
        .ok_or("尚未绑定")?;
    open_path(pair.local.root().join(msync_core::snapshot::BACKUP)).map_err(|e| e.to_string())?;
    app.log.write(
        "INFO",
        format!(
            "电脑备份已打开。平板备份位于 {}\\_同步备份",
            pair.remote.display
        ),
    );
    Ok(())
}
#[tauri::command]
async fn open_conflict(
    token: String,
    path: String,
    side: usize,
    app: State<'_, Arc<App>>,
) -> std::result::Result<(), String> {
    let app = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<()> {
        let (pair, conflict) = {
            let co = app
                .coordinator
                .try_lock()
                .map_err(|_| anyhow::anyhow!("正在检查文件，请稍后再打开"))?;
            let pending = co.preview().context("该冲突已处理")?;
            anyhow::ensure!(pending.token == token, "文件状态已更新，请重试");
            let conflict = pending
                .conflicts
                .into_iter()
                .find(|c| c.path == path)
                .context("该冲突已处理")?;
            (co.store.pair()?.context("尚未绑定")?, conflict)
        };
        let file =
            preview::copy_version(app.previews.path(), &pair, &conflict, side, app.log.clone())?;
        open_path(file.clone())?;
        app.log
            .write("INFO", format!("打开版本副本：{}", file.display()));
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("打开失败：{e:#}"))
}
#[tauri::command]
async fn choose_folder() -> Option<String> {
    rfd::AsyncFileDialog::new()
        .set_title("选择电脑文件夹")
        .pick_folder()
        .await
        .map(|p| p.path().to_string_lossy().into_owned())
}
#[tauri::command]
async fn list_devices(
    app: State<'_, Arc<App>>,
) -> std::result::Result<Vec<adb::DeviceInfo>, String> {
    app.log.write("INFO", "开始枚举 ADB 设备及授权状态");
    let log = app.log.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<_> {
        adb::devices(log, Arc::new(AtomicBool::new(false)))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("{e:#}"));
    match &result {
        Ok(devices) => app.log.write(
            "INFO",
            format!(
                "设备枚举完成：{}",
                devices
                    .iter()
                    .map(|d| format!("{} [{}] {}", d.name, d.state, d.serial))
                    .collect::<Vec<_>>()
                    .join("、")
            ),
        ),
        Err(error) => app.log.write("ERROR", format!("设备枚举失败：{error}")),
    }
    result
}
#[derive(Serialize)]
struct FolderList {
    folders: Vec<RemoteEntry>,
}
#[tauri::command]
async fn list_folders(
    serial: String,
    parent: String,
    app: State<'_, Arc<App>>,
) -> std::result::Result<FolderList, String> {
    app.log
        .write("INFO", format!("浏览设备目录：{parent}，设备={serial}"));
    let log = app.log.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<_> {
        let adb = Adb::open(&serial, log, Arc::new(AtomicBool::new(false)))?;
        let objects = adb.list(&parent)?;
        Ok(FolderList {
            folders: objects.into_iter().filter(|o| o.directory).collect(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("{e:#}"));
    match &result {
        Ok(list) => {
            app.log
                .write("INFO", format!("读取到 {} 个子文件夹", list.folders.len()));
        }
        Err(error) => app.log.write("ERROR", format!("目录读取失败：{error}")),
    }
    result
}
#[tauri::command]
async fn bind(
    local: String,
    serial: String,
    path: String,
    label: String,
    app: State<'_, Arc<App>>,
) -> std::result::Result<(), String> {
    app.begin()?;
    let app = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| -> Result<()> {
            app.phase("检查文件夹中");
            let adb = Adb::open(&serial, app.log.clone(), app.stop.clone())?;
            let path = adb.canonical_root(&path)?;
            anyhow::ensure!(
                path != adb::SHARED_ROOT && path != "/",
                "请选择共享存储内的工作子文件夹"
            );
            let remote = Binding {
                serial,
                path: path.clone(),
                id: uuid::Uuid::new_v4().to_string(),
                display: format!("{label} / {path}"),
                label,
            };
            let mut co = app.coordinator.lock().unwrap();
            co.bind(&PathBuf::from(local), remote)?;
            app.refresh(&co)?;
            let mut s = app.status.lock().unwrap();
            s.phase = "已就绪".into();
            s.detail = "文件夹已绑定，正在开始同步。".into();
            Ok(())
        })();
        let error = result.as_ref().err().map(|e| format!("{e:#}"));
        app.finish(result);
        error.map_or(Ok(()), Err)
    })
    .await
    .map_err(|e| e.to_string())?
}
fn start(
    app: Arc<App>,
    handle: tauri::AppHandle,
    confirmation: Option<(String, BTreeMap<String, usize>)>,
) -> std::result::Result<(), String> {
    app.begin()?;
    std::thread::spawn(move || {
        let result = (|| -> Result<()> {
            app.phase("检查文件中");
            let mut co = app.coordinator.lock().unwrap();
            let before = co.preview().map(|p| p.conflicts);
            let (token, choices) = match confirmation {
                Some(selected) => selected,
                None => (co.prepare()?.token, BTreeMap::new()),
            };
            co.execute(&token, choices, |completed, total| {
                app.progress(completed, total)
            })?;
            app.refresh(&co)?;
            app.completed();
            let after = co.preview().map(|p| p.conflicts);
            if after.is_some() && after != before {
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }
            Ok(())
        })();
        if let Ok(co) = app.coordinator.lock() {
            let _ = app.refresh(&co);
        }
        app.finish(result);
    });
    Ok(())
}
#[tauri::command]
fn start_sync(app: State<Arc<App>>, handle: tauri::AppHandle) -> std::result::Result<(), String> {
    start(app.inner().clone(), handle, None)
}
#[tauri::command]
fn confirm_sync(
    token: String,
    choices: BTreeMap<String, usize>,
    app: State<Arc<App>>,
    handle: tauri::AppHandle,
) -> std::result::Result<(), String> {
    start(app.inner().clone(), handle, Some((token, choices)))
}
#[tauri::command]
fn stop_sync(app: State<Arc<App>>) {
    app.stop.store(true, Ordering::Relaxed);
    app.phase("正在停止");
    app.log.write(
        "WARN",
        "收到停止请求，将在当前设备调用返回后停止，保留恢复材料",
    );
}
#[tauri::command]
fn set_automatic(enabled: bool, app: State<Arc<App>>) -> std::result::Result<(), String> {
    (move || -> Result<()> {
        let co = app
            .coordinator
            .try_lock()
            .map_err(|_| anyhow::anyhow!("任务正在运行"))?;
        let mut pair = co.store.pair()?.context("尚未绑定")?;
        pair.automatic = enabled;
        co.store.save_pair(&pair)?;
        app.refresh(&co)?;
        app.log.write("INFO", format!("自动同步：{enabled}"));
        Ok(())
    })()
    .map_err(|e| format!("{e:#}"))
}
#[tauri::command]
fn set_ignore_patterns(
    patterns: Vec<String>,
    app: State<Arc<App>>,
) -> std::result::Result<(), String> {
    (|| -> Result<()> {
        let mut co = app
            .coordinator
            .try_lock()
            .map_err(|_| anyhow::anyhow!("任务正在运行，请等待完成后保存"))?;
        co.set_ignore_patterns(patterns)?;
        app.refresh(&co)?;
        Ok(())
    })()
    .map_err(|e| format!("{e:#}"))
}
#[tauri::command]
fn quit(handle: tauri::AppHandle, app: State<Arc<App>>) -> std::result::Result<(), String> {
    if app.status.lock().unwrap().busy {
        return Err("请先停止同步，等待正在运行的设备调用结束后再退出。".into());
    }
    app.log.write("INFO", "用户退出程序");
    handle.exit(0);
    Ok(())
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    let dir = if let Some(i) = args.iter().position(|a| a == "--data-dir") {
        PathBuf::from(args.get(i + 1).context("--data-dir 缺少目录")?)
    } else {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").context("无法定位本机应用数据目录")?)
            .join("MSync")
    };
    std::fs::create_dir_all(&dir)?;
    let instance = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("app.lock"))?;
    fs2::FileExt::try_lock_exclusive(&instance).context("MSync 已在运行，请在系统托盘打开")?;
    let log = Log::new(&dir.join("logs"))?;
    log.write(
        "INFO",
        format!(
            "MSync {} 启动；状态目录 {}；传输 ADB USB",
            env!("CARGO_PKG_VERSION"),
            dir.display()
        ),
    );
    let stop = Arc::new(AtomicBool::new(false));
    match adb::version(log.clone()) {
        Ok(version) => log.write("INFO", version),
        Err(error) => log.write("ERROR", format!("ADB 初始化失败：{error:#}")),
    }
    let co = Coordinator::open(dir, log.clone(), stop.clone())?;
    let app = Arc::new(App {
        coordinator: Mutex::new(co),
        status: Mutex::new(Status {
            phase: "尚未设置".into(),
            detail: "选好电脑和平板文件夹，就可以开始。".into(),
            busy: false,
            pair: None,
            last_success: None,
            preview: None,
            failures: vec![],
            progress: None,
            elapsed_ms: 0,
            started: None,
        }),
        log,
        stop,
        previews: tempfile::Builder::new().prefix("MSync-预览-").tempdir()?,
    });
    app.refresh(&app.coordinator.lock().unwrap())?;
    {
        let mut s = app.status.lock().unwrap();
        if s.pair.is_some() {
            s.phase = "已就绪".into();
            s.detail = "连接平板后自动同步。".into();
        }
    }
    let on_close = app.clone();
    tauri::Builder::default()
        .manage(app)
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_logs,
            copy_logs,
            open_logs,
            open_backups,
            open_conflict,
            choose_folder,
            list_devices,
            list_folders,
            bind,
            start_sync,
            confirm_sync,
            stop_sync,
            set_automatic,
            set_ignore_patterns,
            quit
        ])
        .setup(|app| {
            let background = app.state::<Arc<App>>().inner().clone();
            let handle = app.handle().clone();
            // 串行检查；只有需要用户选版或确认时才展开窗口。
            std::thread::spawn(move || {
                background
                    .coordinator
                    .lock()
                    .unwrap()
                    .cleanup_legacy_markers();
                let mut delay = 10;
                loop {
                    std::thread::sleep(Duration::from_secs(delay));
                    delay = 60;
                    let s = background.status.lock().unwrap().clone();
                    if s.busy || s.phase == "已暂停" {
                        continue;
                    }
                    if s.pair.as_ref().is_some_and(|p| p.automatic) {
                        let _ = start(background.clone(), handle.clone(), None);
                    }
                }
            });
            let show = MenuItem::with_id(app, "show", "打开 MSync", true, None::<&str>)?;
            let exit = MenuItem::with_id(app, "exit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &exit])?;
            let mut rgba = vec![0u8; 32 * 32 * 4];
            for y in 0..32 {
                for x in 0..32 {
                    let i = (y * 32 + x) * 4;
                    rgba[i..i + 4].copy_from_slice(
                        if (y == 10 || y == 21) && (6..26).contains(&x) {
                            &[240, 255, 246, 255]
                        } else {
                            &[23, 106, 87, 255]
                        },
                    );
                }
            }
            TrayIconBuilder::new()
                .icon(tauri::image::Image::new_owned(rgba, 32, 32))
                .tooltip("MSync · 文件同步")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "exit" => {
                        let state = app.state::<Arc<App>>();
                        if !state.status.lock().unwrap().busy {
                            app.exit(0);
                        } else {
                            state.stop.store(true, Ordering::Relaxed);
                            state.phase("正在停止");
                        }
                    }
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(move |window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                on_close.log.write(
                    "INFO",
                    "窗口已收起到托盘；自动同步仍按设置运行，托盘菜单可退出",
                );
            }
        })
        .build(tauri::generate_context!())?
        .run(|handle, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                // Tauri 直接结束进程，不能依赖 TempDir 的析构函数清理副本。
                let state = handle.state::<Arc<App>>();
                if let Err(error) = std::fs::remove_dir_all(state.previews.path()) {
                    state
                        .log
                        .write("WARN", format!("预览副本清理失败：{error}"));
                }
            }
        });
    drop(instance);
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        rfd::MessageDialog::new()
            .set_title("MSync 无法启动")
            .set_description(format!("{error:#}"))
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}
