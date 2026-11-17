param(
    [string]$RuntimePath = "$PSScriptRoot\..\runtime\WebView2",
    [string]$AdbPath = "$PSScriptRoot\..\runtime\adb-download\platform-tools",
    [string]$SevenZipPath = "$PSScriptRoot\..\runtime\7zip\7zr.exe"
)
$ErrorActionPreference = 'Stop'
$project = (Resolve-Path "$PSScriptRoot\..").Path
Set-Location $project
if (!(Test-Path -LiteralPath $SevenZipPath -PathType Leaf)) { throw '缺少 7-Zip 命令行程序。请指定 -SevenZipPath，或将官方 7zr.exe 放入 runtime/7zip。' }
$SevenZipPath = (Resolve-Path -LiteralPath $SevenZipPath).Path
if (!(Test-Path -LiteralPath "$RuntimePath\msedgewebview2.exe")) { throw '缺少 Fixed Version WebView2 运行时。请解压官方 CAB 后指定 -RuntimePath。' }
foreach ($name in @('adb.exe', 'AdbWinApi.dll', 'AdbWinUsbApi.dll', 'NOTICE.txt', 'source.properties')) {
    if (!(Test-Path -LiteralPath "$AdbPath\$name")) { throw "缺少官方 Platform-Tools 文件：$name" }
}
if (!(Test-Path -LiteralPath "$project/docs/MSync 图文使用说明.docx")) { throw '缺少图文使用说明，请先生成并检查文档。' }
$usbDriver = Join-Path $project 'runtime/usb-driver/usb_driver'
if (!(Test-Path -LiteralPath "$usbDriver/android_winusb.inf")) { throw '缺少已验证的首次连接支持文件。' }
foreach ($catalog in @('androidwinusb86.cat', 'androidwinusba64.cat')) {
    if ((Get-AuthenticodeSignature -LiteralPath "$usbDriver/$catalog").Status -ne 'Valid') { throw "连接支持文件签名无效：$catalog" }
}
npm run build
if ($LASTEXITCODE) { throw '前端构建失败' }
cargo build --release --workspace
if ($LASTEXITCODE) { throw 'Rust 构建失败' }
$version = (Get-Content 'crates/desktop/tauri.conf.json' -Raw | ConvertFrom-Json).version.Replace('.', '-')
$buildId = $version + '-' + (Get-FileHash 'target/release/msync-ui.exe' -Algorithm SHA256).Hash.Substring(0,12).ToLower()
$output = Join-Path $project "releases/MSync-$buildId"
if (Test-Path -LiteralPath $output) { throw "交付目录已存在，避免覆盖：$output" }
New-Item -ItemType Directory -Force "$output/app/adb","$output/runtime/WebView2","$output/licenses/adb" | Out-Null
Copy-Item 'target/release/MSync.exe' "$output/MSync.exe"
Copy-Item 'target/release/msync-ui.exe' "$output/app/msync-ui.exe"
foreach ($name in @('adb.exe', 'AdbWinApi.dll', 'AdbWinUsbApi.dll')) { Copy-Item -LiteralPath "$AdbPath\$name" -Destination "$output/app/adb" }
Copy-Item -LiteralPath "$AdbPath/NOTICE.txt","$AdbPath/source.properties" -Destination "$output/licenses/adb"
Copy-Item "$RuntimePath/*" "$output/runtime/WebView2" -Recurse
Copy-Item 'docs/使用说明.md' "$output/使用说明.md"
Copy-Item 'LICENSE' "$output/LICENSE"
Copy-Item 'docs/MSync 图文使用说明.docx' $output
New-Item -ItemType Directory -Force "$output/首次连接" | Out-Null
Copy-Item 'tools/首次连接.cmd','tools/prepare-device.ps1' "$output/首次连接"
Copy-Item -LiteralPath $usbDriver -Destination "$output/首次连接/usb_driver" -Recurse
$metadata = cargo metadata --format-version 1 | ConvertFrom-Json
foreach ($package in $metadata.packages) {
    $source = Split-Path $package.manifest_path
    $licenses = Get-ChildItem -LiteralPath $source -File | Where-Object { $_.Name -match '^(LICENSE|COPYING|NOTICE)' }
    if ($licenses) {
        $destination = "$output/licenses/$($package.name)-$($package.version)"
        New-Item -ItemType Directory -Force $destination | Out-Null
        $licenses | Copy-Item -Destination $destination
    }
}
$metadata.packages | Select-Object name,version,license,repository | ConvertTo-Json -Depth 5 | Set-Content "$output/licenses/依赖清单.json" -Encoding utf8
$files = @(Get-ChildItem "$output/app","$output/runtime","$output/licenses" -Recurse -File | ForEach-Object {
    @{path=$_.FullName.Substring($output.Length+1).Replace('\','/');sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLower()}
})
@{build_id=$buildId;files=$files} | ConvertTo-Json -Depth 5 | Set-Content "$output/version-manifest.json" -Encoding utf8NoBOM
# 高压缩率与固实压缩生成单包；限制为两个线程，控制压缩时的内存占用。
& $SevenZipPath a -t7z -mx9 -m0=LZMA2 -md=128m -mfb=273 -ms=on -mmt=2 -bd -sse "$output.7z" $output
if ($LASTEXITCODE) { throw '压缩失败' }
& $SevenZipPath t -bd "$output.7z"
if ($LASTEXITCODE) { throw '压缩包完整性检查失败' }
Get-FileHash -LiteralPath "$output.7z" -Algorithm SHA256
