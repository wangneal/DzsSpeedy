# 管理员 PowerShell 运行：验证"提权目标进程"是否也被拦截（区分 360 拦游戏 vs 拦所有提权进程）
# powershell -NoProfile -ExecutionPolicy Bypass -File D:\DzsSpeedy\_tools\hooktest\test7.ps1
$ErrorActionPreference = 'Continue'
$dir = 'D:\DzsSpeedy\_tools\hooktest'
$out = Join-Path $dir 'result7.txt'
"=== start $(Get-Date -Format o) ===" | Out-File $out -Encoding utf8

Remove-Item "$env:TEMP\hooktest-*" -ErrorAction SilentlyContinue

$f = Start-Process -FilePath (Join-Path $dir 'fixture64.exe') -PassThru
Start-Sleep -Seconds 2
"fixture_pid=$($f.Id)" | Out-File $out -Append -Encoding utf8
& (Join-Path $dir 'inject64.exe') $f.Id (Join-Path $dir 'hookdll64.dll') *>> $out
"--- inject log ---" | Out-File $out -Append -Encoding utf8
Get-Content "$env:TEMP\hooktest-inject-$($f.Id).log" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
"--- markers ---" | Out-File $out -Append -Encoding utf8
Get-Content "$env:TEMP\hooktest-dllmain-$($f.Id).txt","$env:TEMP\hooktest-hookproc-$($f.Id).txt" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
Stop-Process -Id $f.Id -Force -ErrorAction SilentlyContinue
"=== end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8