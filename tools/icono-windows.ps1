# Windows: genera packaging/io.github.garbanhh.Garbo.png (256 px, avisos del escritorio) y
# packaging/io.github.garbanhh.Garbo.ico (16 a 256 px, ejecutable e instalador) dibujando el icono
# vectorial de Linux (packaging/io.github.garbanhh.Garbo.svg): un rectángulo redondeado y polígonos
# (M x y x y … Z), con relleno par-impar. Sin dependencias: System.Drawing.
#   powershell -File tools\icono-windows.ps1
Add-Type -AssemblyName System.Drawing
$root = Split-Path $PSScriptRoot -Parent
$svg = [xml](Get-Content -Raw (Join-Path $root "packaging\io.github.garbanhh.Garbo.svg"))

function Render([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)
    $k = $size / 128.0
    $g.ScaleTransform($k, $k)
    # Fondo: <rect x y width height rx fill>
    $r = $svg.svg.rect
    $x = [double]$r.x; $y = [double]$r.y; $w = [double]$r.width; $h = [double]$r.height; $rx = [double]$r.rx
    $bg = New-Object System.Drawing.Drawing2D.GraphicsPath
    $bg.AddArc($x, $y, 2 * $rx, 2 * $rx, 180, 90)
    $bg.AddArc($x + $w - 2 * $rx, $y, 2 * $rx, 2 * $rx, 270, 90)
    $bg.AddArc($x + $w - 2 * $rx, $y + $h - 2 * $rx, 2 * $rx, 2 * $rx, 0, 90)
    $bg.AddArc($x, $y + $h - 2 * $rx, 2 * $rx, 2 * $rx, 90, 90)
    $bg.CloseFigure()
    $g.FillPath((New-Object System.Drawing.SolidBrush([System.Drawing.ColorTranslator]::FromHtml($r.fill))), $bg)
    # Figuras: <path fill d="M x y x y … Z M …">
    foreach ($p in $svg.svg.path) {
        $gp = New-Object System.Drawing.Drawing2D.GraphicsPath([System.Drawing.Drawing2D.FillMode]::Alternate)
        foreach ($sub in ($p.d -split 'M' | Where-Object { $_.Trim() })) {
            $nums = ($sub -replace 'Z', '').Trim() -split '[\s,]+' | ForEach-Object { [single]$_ }
            $pts = for ($i = 0; $i + 1 -lt $nums.Count; $i += 2) { New-Object System.Drawing.PointF($nums[$i], $nums[$i + 1]) }
            $gp.AddPolygon([System.Drawing.PointF[]]$pts)
        }
        $g.FillPath((New-Object System.Drawing.SolidBrush([System.Drawing.ColorTranslator]::FromHtml($p.fill))), $gp)
    }
    $g.Dispose()
    return $bmp
}

function PngBytes($bmp) {
    $ms = New-Object IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    # La coma evita que PowerShell desenrolle el array al devolverlo.
    return , $ms.ToArray()
}

$png = Join-Path $root "packaging\io.github.garbanhh.Garbo.png"
[IO.File]::WriteAllBytes($png, (PngBytes (Render 256)))

# ICO con entradas PNG (admitidas desde Windows Vista).
$sizes = 16, 20, 24, 32, 40, 48, 64, 256
$images = New-Object System.Collections.Generic.List[byte[]]
foreach ($s in $sizes) { $images.Add((PngBytes (Render $s))) }
$ico = New-Object IO.MemoryStream
$bw = New-Object IO.BinaryWriter($ico)
$bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]; $b = if ($s -ge 256) { 0 } else { $s }
    $bw.Write([byte]$b); $bw.Write([byte]$b); $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([uint16]1); $bw.Write([uint16]32)
    $bw.Write([uint32]$images[$i].Length); $bw.Write([uint32]$offset)
    $offset += $images[$i].Length
}
foreach ($img in $images) { $bw.Write($img) }
$bw.Flush()
[IO.File]::WriteAllBytes((Join-Path $root "packaging\io.github.garbanhh.Garbo.ico"), $ico.ToArray())
"PNG e ICO generados en packaging/"
