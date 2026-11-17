$ErrorActionPreference = 'Stop'
Set-Location "$PSScriptRoot\.."
$env:MSYNC_ADB_PATH = (Resolve-Path 'runtime/adb-download/platform-tools/adb.exe').Path
npm run build
if ($LASTEXITCODE) { throw '前端构建失败' }
cargo run -p msync-desktop
if ($LASTEXITCODE) { throw '桌面程序退出异常' }
