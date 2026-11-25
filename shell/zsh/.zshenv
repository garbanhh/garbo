# garbo: arranque de zsh. Garbo apunta ZDOTDIR a esta carpeta para cargar su integración sin
# tocar tus archivos: aquí ZDOTDIR vuelve a su valor, se carga tu .zshenv y, en las sesiones
# interactivas, la integración (garbo.zsh). Después zsh lee tu .zprofile, .zshrc y .zlogin de
# siempre, porque busca cada archivo en el ZDOTDIR del momento.

if [[ -n "${GARBO_ZSH_ZDOTDIR+X}" ]]; then
    builtin export ZDOTDIR="$GARBO_ZSH_ZDOTDIR"
    builtin unset GARBO_ZSH_ZDOTDIR
else
    builtin unset ZDOTDIR
fi

{
    builtin typeset __garbo_rc="${ZDOTDIR-$HOME}/.zshenv"
    [[ -r "$__garbo_rc" ]] && builtin source -- "$__garbo_rc"
} always {
    if [[ -o interactive ]]; then
        # %x: este archivo; la integración está en la carpeta de arriba.
        builtin source -- "${${(%):-%x}:A:h:h}/garbo.zsh"
    fi
    builtin unset __garbo_rc
    # Por ssh (ver remoto.sh): la carpeta temporal con la integración ya no hace falta.
    if [[ -n ${GARBO_REMOTE_TMP-} ]]; then
        builtin command rm -rf -- "$GARBO_REMOTE_TMP"
        builtin unset GARBO_REMOTE_TMP
    fi
}
