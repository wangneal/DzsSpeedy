$ErrorActionPreference = 'Continue'
$dir = 'D:\DzsSpeedy\_tools\hooktest'
$out = Join-Path $dir 'result2.txt'
"=== start $(Get-Date -Format o) ===" | Out-File $out -Encoding utf8

# clean stale markers
Remove-Item "$env:TEMP\hooktest-*" -ErrorAction SilentlyContinue

$p = Start-Process -FilePath (Join-Path $dir 'fixture64.exe') -PassThru
Start-Sleep -Seconds 2
"fixture_pid=$($p.Id)" | Out-File $out -Append -Encoding utf8

$inj = Join-Path $dir 'inject64.exe'
$dll = Join-Path $dir 'hookdll64.dll'
& $inj "$($p.Id)" $dll *>> $out
"--- tid file ---" | Out-File $out -Append -Encoding utf8
Get-Content "$env:TEMP\hooktest-tid-$($p.Id).txt" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
"--- markers ---" | Out-File $out -Append -Encoding utf8
Get-ChildItem "$env:TEMP\hooktest-*.txt" -ErrorAction SilentlyContinue | Select-Object Name | Out-File $out -Append -Encoding utf8
Get-Content "$env:TEMP\hooktest-dllmain-$($p.Id).txt" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
Get-Content "$env:TEMP\hooktest-hookproc-$($p.Id).txt" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
"=== end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8