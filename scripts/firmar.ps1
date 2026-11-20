param([Parameter(Mandatory)] [string[]]$Archivos)
# Windows: firma de código (Authenticode) con signtool y sello de tiempo. El certificado va en dos
# secretos: GARBO_WINDOWS_CERT (el .pfx en base64) y GARBO_WINDOWS_CERT_PASSWORD. Sin ellos no
# hace nada: Garbo se publica sin firmar (el Control inteligente de aplicaciones de Windows 11 lo
# bloqueará y SmartScreen avisará). Ver docs/Procesos/Publicar una versión.md.
$ErrorActionPreference = "Stop"
if (-not $env:GARBO_WINDOWS_CERT) {
    "Sin certificado de firma de Windows: sin firmar."
    return
}
$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
    Sort-Object FullName | Select-Object -Last 1
if (-not $signtool) { throw "No se encuentra signtool.exe (Windows SDK)" }
$pfx = Join-Path ([IO.Path]::GetTempPath()) ("garbo-firma-" + [guid]::NewGuid() + ".pfx")
[IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:GARBO_WINDOWS_CERT))
try {
    foreach ($f in $Archivos) {
        & $signtool.FullName sign /fd sha256 /tr http://timestamp.digicert.com /td sha256 `
            /f $pfx /p $env:GARBO_WINDOWS_CERT_PASSWORD $f
        if ($LASTEXITCODE -ne 0) { throw "No se pudo firmar $f" }
        & $signtool.FullName verify /pa $f
        if ($LASTEXITCODE -ne 0) { throw "La firma de $f no se valida" }
    }
} finally {
    Remove-Item $pfx -Force -ErrorAction SilentlyContinue
}
