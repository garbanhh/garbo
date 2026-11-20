param(
    # Carpetas donde dejar conpty.dll y OpenConsole.exe (junto a garbo.exe).
    [string[]]$Destino = @("target\debug", "target\release"),
    [ValidateSet("x64", "arm64")] [string]$Arquitectura = "x64"
)
# Windows: la ConPTY de Windows Terminal (conpty.dll + OpenConsole.exe, MIT) junto a garbo.exe.
# alacritty_terminal la usa si está en la carpeta del ejecutable: deja pasar las secuencias en orden
# (marcas de bloques, OSC 7501) y es mucho más rápida que la de Windows (ver docs/Rendimiento.md).
# Se descarga la versión fijada del paquete NuGet oficial y se comprueban su SHA-256 y la firma de
# Microsoft de los dos archivos antes de copiarlos.
$ErrorActionPreference = "Stop"
$version = "1.25.261109001"
$sha256 = "3D77F535DF848E44225C2A8912AC844FECB23F3057732B639F3034B587496F09"

$cache = Join-Path $env:TEMP "garbo-conpty\$version"
$nupkg = Join-Path $cache "conpty.nupkg"
New-Item -ItemType Directory -Force $cache | Out-Null
if (-not (Test-Path $nupkg) -or (Get-FileHash $nupkg -Algorithm SHA256).Hash -ne $sha256) {
    $url = "https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/$version/microsoft.windows.console.conpty.$version.nupkg"
    Invoke-WebRequest $url -OutFile $nupkg -UseBasicParsing
}
$hash = (Get-FileHash $nupkg -Algorithm SHA256).Hash
if ($hash -ne $sha256) { throw "SHA-256 del paquete inesperado: $hash" }

$pkg = Join-Path $cache "pkg"
if (Test-Path $pkg) { Remove-Item -Recurse -Force $pkg }
Add-Type -AssemblyName System.IO.Compression.FileSystem
[IO.Compression.ZipFile]::ExtractToDirectory($nupkg, $pkg)
$files = @(
    (Join-Path $pkg "runtimes\win-$Arquitectura\native\conpty.dll"),
    (Join-Path $pkg "build\native\runtimes\$Arquitectura\OpenConsole.exe")
)
foreach ($f in $files) {
    $sig = Get-AuthenticodeSignature $f
    if ($sig.Status -ne "Valid" -or $sig.SignerCertificate.Subject -notmatch "O=Microsoft Corporation") {
        throw "Firma no válida en $(Split-Path -Leaf $f): $($sig.Status) $($sig.SignerCertificate.Subject)"
    }
}
foreach ($d in $Destino) {
    New-Item -ItemType Directory -Force $d | Out-Null
    Copy-Item $files $d -Force
    "ConPTY $version ($Arquitectura) en $d"
}
