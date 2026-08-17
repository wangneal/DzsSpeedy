# 由用户在【管理员 PowerShell】中运行：注入测试 DLL 到 Asura(11416)
# 用法: powershell -NoProfile -ExecutionPolicy Bypass -File D:\DzsSpeedy\_tools\hooktest\test6.ps1
$ErrorActionPreference = 'Continue'
$dir = 'D:\DzsSpeedy\_tools\hooktest'
$out = Join-Path $dir 'result6.txt'
"=== start $(Get-Date -Format o) ===" | Out-File $out -Encoding utf8

$asura = Get-Process -Name asura -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $asura) {
    "ERROR: Asura not running" | Out-File $out -Append -Encoding utf8
    exit 1
}
$pid_t = $asura.Id
"asura_pid=$pid_t" | Out-File $out -Append -Encoding utf8

Remove-Item "$env:TEMP\hooktest-dllmain-$pid_t.txt","$env:TEMP\hooktest-hookproc-$pid_t.txt" -ErrorAction SilentlyContinue

& (Join-Path $dir 'inject32.exe') $pid_t (Join-Path $dir 'hookdll32.dll') *>> $out
"--- inject log ---" | Out-File $out -Append -Encoding utf8
Get-Content "$env:TEMP\hooktest-inject-$pid_t.log" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
"--- markers ---" | Out-File $out -Append -Encoding utf8
Get-Content "$env:TEMP\hooktest-dllmain-$pid_t.txt","$env:TEMP\hooktest-hookproc-$pid_t.txt" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
"=== end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8