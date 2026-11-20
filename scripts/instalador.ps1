param(
    # No volver a compilar (usar el garbo.exe que ya haya en target\release).
    [switch]$SinCompilar,
    # Ruta de ISCC.exe (Inno Setup 6); por defecto, la de una instalación normal.
    [string]$Iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
)
# Windows: genera en target\windows el instalador (garbo-X.Y.Z-x64-setup.exe, Inno Setup) y la
# versión portátil (garbo-X.Y.Z-x64.zip), cada uno con su .sha256. Incluye la ConPTY de Windows
# Terminal (scripts\conpty.ps1). Firmar va aparte (ver docs/Procesos/Publicar una versión.md).
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value
if (-not $version) { throw "No se encontró la versión en Cargo.toml" }

if (-not $SinCompilar) {
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw "cargo build falló" }
}
& "$PSScriptRoot\conpty.ps1" -Destino "target\release"
# Con certificado (secretos GARBO_WINDOWS_CERT*), garbo.exe firmado antes de empaquetarlo.
& "$PSScriptRoot\firmar.ps1" -Archivos "target\release\garbo.exe"

$out = Join-Path $root "target\windows"
New-Item -ItemType Directory -Force $out | Out-Null

# Portátil: garbo.exe con la ConPTY, la licencia y los avisos de terceros.
$zip = Join-Path $out "garbo-$version-x64.zip"
$stage = Join-Path $out "garbo-$version-x64"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory $stage | Out-Null
Copy-Item target\release\garbo.exe, target\release\conpty.dll, target\release\OpenConsole.exe $stage
Copy-Item LICENSE (Join-Path $stage "LICENSE.txt")
Copy-Item NOTICE (Join-Path $stage "NOTICE.txt")
Copy-Item assets\fuentes\OFL-AdwaitaMono.txt $stage
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path "$stage\*" -DestinationPath $zip
Remove-Item -Recurse -Force $stage

# Instalador.
if (-not (Test-Path $Iscc)) { throw "No se encuentra Inno Setup 6 ($Iscc)" }
& $Iscc /Qp "/DVersion=$version" "/DFuente=$root\target\release" "packaging\windows\garbo.iss"
if ($LASTEXITCODE -ne 0) { throw "Inno Setup falló" }
& "$PSScriptRoot\firmar.ps1" -Archivos (Join-Path $out "garbo-$version-x64-setup.exe")

foreach ($f in Get-ChildItem $out -File | Where-Object { $_.Extension -in '.exe', '.zip' }) {
    $hash = (Get-FileHash $f.FullName -Algorithm SHA256).Hash.ToLower()
    # Mismo formato que sha256sum: "<suma>  <archivo>".
    [IO.File]::WriteAllText("$($f.FullName).sha256", "$hash  $($f.Name)`n")
    "{0,-36} {1,8:N1} MB  {2}" -f $f.Name, ($f.Length / 1MB), $hash
}
