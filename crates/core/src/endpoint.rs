use crate::snapshot::META;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};
use uuid::Uuid;
use windows_sys::Win32::Storage::FileSystem::*;

pub fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.as_ref().encode_wide().chain(Some(0)).collect()
}
fn string(buf: &[u16]) -> String {
    String::from_utf16_lossy(&buf[..buf.iter().position(|c| *c == 0).unwrap_or(buf.len())])
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Endpoint {
    pub id: String,
    pub role: String,
    pub guid: String,
    pub serial: u32,
    pub display: String,
    pub label: String,
}
#[derive(Serialize, Deserialize, PartialEq)]
struct Marker {
    pair: String,
    endpoint: String,
    role: String,
}
impl Endpoint {
    pub fn inspect(path: &Path, role: &str) -> Result<Self> {
        ensure!(path.is_dir(), "请选择已存在的文件夹");
        for ancestor in path.ancestors() {
            if ancestor.as_os_str().is_empty() {
                continue;
            }
            ensure!(
                std::fs::symlink_metadata(ancestor)?.file_attributes() & 0x400 == 0,
                "所选路径经过符号链接或 junction"
            );
        }
        let canonical = path.canonicalize()?;
        let mut mount = [0u16; 32768];
        let mut guid = [0u16; 128];
        let mut label = [0u16; 261];
        let mut fs = [0u16; 261];
        let mut serial = 0;
        unsafe {
            ensure!(
                GetVolumePathNameW(
                    wide(&canonical).as_ptr(),
                    mount.as_mut_ptr(),
                    mount.len() as u32
                ) != 0,
                "无法识别目录所在卷：{}",
                std::io::Error::last_os_error()
            );
            let drive = GetDriveTypeW(mount.as_ptr());
            ensure!(
                drive == 2 || drive == 3,
                "仅支持本机磁盘和 U 盘，不能使用网络盘"
            );
            ensure!(
                GetVolumeNameForVolumeMountPointW(
                    mount.as_ptr(),
                    guid.as_mut_ptr(),
                    guid.len() as u32
                ) != 0,
                "无法读取卷 GUID"
            );
            ensure!(
                GetVolumeInformationW(
                    mount.as_ptr(),
                    label.as_mut_ptr(),
                    label.len() as u32,
                    &mut serial,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    fs.as_mut_ptr(),
                    fs.len() as u32
                ) != 0,
                "无法读取卷信息"
            );
        }
        let mount_path = PathBuf::from(string(&mount)).canonicalize()?;
        let relative = canonical
            .strip_prefix(&mount_path)
            .context("无法计算卷内目录")?
            .to_path_buf();
        ensure!(
            !relative.as_os_str().is_empty(),
            "请选择盘内的文件夹，不要选择整个磁盘"
        );
        let filesystem = string(&fs);
        ensure!(
            ["NTFS", "exFAT", "FAT32"].contains(&filesystem.as_str()),
            "暂不支持文件系统：{filesystem}"
        );
        Ok(Self {
            id: Uuid::new_v4().to_string(),
            role: role.into(),
            guid: string(&guid),
            serial,
            display: path.to_string_lossy().into(),
            label: string(&label),
        })
    }
    pub fn root(&self) -> PathBuf {
        PathBuf::from(&self.display)
    }
    pub fn validate(&self, pair: &str) -> Result<(PathBuf, bool)> {
        let root = self.root();
        // 始终使用配置路径；目录被移走或删除后重建，再从另一端合并回来。
        std::fs::create_dir_all(&root)?;
        let current = Self::inspect(&root, &self.role)?;
        let changed_volume = current.guid != self.guid || current.serial != self.serial;
        match std::fs::symlink_metadata(root.join(META)) {
            Ok(meta) => ensure!(
                meta.is_dir() && meta.file_attributes() & 0x400 == 0,
                "同步信息目录类型不正确"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let merge = match std::fs::read(root.join(META).join("endpoint.json")) {
            Ok(bytes) => serde_json::from_slice::<Marker>(&bytes)
                .map_or(true, |marker| marker != self.marker(pair)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(error) => return Err(error.into()),
        };
        Ok((root, merge || changed_volume))
    }
    fn marker(&self, pair: &str) -> Marker {
        Marker {
            pair: pair.into(),
            endpoint: self.id.clone(),
            role: self.role.clone(),
        }
    }
    pub fn restore_meta(&self, pair: &str) -> Result<Self> {
        let root = self.root();
        let mut current = Self::inspect(&root, &self.role)?;
        current.id = self.id.clone();
        let meta = root.join(META);
        std::fs::create_dir_all(&meta)?;
        let mut temp = tempfile::NamedTempFile::new_in(&meta)?;
        temp.write_all(&serde_json::to_vec_pretty(&current.marker(pair))?)?;
        temp.as_file().sync_all()?;
        temp.persist(meta.join("endpoint.json"))?;
        Ok(current)
    }
}
pub fn lock(root: &Path) -> Result<File> {
    // 锁文件可先创建；身份标记仍要等合并全部成功后再补回。
    std::fs::create_dir_all(root.join(META))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(META).join("endpoint.lock"))?;
    fs2::FileExt::try_lock_exclusive(&file).context("这组文件夹正在被另一个程序同步")?;
    Ok(file)
}

pub fn cleanup_legacy_markers(root: &Path) -> Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    // 只处理旧版在固定位置生成的普通文件，不递归清理备份或工作资料。
    for entry in entries {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".usbsync-access-")
            && entry.file_type()?.is_file()
        {
            std::fs::remove_file(entry.path())?;
        }
    }
    let owner = root.join(crate::snapshot::BACKUP).join(".msync-owner");
    match std::fs::symlink_metadata(&owner) {
        Ok(meta) if meta.is_file() => std::fs::remove_file(owner)?,
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
