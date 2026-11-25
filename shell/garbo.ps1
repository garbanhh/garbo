# garbo: integración con PowerShell (Windows PowerShell 5.1 y PowerShell 7) para los bloques de
# comandos. Garbo arranca PowerShell con `-NoExit -Command`, que carga este archivo después de tu
# perfil (como bloque de código: no depende de la directiva de ejecución ni la cambia). Marcas
# OSC 133 (invisibles):
#   A = empieza el prompt   B = empieza el comando   C = empieza la salida   D;n = terminó (código n)
# y OSC 9;9 con la carpeta: PowerShell no cambia la del proceso con `cd` (Set-Location).
# Con GARBO_PROMPT=1 (opción "Prompt de Garbo") el prompt es el de Garbo: una línea con la carpeta,
# que Garbo dibuja como etiqueta (`A;garbo` lo anuncia), y debajo, desde la primera columna, lo que
# se escribe. Con GARBO_AUTOSUGGEST=1 (opción "Sugerencias") se apagan las predicciones de
# PSReadLine: son texto gris detrás del cursor y Garbo lo tomaría por escrito. Con GARBO_SSH=1
# (opción "Garbo en sesiones ssh"), `ssh` lleva Garbo al servidor (ver el final).

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

# ssh con Garbo también en el servidor, como ssh.sh con bash, zsh y fish: si es una sesión
# interactiva (sin orden para el servidor), conecta pidiendo al servidor que abra tu shell con la
# integración de Garbo (remoto.sh, que Garbo deja en `remoto.b64` junto a este archivo): bloques,
# prompt de Garbo, edición de la línea y el agente que se lance allí. Si no, ssh tal cual. En el
# servidor no queda nada instalado. No pisa un alias ni una función `ssh` tuyos; `ssh.exe` se la
# salta. Tal cual también con -N -f -G -V -T -s -W -O -Q, con `RemoteCommand`, `SessionType` o
# `RequestTTY` (en -o o, los tres primeros, en la configuración) y con la salida a otra orden.
if ($env:GARBO_SSH -and $global:__GarboDir -and
    -not (Get-Command ssh -CommandType Alias, Function -ErrorAction SilentlyContinue)) {

    # Las opciones pueden ir antes y después del equipo, como las lee ssh; lo primero que no es
    # opción es el equipo y lo siguiente, una orden.
    function global:__GarboSshInteractive([object[]]$a) {
        $dest = $null
        for ($i = 0; $i -lt $a.Count; $i++) {
            $w = [string]$a[$i]
            if ($w -eq '--') {
                # Tras `--`, lo siguiente es el equipo aunque empiece por `-`.
                $i++
                if ($i -ge $a.Count) { break }
                if ($dest) { return $false }
                $dest = [string]$a[$i]
                continue
            }
            if ($w.Length -lt 2 -or $w[0] -ne '-') {
                if ($dest) { return $false }
                $dest = $w
                continue
            }
            # Opciones juntas (`-tt`, `-p22`, `-oUser=ana`), distinguiendo mayúsculas.
            for ($j = 1; $j -lt $w.Length; $j++) {
                $c = [string]$w[$j]
                if ('NfGVTsWOQ'.Contains($c)) { return $false }
                if ('BbcDEeFIiJLlmOoPpQRSWw'.Contains($c)) {
                    if ($j + 1 -lt $w.Length) { $v = $w.Substring($j + 1) }
                    else {
                        $i++
                        if ($i -ge $a.Count) { return $false }
                        $v = [string]$a[$i]
                    }
                    if ($c -ceq 'o' -and $v -match '^(remotecommand|sessiontype|requesttty)') {
                        return $false
                    }
                    break
                }
                if (-not '46AaCgKkMnqtvXxYy'.Contains($c)) { return $false }
            }
        }
        [bool]$dest
    }

    # Lo que ejecuta el servidor con tu shell de inicio de sesión (`$SHELL -c`). Sin comillas
    # dobles ni barras invertidas, que Windows PowerShell no pasa bien a los programas: el script
    # va en base64 y `eval $c` lo lee entero con IFS vacío y sin comodines (remoto.sh lo deja
    # luego como siempre). Si el servidor no puede leerlo (sin base64), tu shell de siempre.
    function global:__GarboSshCommand {
        try { $b64 = [IO.File]::ReadAllText((Join-Path $global:__GarboDir 'remoto.b64')).Trim() }
        catch { return $null }
        if (-not $b64) { return $null }
        $p = if ($env:GARBO_PROMPT) { '1' } else { '' }
        $boot = 'c=$(echo ' + $b64 + ' | base64 -d 2>/dev/null) && IFS= && set -f && eval $c; ' +
            'exec ${SHELL:-/bin/sh} -l'
        "exec env GARBO_PROMPT=$p sh -c '$boot'"
    }

    function global:ssh {
        $exe = Get-Command ssh -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if (-not $exe) { ssh.exe @args; return }
        $remote = $null
        if ($env:GARBO_SSH -and $MyInvocation.PipelineLength -le 1 -and
            -not [Console]::IsInputRedirected -and -not [Console]::IsOutputRedirected -and
            (__GarboSshInteractive $args)) {
            # ¿Deja la configuración (~/.ssh/config) abrir un shell en ese equipo? `-G` lo dice
            # sin conectar.
            $conf = & $exe.Source -G @args 2>$null
            if ($LASTEXITCODE -eq 0 -and
                -not ($conf -match '^(remotecommand |sessiontype (none|subsystem)|requesttty no)')) {
                $remote = __GarboSshCommand
            }
        }
        if ($remote) { & $exe.Source -t @args $remote } else { & $exe.Source @args }
    }
}
