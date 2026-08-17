# sign-speedpatch.ps1 - self-contained code-signing for DzsSpeedy DLLs.
# Strategy (verified against Tencent TerSafe): the anti-cheat validates the
# Authenticode trust chain of any DLL injected via SetWindowsHookEx. We build
# our own CA, install its root into the machine TRUSTED ROOT store, then sign
# the speedpatch DLLs. Standard WinVerifyTrust then passes and the injection
# is allowed. No commercial certificate needed.
#
# Usage: sign-speedpatch.ps1 [-Dll <path>...] [-Years <int>] [-OutDir <dir>]
#   Defaults: signs speedpatch32.dll + speedpatch64.dll in the DzsSpeedy
#   install dir; stores keys under _tools\codesign\.
# Requires: admin (LocalMachine\Root import), osslsigncode on PATH.
param(
    [string[]]$Dll,
    [int]$Years = 10,
    [string]$OutDir = 'D:\DzsSpeedy\_tools\codesign'
)
$ErrorActionPreference = 'Stop'

$installDir = 'D:\Program Files\DzsSpeedy'
if (-not $Dll -or $Dll.Count -eq 0) {
    $Dll = @((Join-Path $installDir 'speedpatch32.dll'), (Join-Path $installDir 'speedpatch64.dll'))
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$rootName = 'DzsSpeedy Root CA'
$signName = 'DzsSpeedy Code Signing'
$pfxPass = 'DzsSpeedySign2026!'

# 1. Root CA (self-signed, trusted via LocalMachine\Root import)
$root = Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.Subject -eq "CN=$rootName" } | Select-Object -First 1
if (-not $root) {
    $root = New-SelfSignedCertificate -Type Custom -Subject "CN=$rootName" `
        -KeyExportPolicy Exportable -KeySpec Signature -KeyUsage CertSign, CrlSign, DigitalSignature `
        -NotAfter (Get-Date).AddYears([Math]::Max($Years * 2, 20)) -TextExtension @('2.5.29.19={text}CA=1&pathlength=1') `
        -FriendlyName 'DzsSpeedy Root CA'
    "root created: $($root.Thumbprint)"
} else {
    "root exists: $($root.Thumbprint)"
}
$rootCer = Join-Path $OutDir 'dzsspeedy-root.cer'
Export-Certificate -Cert $root -FilePath $rootCer -Force | Out-Null

# 2. Ensure root is in the machine TRUSTED ROOT store (idempotent)
$installed = Get-ChildItem Cert:\LocalMachine\Root | Where-Object { $_.Thumbprint -eq $root.Thumbprint }
if (-not $installed) {
    Import-Certificate -FilePath $rootCer -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
    "root installed into LocalMachine\Root"
} else {
    "root already trusted"
}

# 3. Code-signing cert issued by our root
$sign = Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.Subject -eq "CN=$signName" } | Select-Object -First 1
if (-not $sign) {
    $sign = New-SelfSignedCertificate -Type CodeSigningCert -Subject "CN=$signName" `
        -Signer $root -KeyExportPolicy Exportable -KeySpec Signature `
        -NotAfter (Get-Date).AddYears($Years) -FriendlyName 'DzsSpeedy Code Signing'
    "signing cert created: $($sign.Thumbprint)"
} else {
    "signing cert exists: $($sign.Thumbprint)"
}
$pfx = Join-Path $OutDir 'dzsspeedy-sign.pfx'
Export-PfxCertificate -Cert $sign -FilePath $pfx -Password (ConvertTo-SecureString $pfxPass -AsPlainText -Force) -Force | Out-Null

# 4. Sign each DLL (osslsigncode; RFC3161 timestamp optional if network allows)
$ossl = (Get-Command osslsigncode -ErrorAction SilentlyContinue).Source
if (-not $ossl) { $ossl = 'C:\ProgramData\chocolatey\lib\osslsigncode\tools\bin\osslsigncode.exe' }
foreach ($d in $Dll) {
    if (-not (Test-Path $d)) { "SKIP (missing): $d"; continue }
    $tmp = "$d.signed"
    & $ossl sign -pkcs12 $pfx -pass $pfxPass -h sha256 -in $d -out $tmp | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "osslsigncode failed for $d" }
    Copy-Item $d "$d.bak" -Force -ErrorAction SilentlyContinue
    Move-Item $tmp $d -Force
    $st = (Get-AuthenticodeSignature $d).Status
    "signed: $d -> $st"
}
"ALL DONE. Root cert to distribute to other machines: $rootCer"