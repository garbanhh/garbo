# garbo: integración con bash para los bloques de comandos.
# garbo lanza bash con `--rcfile` apuntando aquí: primero se carga tu ~/.bashrc como
# siempre y después se añaden las marcas OSC 133 (invisibles) que delimitan cada comando:
#   A = empieza el prompt   B = empieza el comando   C = empieza la salida   D;n = terminó (código n)
# Con GARBO_PROMPT=1 (opción "Prompt de Garbo") el prompt es el de Garbo: una línea con la carpeta,
# que Garbo dibuja como etiqueta (`A;garbo` lo anuncia), y debajo, desde la primera columna, lo que
# se escribe. La A dice también qué shell es (`s=bash`), para las teclas que Garbo le manda.
# Por ssh, el servidor recibe este mismo archivo (ver remoto.sh) y lo carga como el shell de inicio
# de sesión que abriría ssh.

if [[ -n "${__garbo_login:-}" ]]; then
    builtin unset __garbo_login
    [[ -f /etc/profile ]] && builtin source /etc/profile
    for __garbo_rc in ~/.bash_profile ~/.bash_login ~/.profile; do
        if [[ -f "$__garbo_rc" ]]; then
            builtin source "$__garbo_rc"
            break
        fi
    done
    builtin unset __garbo_rc
elif [[ -f ~/.bashrc ]]; then
    builtin source ~/.bashrc
fi

if [[ $- == *i* && -z "${__garbo_loaded:-}" ]]; then
    __garbo_loaded=1

    # Primero en PROMPT_COMMAND: captura el código de salida real y lo conserva.
    __garbo_precmd() {
        local s=$?
        builtin printf '\e]133;D;%s\a' "$s"
        # Prompt de Garbo: siempre desde la primera columna. Si la salida no acabó en salto de línea
        # (`printf hola`), los espacios pasan a la línea siguiente y \r vuelve a su inicio; si ya
        # estaba al principio, no llegan al final y \r vuelve a donde estaba (como PROMPT_SP de zsh).
        if [[ -n "${GARBO_PROMPT:-}" && ${COLUMNS:-0} -gt 1 ]]; then
            builtin printf '%*s\r' $((COLUMNS - 1)) ''
        fi
        return $s
    }

    # Último en PROMPT_COMMAND: (re)envuelve PS1 por si otro script lo regenera (starship...).
    __garbo_wrap_ps1() {
        if [[ -n "${GARBO_PROMPT:-}" ]]; then
            PS1='\[\e]133;A;garbo;s=bash\a\]  \w${VIRTUAL_ENV:+  (${VIRTUAL_ENV##*/})}\n\[\e]133;B\a\]'
        elif [[ "$PS1" != *'133;A'* ]]; then
            PS1='\[\e]133;A;s=bash\a\]'"$PS1"'\[\e]133;B\a\]'
        fi
    }

    if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
        PROMPT_COMMAND=(__garbo_precmd "${PROMPT_COMMAND[@]}" __garbo_wrap_ps1)
    else
        PROMPT_COMMAND="__garbo_precmd${PROMPT_COMMAND:+; $PROMPT_COMMAND}; __garbo_wrap_ps1"
    fi

    # PS0 se imprime justo antes de ejecutar cada comando (bash >= 4.4).
    PS0='\e]133;C\a'"${PS0:-}"

    # ssh y mosh con Garbo también en el servidor (ver ssh.sh, que solo está en este equipo: en el
    # servidor no se prepara otro ssh). No pisa un alias ni una función tuyos.
    if [[ -r "${BASH_SOURCE[0]%/*}/ssh.sh" ]]; then
        __garbo_ssh_sh="${BASH_SOURCE[0]%/*}/ssh.sh"
        if ! builtin alias ssh &>/dev/null && ! builtin declare -F ssh >/dev/null; then
            ssh() { command sh "$__garbo_ssh_sh" ssh "$@"; }
        fi
        if ! builtin alias mosh &>/dev/null && ! builtin declare -F mosh >/dev/null; then
            mosh() { command sh "$__garbo_ssh_sh" mosh "$@"; }
        fi
    fi
fi
