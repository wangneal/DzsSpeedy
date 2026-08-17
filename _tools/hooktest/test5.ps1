$ErrorActionPreference = 'Continue'
$dir = 'D:\DzsSpeedy\_tools\hooktest'
$out = Join-Path $dir 'result5.txt'
"=== start $(Get-Date -Format o) ===" | Out-File $out -Encoding utf8

Remove-Item "$env:TEMP\hooktest-dllmain-11416.txt","$env:TEMP\hooktest-hookproc-11416.txt" -ErrorAction SilentlyContinue

$inj = Join-Path $dir 'inject32.exe'
$dll = Join-Path $dir 'hookdll32.dll'
& $inj 11416 $dll *>> $out
"--- inject log ---" | Out-File $out -Append -Encoding utf8
Get-Content "$env:TEMP\hooktest-inject-11416.log" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
"--- markers ---" | Out-File $out -Append -Encoding utf8
Get-ChildItem "$env:TEMP\hooktest-*.txt" -ErrorAction SilentlyContinue | Select-Object Name, Length | Out-File $out -Append -Encoding utf8
Get-Content "$env:TEMP\hooktest-dllmain-11416.txt","$env:TEMP\hooktest-hookproc-11416.txt" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
"=== end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8