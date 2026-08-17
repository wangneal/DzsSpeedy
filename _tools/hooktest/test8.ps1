# 管理员 PowerShell 运行：【360 防护关闭后】用真实 bridge32 重新注入 Asura，验证能否 ENABLED
# powershell -NoProfile -ExecutionPolicy Bypass -File D:\DzsSpeedy\_tools\hooktest\test8.ps1
$ErrorActionPreference = 'Continue'
$dir = 'D:\DzsSpeedy\_tools\hooktest'
$out = Join-Path $dir 'result8.txt'
"=== start $(Get-Date -Format o) ===" | Out-File $out -Encoding utf8

$asura = Get-Process -Name asura -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $asura) { "ERROR: Asura not running" | Out-File $out -Append -Encoding utf8; exit 1 }
$pid_t = $asura.Id
"asura_pid=$pid_t" | Out-File $out -Append -Encoding utf8

$bridge = 'D:\Program Files\DzsSpeedy\bridge32.exe'
$existing = Get-Process -Name bridge32 -ErrorAction SilentlyContinue
if (-not $existing) {
    Start-Process -FilePath $bridge | Out-Null
    Start-Sleep -Seconds 3
}

$pipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', 'DzsSpeedyBridge32', [System.IO.Pipes.PipeDirection]::InOut)
try { $pipe.Connect(10000) } catch { "connect_failed=$_" | Out-File $out -Append -Encoding utf8; exit 1 }
$sr = New-Object System.IO.StreamReader($pipe)
$sw = New-Object System.IO.StreamWriter($pipe)
$sw.NewLine = "`n"
$sw.AutoFlush = $true
function Send($cmd) {
    $sw.WriteLine($cmd)
    $line = $sr.ReadLine()
    "[$(Get-Date -Format HH:mm:ss)] $cmd -> $line" | Out-File $out -Append -Encoding utf8
    return $line
}

Send 'GETSPEED'
Send "EJECT $pid_t"
Send "INJECT $pid_t"
$done = $false
for ($i = 0; $i -lt 30; $i++) {
    Start-Sleep -Seconds 2
    $r = Send "STATUS $pid_t"
    if ($r -match 'ENABLED|FAILED') { $done = $true; break }
}
Send "EJECT $pid_t"
$pipe.Dispose()

"--- speedpatch log ---" | Out-File $out -Append -Encoding utf8
Get-ChildItem "$env:TEMP\dzsspeedy-speedpatch-$pid_t.log" -ErrorAction SilentlyContinue | Select-Object Name, Length, LastWriteTime | Out-File $out -Append -Encoding utf8
Get-Content "$env:TEMP\dzsspeedy-speedpatch-$pid_t.log" -Tail 30 -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
"=== end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8