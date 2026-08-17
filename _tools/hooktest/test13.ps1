# 管理员运行：复制 hookdll32 到 System32 并跑三路远程加载对照
$dir = 'D:\DzsSpeedy\_tools\hooktest'
$out = Join-Path $dir 'result13.txt'
"=== start $(Get-Date -Format o) ===" | Out-File $out -Encoding utf8
Copy-Item (Join-Path $dir 'hookdll32.dll') 'C:\Windows\SysWOW64\hookdll32.dll' -Force
"copied to SysWOW64" | Out-File $out -Append -Encoding utf8
Remove-Item "$env:TEMP\hooktest-dllmain-6464.txt","$env:TEMP\hooktest-hookproc-6464.txt" -ErrorAction SilentlyContinue
& (Join-Path $dir 'remload2.exe') 6464 *>> $out
Get-Content "$env:TEMP\hooktest-dllmain-6464.txt","$env:TEMP\hooktest-hookproc-6464.txt" -ErrorAction SilentlyContinue | Out-File $out -Append -Encoding utf8
"=== end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8