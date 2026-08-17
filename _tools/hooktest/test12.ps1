# 管理员运行：恢复 WeGameProcService 为手动启动（WeGame 需要它才能拉起游戏）
$out = 'D:\DzsSpeedy\_tools\hooktest\result12.txt'
"=== start $(Get-Date -Format o) ===" | Out-File $out -Encoding utf8
sc.exe config WeGameProcService start=demand | Out-File $out -Append -Encoding utf8
$st = (Get-CimInstance Win32_SystemDriver -Filter "Name='WeGameProcService'").State
"wegame_driver_state=$st" | Out-File $out -Append -Encoding utf8
"=== end $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8