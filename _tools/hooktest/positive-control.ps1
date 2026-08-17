# Positive control: fixture64 + bridge64 must still inject OK fast.
# Usage: positive-control.ps1 [bridge64-exe] [arch]
$ErrorActionPreference = 'Continue'
$dir = 'D:\DzsSpeedy\_tools\hooktest'
$bridge64 = if ($args[0]) { $args[0] } else { Join-Path $dir 'bridge64.exe' }
$arch = if ($args[1]) { $args[1] } else { 'x64' }
$out = Join-Path $dir 'result-positive-control-fixed.txt'
$fixName = if ($arch -eq 'x86') { 'fixture32.exe' } else { 'fixture64.exe' }
$pipeName = if ($arch -eq 'x86') { 'DzsSpeedyBridge32' } else { 'DzsSpeedyBridge64' }
$bridgeProcName = if ($arch -eq 'x86') { 'bridge32' } else { 'bridge64' }
"=== positive control $(Get-Date -Format o) bridge=$bridge64 arch=$arch ===" | Out-File $out -Encoding utf8
Get-Process -Name $bridgeProcName -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
$fix = Start-Process -FilePath (Join-Path $dir $fixName) -PassThru
Start-Sleep -Seconds 2
$bp = Start-Process -FilePath $bridge64 -PassThru
"fixture_pid=$($fix.Id) bridge_pid=$($bp.Id)" | Out-File $out -Append -Encoding utf8
$pipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', $pipeName, [System.IO.Pipes.PipeDirection]::InOut)
$pipe.Connect(15000)
$sr = New-Object System.IO.StreamReader($pipe)
$sw = New-Object System.IO.StreamWriter($pipe)
$sw.NewLine = "`n"
$sw.AutoFlush = $true
function Send($cmd) {
    $sw.WriteLine($cmd)
    return $sr.ReadLine()
}
Send 'GETSPEED' | Out-Null
$t = [System.Diagnostics.Stopwatch]::StartNew()
$r = Send "INJECT $($fix.Id)"
"INJECT elapsed_ms=$($t.ElapsedMilliseconds) -> $r" | Out-File $out -Append -Encoding utf8
Start-Sleep -Seconds 2
"STATUS -> $(Send "STATUS $($fix.Id)")" | Out-File $out -Append -Encoding utf8
Send "EJECT $($fix.Id)" | Out-Null
Send 'SHUTDOWN' | Out-Null
Start-Sleep -Seconds 2
$pipe.Dispose()
Stop-Process -Id $fix.Id -Force -ErrorAction SilentlyContinue
"=== end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8
