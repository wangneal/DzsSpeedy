# Extract ASCII + UTF-16 strings from a binary, print to console (line-buffered to a file).
param([string]$Path, [string]$Out, [int]$MinLen = 5)
$bytes = [System.IO.File]::ReadAllBytes($Path)
$sb = New-Object System.Text.StringBuilder
$utf16 = New-Object System.Text.StringBuilder
$results = New-Object System.Collections.Generic.List[string]
for ($i = 0; $i -lt $bytes.Length; $i++) {
    $b = $bytes[$i]
    # ASCII run
    if ($b -ge 0x20 -and $b -le 0x7E) {
        [void]$sb.Append([char]$b)
    } else {
        if ($sb.Length -ge $MinLen) { $results.Add($sb.ToString()) }
        [void]$sb.Clear()
    }
    # UTF-16LE run (check pair)
    if ($i + 1 -lt $bytes.Length) {
        $b2 = $bytes[$i + 1]
        if ($b2 -eq 0 -and $b -ge 0x20 -and $b -le 0x7E) {
            [void]$utf16.Append([char]$b)
        } else {
            if ($utf16.Length -ge $MinLen) { $results.Add($utf16.ToString()) }
            [void]$utf16.Clear()
        }
    }
}
if ($sb.Length -ge $MinLen) { $results.Add($sb.ToString()) }
if ($utf16.Length -ge $MinLen) { $results.Add($utf16.ToString()) }
$results | Set-Content -Path $Out -Encoding utf8
"extracted $($results.Count) strings from $Path -> $Out"