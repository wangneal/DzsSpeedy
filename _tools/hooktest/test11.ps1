# 管理员运行：杀 WeGame 用户态 -> 停 QMT 驱动 -> 禁用服务（防 WeGame 重启它）
$out = 'D:\DzsSpeedy\_tools\hooktest\result11.txt'
"=== start $(Get-Date -Format o) ===" | Out-File $out -Encoding utf8
Get-Process -Name wegame, wegame_env, tcls_core, CrossProxy -ErrorAction SilentlyContinue | ForEach-Object {
    "killing $($_.ProcessName) pid=$($_.Id)" | Out-File $out -Append -Encoding utf8
    Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
}
Start-Sleep -Seconds 2
sc.exe stop WeGameProcService | Out-File $out -Append -Encoding utf8
Start-Sleep -Seconds 3
sc.exe config WeGameProcService start=disabled | Out-File $out -Append -Encoding utf8
Start-Sleep -Seconds 1
$st = (Get-CimInstance Win32_SystemDriver -Filter "Name='WeGameProcService'").State
"wegame_driver_state=$st" | Out-File $out -Append -Encoding utf8
"=== end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8