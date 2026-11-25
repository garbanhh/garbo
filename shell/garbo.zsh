# garbo: integración con zsh para los bloques de comandos. La carga zsh/.zshenv antes de tu
# .zshrc. Marcas OSC 133 (invisibles) que delimitan cada comando:
#   A = empieza el prompt   B = empieza el comando   C = empieza la salida   D;n = terminó (código n)
#   P;k=r = empieza el prompt derecho (RPROMPT); la B que lo cierra no mueve el inicio del comando.
# Con GARBO_PROMPT=1 (opción "Prompt de Garbo") el prompt es el de Garbo: una línea con la carpeta,
# que Garbo dibuja como etiqueta (`A;garbo` lo anuncia), y debajo lo que se escribe.

[[ -o interactive ]] || builtin return 0
(( ${+__garbo_loaded} )) && builtin return 0
typeset -g __garbo_loaded=1

# zsh da a cada gancho el código del comando, así que el orden no importa para D.
__garbo_precmd() {
    builtin local s=$?
    builtin emulate -L zsh
    builtin print -rn -- $'\e]133;D;'"$s"$'\a'
    # El que envuelve el prompt va el último, aunque .zshrc añada después ganchos que lo rehacen
    # en cada prompt (temas).
    if [[ ${precmd_functions[-1]} != __garbo_prompt ]]; then
        precmd_functions=(${precmd_functions:#__garbo_prompt} __garbo_prompt)
    fi
}

# Las marcas van dentro del prompt (y no impresas en precmd) para que se repitan cada vez que zsh
# lo vuelve a dibujar: así sus celdas siempre quedan marcadas.
__garbo_prompt() {
    builtin emulate -L zsh
    if [[ -n ${GARBO_PROMPT-} ]]; then
        builtin local venv=
        [[ -n ${VIRTUAL_ENV-} ]] && venv="  (${${VIRTUAL_ENV:t}//\%/%%})"
        PS1=$'%{\e]133;A;garbo;s=zsh\a%}  %~'"$venv"$'\n%{\e]133;B\a%}'
    elif [[ $PS1 != *$'\e]133;A'* ]]; then
        PS1=$'%{\e]133;A;s=zsh\a%}'"$PS1"$'%{\e]133;B\a%}'
    fi
    # RPROMPT y RPS1 no siempre se reflejan el uno en el otro: se miran los dos.
    if [[ -n ${RPROMPT-} && $RPROMPT != *$'\e]133;P'* ]]; then
        RPROMPT=$'%{\e]133;P;k=r\a%}'"$RPROMPT"$'%{\e]133;B\a%}'
    fi
    if [[ -n ${RPS1-} && $RPS1 != *$'\e]133;P'* ]]; then
        RPS1=$'%{\e]133;P;k=r\a%}'"$RPS1"$'%{\e]133;B\a%}'
    fi
}

__garbo_preexec() {
    builtin print -rn -- $'\e]133;C\a'
}

typeset -ga precmd_functions preexec_functions
precmd_functions=(__garbo_precmd ${${precmd_functions:#__garbo_precmd}:#__garbo_prompt} __garbo_prompt)
preexec_functions=(${preexec_functions:#__garbo_preexec} __garbo_preexec)

# ssh y mosh con Garbo también en el servidor (ver ssh.sh, que solo está en este equipo: en el
# servidor no se prepara otro ssh). No pisa un alias ni una función tuyos.
typeset -g __garbo_ssh_sh=${${(%):-%x}:A:h}/ssh.sh
if [[ -r $__garbo_ssh_sh ]]; then
    (( ${+functions[ssh]} || ${+aliases[ssh]} )) || ssh() { command sh "$__garbo_ssh_sh" ssh "$@"; }
    (( ${+functions[mosh]} || ${+aliases[mosh]} )) || mosh() { command sh "$__garbo_ssh_sh" mosh "$@"; }
fi
