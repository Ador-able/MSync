$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
try {
    # 仅为匹配的硬件接口安装驱动，其他设备需使用厂商提供的驱动。
    $device = @(Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -like 'USB\VID_18D1&PID_4E11\*' })
    if (!$device.Count) { throw '请先用数据线连接平板，解锁并打开 USB 调试。若已经完成，请把此提示发给提供软件的人。' }
    $principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if (!$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        Start-Process -FilePath 'powershell.exe' -Verb RunAs -WindowStyle Hidden -ArgumentList ('-NoProfile -ExecutionPolicy Bypass -File "' + $PSCommandPath + '"') | Out-Null
        exit
    }
    $log = Join-Path $env:TEMP 'MSync-首次连接.txt'
    & "$env:WINDIR\System32\pnputil.exe" /add-driver (Join-Path $PSScriptRoot 'usb_driver\android_winusb.inf') /install 2>&1 | Out-File -LiteralPath $log -Encoding utf8
    if ($LASTEXITCODE -ne 0) { throw "连接准备未完成。请将此窗口截图，并把文件 $log 发给提供软件的人。" }
    [System.Windows.Forms.MessageBox]::Show('电脑已完成连接准备。请查看平板，若出现 USB 调试提示，请点允许；然后回到 MSync 点刷新。', 'MSync 首次连接') | Out-Null
} catch {
    [System.Windows.Forms.MessageBox]::Show($_.Exception.Message, 'MSync 连接提示') | Out-Null
}
