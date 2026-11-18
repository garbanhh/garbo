# garbo: integración con bash para los bloques de comandos.
# garbo lanza bash con `--rcfile` apuntando aquí: primero se carga tu ~/.bashrc como
# siempre y después se añaden las marcas OSC 133 (invisibles) que delimitan cada comando:
#   A = empieza el prompt   B = empieza el comando   C = empieza la salida   D;n = terminó (código n)

if [[ -f ~/.bashrc ]]; then
    builtin source ~/.bashrc
fi

if [[ $- == *i* && -z "${__garbo_loaded:-}" ]]; then
    __garbo_loaded=1

    # Primero en PROMPT_COMMAND: captura el código de salida real y lo conserva.
    __garbo_precmd() {
        local s=$?
        builtin printf '\e]133;D;%s\a' "$s"
        return $s
    }

    # Último en PROMPT_COMMAND: (re)envuelve PS1 por si otro script lo regenera (starship...).
    __garbo_wrap_ps1() {
        if [[ "$PS1" != *'133;A'* ]]; then
            PS1='\[\e]133;A\a\]'"$PS1"'\[\e]133;B\a\]'
        fi
    }

    if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
        PROMPT_COMMAND=(__garbo_precmd "${PROMPT_COMMAND[@]}" __garbo_wrap_ps1)
    else
        PROMPT_COMMAND="__garbo_precmd${PROMPT_COMMAND:+; $PROMPT_COMMAND}; __garbo_wrap_ps1"
    fi

    # PS0 se imprime justo antes de ejecutar cada comando (bash >= 4.4).
    PS0='\e]133;C\a'"${PS0:-}"
fi
