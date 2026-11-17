#![windows_subsystem = "windows"]
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    os::windows::process::CommandExt,
    path::{Component, Path, PathBuf},
    process::Command,
};
#[derive(Deserialize)]
struct Manifest {
    build_id: String,
    files: Vec<Entry>,
}
#[derive(Deserialize)]
struct Entry {
    path: String,
    sha256: String,
}
fn hash(path: &Path) -> Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 131072];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn run() -> Result<()> {
    let source = std::env::current_exe()?
        .parent()
        .context("无法读取程序目录")?
        .to_path_buf();
    let bytes = std::fs::read(source.join("version-manifest.json"))
        .context("程序包不完整，请完整解压压缩包后再运行")?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    ensure!(
        manifest
            .build_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "版本清单格式错误"
    );
    let data = PathBuf::from(std::env::var_os("LOCALAPPDATA").context("本机应用数据目录不可用")?)
        .join("MSync");
    let log = msync_log::Log::launcher(&data.join("logs"))?;
    log.write(
        "INFO",
        format!(
            "启动准备：版本 {}，来源 {}",
            manifest.build_id,
            source.display()
        ),
    );
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(data.join("runtime.lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    let cache = data.join("runtime").join(&manifest.build_id);
    std::fs::create_dir_all(&cache)?;
    for entry in &manifest.files {
        let relative = Path::new(&entry.path);
        ensure!(
            relative
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
            "清单路径越界"
        );
        let dest = cache.join(relative);
        if dest.is_file() && hash(&dest)? == entry.sha256 {
            continue;
        }
        let original = source.join(relative);
        ensure!(
            hash(&original)? == entry.sha256,
            "程序包文件校验失败：{}",
            entry.path
        );
        std::fs::create_dir_all(dest.parent().unwrap())?;
        std::fs::copy(&original, &dest)?;
        ensure!(
            hash(&dest)? == entry.sha256,
            "运行缓存校验失败：{}",
            entry.path
        );
    }
    let runtime = cache.join("runtime/WebView2");
    ensure!(
        runtime.join("msedgewebview2.exe").is_file(),
        "缺少离线 WebView2 运行时"
    );
    // 只给自有运行时目录授予 AppContainer 读取权限，状态和用户资料不放宽。
    for sid in ["*S-1-15-2-2:(OI)(CI)(RX)", "*S-1-15-2-1:(OI)(CI)(RX)"] {
        let output = Command::new("icacls.exe")
            .arg(&runtime)
            .args(["/grant", sid, "/Q"])
            .creation_flags(0x08000000)
            .output()?;
        ensure!(
            output.status.success(),
            "无法准备 WebView2 读取权限：{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    log.write(
        "INFO",
        format!("运行缓存校验完成，启动界面：{}", cache.display()),
    );
    Command::new(cache.join("app/msync-ui.exe"))
        .env("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", &runtime)
        .env("WEBVIEW2_USER_DATA_FOLDER", data.join("webview"))
        .current_dir(&cache)
        .spawn()
        .context("无法启动 MSync 界面")?;
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        let message =
            format!("MSync 启动失败\n\n{e:#}\n\n请将整个程序包重新解压到可写目录后重试。");
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                std::ptr::null_mut(),
                wide(&message).as_ptr(),
                wide("MSync").as_ptr(),
                0x10,
            );
        }
    }
}
