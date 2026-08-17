# pe-exports.ps1 — list PE export names of a DLL.
param([string]$Path)
$fs = [System.IO.File]::OpenRead($Path)
$br = New-Object System.IO.BinaryReader($fs)
try {
    $fs.Seek(0x3c, 'Begin') | Out-Null
    $peOff = $br.ReadInt32()
    $fs.Seek($peOff, 'Begin') | Out-Null
    $sig = $br.ReadUInt32()  # 0x00004550
    $fs.Seek($peOff + 4 + 20, 'Begin') | Out-Null
    $magic = $br.ReadUInt16()  # 0x10b PE32 / 0x20b PE32+
    $ddOff = if ($magic -eq 0x10b) { $peOff + 4 + 20 + 96 } else { $peOff + 4 + 20 + 112 }
    $fs.Seek($ddOff, 'Begin') | Out-Null
    $exportRva = $br.ReadUInt32()
    $exportSize = $br.ReadUInt32()
    if ($exportRva -eq 0) { "no exports"; return }
    # section table
    $numSec = [BitConverter]::ToUInt16($br.ReadBytes(2), 0)  # placeholder; read properly below
    $fs.Seek($peOff + 4 + 20, 'Begin') | Out-Null
    $numSec = $br.ReadUInt16()
    $optSize = $br.ReadUInt16()
    $fs.Seek($peOff + 4 + 20 + 2 + 2 + $optSize, 'Begin') | Out-Null
    $sections = @()
    for ($i = 0; $i -lt $numSec; $i++) {
        $name = [System.Text.Encoding]::ASCII.GetString($br.ReadBytes(8)).TrimEnd([char]0)
        $vsize = $br.ReadUInt32(); $vaddr = $br.ReadUInt32()
        $rsize = $br.ReadUInt32(); $raddr = $br.ReadUInt32()
        $br.ReadBytes(16) | Out-Null
        $sections += @{ name = $name; vaddr = $vaddr; vsize = $vsize; raddr = $raddr; rsize = $rsize }
    }
    function RvaToOff($rva) {
        foreach ($s in $sections) {
            $span = [Math]::Max($s.vsize, $s.rsize)
            if ($rva -ge $s.vaddr -and $rva -lt ($s.vaddr + $span)) {
                return $s.raddr + ($rva - $s.vaddr)
            }
        }
        return -1
    }
    $expOff = RvaToOff $exportRva
    if ($expOff -lt 0) { "export dir not in sections"; return }
    $fs.Seek($expOff, 'Begin') | Out-Null
    $br.ReadUInt32() | Out-Null; $br.ReadUInt32() | Out-Null  # characteristics, timestamp
    $br.ReadUInt16() | Out-Null; $br.ReadUInt16() | Out-Null  # major/minor
    $nameRva = $br.ReadUInt32()
    $base = $br.ReadUInt32()
    $numFuncs = $br.ReadUInt32()
    $numNames = $br.ReadUInt32()
    $addrFuncsRva = $br.ReadUInt32()
    $addrNamesRva = $br.ReadUInt32()
    $addrOrdRva = $br.ReadUInt32()
    "dll name: <see below>"  # placeholder
    $fs.Seek((RvaToOff $nameRva), 'Begin') | Out-Null
    $nameBytes = New-Object System.Collections.Generic.List[byte]
    while ($true) { $b = $br.ReadByte(); if ($b -eq 0) { break }; $nameBytes.Add($b) }
    "dll: $([System.Text.Encoding]::ASCII.GetString($nameBytes.ToArray()))  functions=$numFuncs names=$numNames"
    $namesOff = RvaToOff $addrNamesRva
    for ($i = 0; $i -lt $numNames; $i++) {
        $fs.Seek($namesOff + $i * 4, 'Begin') | Out-Null
        $nRva = $br.ReadUInt32()
        $nOff = RvaToOff $nRva
        $fs.Seek($nOff, 'Begin') | Out-Null
        $nb = New-Object System.Collections.Generic.List[byte]
        while ($true) { $b = $br.ReadByte(); if ($b -eq 0) { break }; $nb.Add($b) }
        [System.Text.Encoding]::ASCII.GetString($nb.ToArray())
    }
} finally {
    $br.Dispose(); $fs.Dispose()
}