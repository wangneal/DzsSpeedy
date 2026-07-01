$ErrorActionPreference = 'SilentlyContinue'
$target = 'E:\projects\gamescript\OpenSpeedy\src-tauri\target\debug\speedpatch32.dll'
Get-Process | ForEach-Object {
  $p = $_
  foreach ($m in $p.Modules) {
    if ($m.FileName -eq $target) {
      Write-Host ("PID={0} Name={1} Path={2}" -f $p.Id, $p.ProcessName, $p.Path)
      break
    }
  }
}
