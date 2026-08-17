$ErrorActionPreference = 'Continue'
$dir = 'D:\DzsSpeedy\_tools\hooktest'
$out = Join-Path $dir 'result4.txt'
"=== start $(Get-Date -Format o) ===" | Out-File $out -Encoding utf8

$bridge = 'D:\Program Files\DzsSpeedy\bridge32.exe'
$existing = Get-Process -Name bridge32 -ErrorAction SilentlyContinue
if ($existing) {
    "reuse_existing_bridge pid=$($existing.Id)" | Out-File $out -Append -Encoding utf8
} else {
    $bp = Start-Process -FilePath $bridge -Verb RunAs -PassThru
    "spawned_bridge pid=$($bp.Id)" | Out-File $out -Append -Encoding utf8
}
Start-Sleep -Seconds 3

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
Send 'INJECT 11416'
$done = $false
for ($i = 0; $i -lt 25; $i++) {
    Start-Sleep -Seconds 2
    $r = Send 'STATUS 11416'
    if ($r -match 'ENABLED|FAILED|DISABLED') { $done = $true; break }
}
if (-not $done) { Send 'EJECT 11416' }
$pipe.Dispose()

"=== bridge log tail ===" | Out-File $out -Append -Encoding utf8
Get-Content "$env:TEMP\dzsspeedy-bridge.log" -Tail 80 -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
"=== speedpatch logs ===" | Out-File $out -Append -Encoding utf8
Get-ChildItem "$env:TEMP\dzsspeedy-speedpatch-*.log" -ErrorAction SilentlyContinue | Select-Object Name, Length, LastWriteTime | Out-File $out -Append -Encoding utf8
"=== end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8