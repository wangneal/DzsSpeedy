# 管理员运行：杀 WeGame 用户态进程 -> 重试停止 QMT 驱动 -> 若卸载成功立即注入 Asura
$dir = 'D:\DzsSpeedy\_tools\hooktest'
$out = Join-Path $dir 'result10.txt'
"=== start $(Get-Date -Format o) ===" | Out-File $out -Encoding utf8

Get-Process -Name wegame, wegame_env, CrossProxy, tcls_core -ErrorAction SilentlyContinue | ForEach-Object {
    "killing $($_.ProcessName) pid=$($_.Id)" | Out-File $out -Append -Encoding utf8
    Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
}
Start-Sleep -Seconds 2
sc.exe stop WeGameProcService | Out-File $out -Append -Encoding utf8
Start-Sleep -Seconds 5
$st = (Get-CimInstance Win32_SystemDriver -Filter "Name='WeGameProcService'").State
"wegame_driver_state=$st" | Out-File $out -Append -Encoding utf8

$asura = Get-Process -Name asura -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $asura) { "ERROR: Asura not running (game may have closed)" | Out-File $out -Append -Encoding utf8; exit 0 }
$pid_t = $asura.Id
"asura_pid=$pid_t" | Out-File $out -Append -Encoding utf8
Remove-Item "$env:TEMP\hooktest-dllmain-$pid_t.txt","$env:TEMP\hooktest-hookproc-$pid_t.txt" -ErrorAction SilentlyContinue
& (Join-Path $dir 'inject32.exe') $pid_t (Join-Path $dir 'hookdll32.dll') *>> $out
"--- inject log ---" | Out-File $out -Append -Encoding utf8
Get-Content "$env:TEMP\hooktest-inject-$pid_t.log" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
"--- markers ---" | Out-File $out -Append -Encoding utf8
Get-Content "$env:TEMP\hooktest-dllmain-$pid_t.txt","$env:TEMP\hooktest-hookproc-$pid_t.txt" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
"=== end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8