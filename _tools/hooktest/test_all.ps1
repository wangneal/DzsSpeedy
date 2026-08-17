# 管理员 PowerShell 运行：三组对照实验一次跑完
# A) 提权 64 位 fixture + 我的 DLL    -> result7.txt
# B) 提权 32 位 fixture + 我的 DLL    -> result9.txt
# C) 停 WeGame 驱动后注入 Asura(32位) -> result6.txt
# powershell -NoProfile -ExecutionPolicy Bypass -File D:\DzsSpeedy\_tools\hooktest\test_all.ps1
$ErrorActionPreference = 'Continue'
$dir = 'D:\DzsSpeedy\_tools\hooktest'

function RunFixture($arch, $out) {
    $inj = Join-Path $dir "inject$arch.exe"
    $dll = Join-Path $dir "hookdll$arch.dll"
    $fix = Join-Path $dir "fixture$arch.exe"
    "=== $arch start $(Get-Date -Format o) ===" | Out-File $out -Encoding utf8
    Remove-Item "$env:TEMP\hooktest-*" -ErrorAction SilentlyContinue
    $f = Start-Process -FilePath $fix -PassThru
    Start-Sleep -Seconds 2
    "fixture_pid=$($f.Id)" | Out-File $out -Append -Encoding utf8
    & $inj $f.Id $dll *>> $out
    "--- inject log ---" | Out-File $out -Append -Encoding utf8
    Get-Content "$env:TEMP\hooktest-inject-$($f.Id).log" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
    "--- markers ---" | Out-File $out -Append -Encoding utf8
    Get-Content "$env:TEMP\hooktest-dllmain-$($f.Id).txt","$env:TEMP\hooktest-hookproc-$($f.Id).txt" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
    Stop-Process -Id $f.Id -Force -ErrorAction SilentlyContinue
    "=== $arch end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8
}

function RunAsura($out) {
    $asura = Get-Process -Name asura -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $asura) { "ERROR: Asura not running" | Out-File $out -Append -Encoding utf8; return }
    $pid_t = $asura.Id
    "asura_pid=$pid_t" | Out-File $out -Append -Encoding utf8
    Remove-Item "$env:TEMP\hooktest-dllmain-$pid_t.txt","$env:TEMP\hooktest-hookproc-$pid_t.txt" -ErrorAction SilentlyContinue
    & (Join-Path $dir 'inject32.exe') $pid_t (Join-Path $dir 'hookdll32.dll') *>> $out
    "--- inject log ---" | Out-File $out -Append -Encoding utf8
    Get-Content "$env:TEMP\hooktest-inject-$pid_t.log" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
    "--- markers ---" | Out-File $out -Append -Encoding utf8
    Get-Content "$env:TEMP\hooktest-dllmain-$pid_t.txt","$env:TEMP\hooktest-hookproc-$pid_t.txt" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
}

RunFixture 64 (Join-Path $dir 'result7.txt')
RunFixture 32 (Join-Path $dir 'result9.txt')
"=== stopping WeGame driver $(Get-Date -Format o) ===" | Out-File (Join-Path $dir 'result6.txt') -Append -Encoding utf8
sc.exe stop WeGameProcService | Out-File (Join-Path $dir 'result6.txt') -Append -Encoding utf8
Start-Sleep -Seconds 2
RunAsura (Join-Path $dir 'result6.txt')