# wprobe-run.ps1 — run the wprobe injection experiment against a live target.
# Usage: wprobe-run.ps1 <pid> [tag]
$ErrorActionPreference = 'Continue'
$dir = 'D:\DzsSpeedy\_tools\hooktest\wprobe\x86'
$out = 'D:\DzsSpeedy\_tools\hooktest\result-wprobe.txt'
$pid_ = [int]$args[0]
$tag = if ($args[1]) { $args[1] } else { 'run' }
"=== wprobe $tag pid=$pid_ $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8
foreach ($hook in @('msg','keyboard','cbt','shell')) {
    $r = & "$dir\wprobe.exe" $pid_ $hook visible 2>&1
    "$hook -> $r" | Out-File $out -Append -Encoding utf8
    Write-Output "$hook -> $r"
    Start-Sleep -Seconds 2
}
"=== end $tag $(Get-Date -Format o) ===" | Out-File $out -Append -Encoding utf8
