param([string]$Exe = "target\release\garbo.exe", [string]$Backend = "", [int]$Wait = 8, [string]$Label = "")
# Windows: arranca Garbo con una configuración temporal (GARBO_CONFIG, sin buscar actualizaciones),
# espera, mide memoria y CPU en reposo de Garbo y de sus hijos (conhost y el shell) y lo cierra.
# -Backend dx12|vulkan|gl fuerza la API gráfica (WGPU_BACKEND). Ver docs/Procesos/Compilar y probar.md.
$dir = Join-Path $env:TEMP "garbo-medida"
New-Item -ItemType Directory -Force $dir | Out-Null
Set-Content -Encoding utf8 "$dir\config.toml" "check_updates = false`n"
$env:GARBO_CONFIG = "$dir\config.toml"
$env:GARBO_HISTORY = "$dir\history.tsv"
Remove-Item Env:GARBO_SCREENSHOT -ErrorAction SilentlyContinue
$env:RUST_LOG = "garbo=info"
if ($Backend) { $env:WGPU_BACKEND = $Backend } else { Remove-Item Env:WGPU_BACKEND -ErrorAction SilentlyContinue }
$p = Start-Process -FilePath $Exe -PassThru -RedirectStandardError "$dir\stderr.txt"
# Se cierra siempre al final (`finally` corre también si se corta la salida con Select-Object).
try {
Start-Sleep -Seconds $Wait
$p.Refresh()
$cpu0 = $p.TotalProcessorTime.TotalMilliseconds
Start-Sleep -Seconds 5
$p.Refresh()
$cpu1 = $p.TotalProcessorTime.TotalMilliseconds
# "Memoria" del Administrador de tareas = conjunto de trabajo privado.
$priv = (Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "IDProcess=$($p.Id)").WorkingSetPrivate
$kids = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($p.Id)"
$gpu = (Select-String -Path "$dir\stderr.txt" -Pattern 'backend: (\w+)' | Select-Object -First 1).Matches.Groups[1].Value
"{0,-14} {1,-7} WS={2,6:N1} MB  privada={3,6:N1} MB  commit={4,6:N1} MB  CPU reposo 5 s={5} ms" -f $Label, $gpu, ($p.WorkingSet64/1MB), ($priv/1MB), ($p.PrivateMemorySize64/1MB), [math]::Round($cpu1-$cpu0)
foreach ($k in $kids) {
    $kp = Get-Process -Id $k.ProcessId -ErrorAction SilentlyContinue
    $kpriv = (Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "IDProcess=$($k.ProcessId)").WorkingSetPrivate
    if ($kp) { "    hijo {0,-16} WS={1,6:N1} MB  privada={2,6:N1} MB" -f $k.Name, ($kp.WorkingSet64/1MB), ($kpriv/1MB) }
}
} finally {
    # Por su pid, nunca por nombre: no tocar otro Garbo abierto.
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Confirm:$false }
}
