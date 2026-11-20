# garbo: integración con PowerShell (Windows PowerShell 5.1 y PowerShell 7) para los bloques de
# comandos. Garbo arranca PowerShell con `-NoExit -Command`, que carga este archivo después de tu
# perfil (como bloque de código: no depende de la directiva de ejecución ni la cambia). Marcas
# OSC 133 (invisibles):
#   A = empieza el prompt   B = empieza el comando   C = empieza la salida   D;n = terminó (código n)
# y OSC 9;9 con la carpeta: PowerShell no cambia la del proceso con `cd` (Set-Location).
# Con GARBO_PROMPT=1 (opción "Prompt de Garbo") el prompt es el de Garbo: una línea con la carpeta,
# que Garbo dibuja como etiqueta (`A;garbo` lo anuncia), y debajo, desde la primera columna, lo que
# se escribe. Con GARBO_AUTOSUGGEST=1 (opción "Sugerencias") se apagan las predicciones de
# PSReadLine: son texto gris detrás del cursor y Garbo lo tomaría por escrito.

if ($global:__GarboLoaded) { return }
$global:__GarboLoaded = $true
$global:__GarboUserPrompt = $function:prompt

function global:__GarboMark([string]$m) { "$([char]27)]133;$m$([char]7)" }

function global:prompt {
    # Lo primero: cualquier orden cambia $? y $LASTEXITCODE.
    $ok = $?
    $native = $global:LASTEXITCODE
    $code = 0
    if (-not $ok) {
        # Error de PowerShell en el último comando: 1; si no, el código del programa.
        $last = Get-History -Count 1
        $psError = $last -and $Error.Count -gt 0 -and $Error[0].InvocationInfo -and
            $Error[0].InvocationInfo.HistoryId -eq $last.Id
        $code = if (-not $psError -and $native) { $native } else { 1 }
    }
    $out = __GarboMark "D;$code"
    $loc = $executionContext.SessionState.Path.CurrentLocation
    if ($loc.Provider.Name -eq 'FileSystem') {
        $out += "$([char]27)]9;9;`"$($loc.ProviderPath)`"$([char]7)"
    }
    if ($env:GARBO_PROMPT) {
        # Siempre desde la primera columna: si la salida no acabó en salto de línea, los espacios
        # pasan a la línea siguiente y \r vuelve a su inicio.
        $width = $Host.UI.RawUI.WindowSize.Width
        if ($width -gt 1) { $out += (' ' * ($width - 1)) + "`r" }
        $dir = $loc.Path
        $home_ = $HOME.TrimEnd('\')
        if ($dir -eq $home_) { $dir = '~' }
        elseif ($home_ -and $dir.StartsWith("$home_\", [StringComparison]::OrdinalIgnoreCase)) {
            $dir = '~' + $dir.Substring($home_.Length)
        }
        $venv = if ($env:VIRTUAL_ENV) { "  ($(Split-Path -Leaf $env:VIRTUAL_ENV))" } else { '' }
        $out += (__GarboMark 'A;garbo') + "  $dir$venv`n" + (__GarboMark 'B')
    } else {
        $user = if ($global:__GarboUserPrompt) { & $global:__GarboUserPrompt } else { "PS $loc> " }
        $out += (__GarboMark 'A') + ($user -join '') + (__GarboMark 'B')
    }
    $global:LASTEXITCODE = $native
    $out
}

if (Get-Module PSReadLine) {
    # C: justo cuando PSReadLine entrega la línea, antes de ejecutarla.
    if (Test-Path Function:\PSConsoleHostReadLine) {
        $global:__GarboReadLine = $function:PSConsoleHostReadLine
        function global:PSConsoleHostReadLine {
            $line = & $global:__GarboReadLine
            # Con Intro en vacío no hay comando (como en bash): la D que llegue sola se ignora.
            if ($line.Trim()) { [Console]::Write((__GarboMark 'C')) }
            $line
        }
    }
    try {
        # El prompt de Garbo ocupa dos líneas: PSReadLine tiene que saberlo para repintarlo. Lo
        # escrito empieza en la primera columna, también las líneas que añade Shift+Intro (sin
        # el ">> " de continuación), como en bash.
        if ($env:GARBO_PROMPT) {
            Set-PSReadLineOption -ExtraPromptLineCount 1 -ContinuationPrompt ''
        }
        # PSReadLine 2.1 o posterior (PowerShell 7); la 2.0 de Windows PowerShell no predice.
        if ($env:GARBO_AUTOSUGGEST) { Set-PSReadLineOption -PredictionSource None }
    } catch {}
}
