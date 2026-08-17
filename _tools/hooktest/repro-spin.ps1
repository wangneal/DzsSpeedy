# Reproduce "injection spins forever" with a bridge against a live target.
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File repro-spin.ps1 <pid> <x86|x64> [bridge-exe]
$ErrorActionPreference = 'Continue'
$dir = 'D:\DzsSpeedy\_tools\hooktest'
$targetPid = [int]$args[0]
$arch = $args[1]
if (-not $targetPid -or -not $arch) { Write-Error 'usage: repro-spin.ps1 <pid> <x86|x64> [bridge-exe]'; exit 2 }
if ($arch -eq 'x86') {
    $bridge = if ($args[2]) { $args[2] } else { 'D:\Program Files\DzsSpeedy\bridge32.exe' }
    $pipeName = 'DzsSpeedyBridge32'
    $bridgeName = 'bridge32'
} else {
    $bridge = if ($args[2]) { $args[2] } else { 'D:\Program Files\DzsSpeedy\bridge64.exe' }
    $pipeName = 'DzsSpeedyBridge64'
    $bridgeName = 'bridge64'
}
$out = Join-Path $dir 'result-repro.txt'
$logPath = "$env:TEMP\dzsspeedy-bridge.log"
$logLen0 = if (Test-Path $logPath) { (Get-Item $logPath).Length } else { 0 }

"=== repro start $(Get-Date -Format o) target_pid=$targetPid arch=$arch ===" | Out-File $out -Encoding utf8

$existing = Get-Process -Name $bridgeName -ErrorAction SilentlyContinue
if ($existing) {
    "reuse_bridge pid=$($existing.Id)" | Out-File $out -Append -Encoding utf8
} else {
    $bp = Start-Process -FilePath $bridge -PassThru
    "spawned_bridge pid=$($bp.Id)" | Out-File $out -Append -Encoding utf8
}

$pipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', $pipeName, [System.IO.Pipes.PipeDirection]::InOut)
$sw = [System.Diagnostics.Stopwatch]::StartNew()
try { $pipe.Connect(15000) } catch { "connect_failed=$_" | Out-File $out -Append -Encoding utf8; exit 1 }
"pipe_connected_ms=$($sw.ElapsedMilliseconds)" | Out-File $out -Append -Encoding utf8
$sr = New-Object System.IO.StreamReader($pipe)
$swr = New-Object System.IO.StreamWriter($pipe)
$swr.NewLine = "`n"
$swr.AutoFlush = $true
function Send($cmd) {
    $t0 = [System.Diagnostics.Stopwatch]::StartNew()
    $swr.WriteLine($cmd)
    $line = $sr.ReadLine()
    $ms = $t0.ElapsedMilliseconds
    "[$(Get-Date -Format HH:mm:ss)] $cmd -> $line (${ms}ms)" | Out-File $out -Append -Encoding utf8
    return $line
}

Send 'GETSPEED'
$t0 = [System.Diagnostics.Stopwatch]::StartNew()
$r = Send "INJECT $targetPid"
"inject_elapsed_ms=$($t0.ElapsedMilliseconds)" | Out-File $out -Append -Encoding utf8

$done = $false
for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Seconds 2
    $r = Send "STATUS $targetPid"
    if ($r -match 'ENABLED|FAILED|DISABLED') { $done = $true; break }
}
if (-not $done) { "STATUS_STAYED_INITIALIZING_FOR=$($i*2)s" | Out-File $out -Append -Encoding utf8 }

Send "EJECT $targetPid" | Out-Null
Send 'SHUTDOWN' | Out-Null
Start-Sleep -Seconds 3
$pipe.Dispose()

"--- bridge log tail (new since test start) ---" | Out-File $out -Append -Encoding utf8
if (Test-Path $logPath) {
    $fs = [System.IO.File]::Open($logPath, 'Open', 'Read', 'ReadWrite')
    $fs.Seek($logLen0, 'Begin') | Out-Null
    $rdr = New-Object System.IO.StreamReader($fs)
    while (-not $rdr.EndOfStream) { $rdr.ReadLine() | Out-File $out -Append -Encoding utf8 }
    $rdr.Dispose(); $fs.Dispose()
} else { 'no bridge log' | Out-File $out -Append -Encoding utf8 }
"=== repro end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8