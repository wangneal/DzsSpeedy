# Regression harness for the "injection spins forever" bug.
# Deterministic repro: a windowless fixture whose threads have message queues
# but never pump again — hook accepted, callback never fires, DLL never loads.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File test-nopump.ps1 <x86|x64> <bridge-exe>
#
# Verdicts (grep-able):
#   RED_SPIN   - STATUS stayed INITIALIZING for the whole observation window
#                (old bridge: eternal spinner; the bug).
#   GREEN_FAST_FAIL - INJECT errored quickly with the DLL-not-loaded probe
#                message and STATUS reached FAILED within the deadline.
#   PASS_*     - other correct outcomes (e.g. fixture unexpectedly injected).
$ErrorActionPreference = 'Continue'
$dir = 'D:\DzsSpeedy\_tools\hooktest'
$arch = $args[0]
$bridge = $args[1]
if (-not $arch -or -not $bridge) { Write-Error 'usage: test-nopump.ps1 <x86|x64> <bridge-exe>'; exit 2 }
$pipeName = if ($arch -eq 'x86') { 'DzsSpeedyBridge32' } else { 'DzsSpeedyBridge64' }
$bridgeName = if ($arch -eq 'x86') { 'bridge32' } else { 'bridge64' }
$out = Join-Path $dir "result-nopump-$arch.txt"

"=== nopump start $(Get-Date -Format o) arch=$arch bridge=$bridge ===" | Out-File $out -Encoding utf8

# stale bridge of this arch must not serve the pipe
Get-Process -Name $bridgeName -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

if ($arch -eq 'x86') { $fix = Join-Path $dir 'nopump32.exe' } else { $fix = Join-Path $dir 'nopump64.exe' }
$f = Start-Process -FilePath $fix -RedirectStandardOutput (Join-Path $dir 'nopump-stdout.txt') -PassThru
"fixture_pid=$($f.Id)" | Out-File $out -Append -Encoding utf8
Start-Sleep -Seconds 2

$bp = Start-Process -FilePath $bridge -PassThru
"spawned_bridge pid=$($bp.Id)" | Out-File $out -Append -Encoding utf8

$pipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', $pipeName, [System.IO.Pipes.PipeDirection]::InOut)
try { $pipe.Connect(15000) } catch { "connect_failed=$_" | Out-File $out -Append -Encoding utf8; exit 1 }
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

Send 'GETSPEED' | Out-Null
$t0 = [System.Diagnostics.Stopwatch]::StartNew()
$inj = Send "INJECT $($f.Id)"
$injMs = $t0.ElapsedMilliseconds
"inject_elapsed_ms=$injMs" | Out-File $out -Append -Encoding utf8

$verdict = 'UNKNOWN'
if ($inj -match 'ERROR INJECTION_PENDING') {
    # Observe the status window; the bug is STATUS staying INITIALIZING forever.
    $stayed = 0
    for ($i = 0; $i -lt 20; $i++) {
        Start-Sleep -Seconds 2
        $r = Send "STATUS $($f.Id)"
        if ($r -match 'OK INITIALIZING') { $stayed += 2 }
        elseif ($r -match 'FAILED') { $verdict = 'GREEN_FAILED'; break }
        elseif ($r -match 'ENABLED|DISABLED') { $verdict = 'PASS_TERMINAL'; break }
    }
    if ($verdict -eq 'UNKNOWN') {
        if ($stayed -ge 30) { $verdict = 'RED_SPIN' } else { $verdict = 'UNKNOWN' }
    }
    "status_stayed_initializing_s=$stayed" | Out-File $out -Append -Encoding utf8
} elseif ($inj -match 'not loaded into the target within') {
    $verdict = 'GREEN_FAST_FAIL'
} elseif ($inj -match 'OK') {
    $verdict = 'PASS_INJECTED'
} else {
    $verdict = "OTHER_ERROR"
}

Send "EJECT $($f.Id)" | Out-Null
Send 'SHUTDOWN' | Out-Null
Start-Sleep -Seconds 2
$pipe.Dispose()
Stop-Process -Id $f.Id -Force -ErrorAction SilentlyContinue

"VERDICT=$verdict" | Out-File $out -Append -Encoding utf8
"=== nopump end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8
Write-Output "VERDICT=$verdict"