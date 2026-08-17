# 管理员运行：对照实验 —— remload2 打提权 fixture32（健康进程）
$dir = 'D:\DzsSpeedy\_tools\hooktest'
$out = Join-Path $dir 'result14.txt'
"=== start $(Get-Date -Format o) ===" | Out-File $out -Encoding utf8
Remove-Item "$env:TEMP\hooktest-*" -ErrorAction SilentlyContinue
$f = Start-Process -FilePath (Join-Path $dir 'fixture32.exe') -PassThru
Start-Sleep -Seconds 2
"fixture_pid=$($f.Id)" | Out-File $out -Append -Encoding utf8
& (Join-Path $dir 'remload2.exe') $f.Id *>> $out
Get-Content "$env:TEMP\hooktest-dllmain-$($f.Id).txt","$env:TEMP\hooktest-hookproc-$($f.Id).txt" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
Stop-Process -Id $f.Id -Force -ErrorAction SilentlyContinue
"=== end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8