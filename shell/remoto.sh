# garbo: arranque del shell del servidor en una sesión por ssh o mosh (lo manda ssh.sh).
#
# Lo ejecuta `sh` en el servidor. Deja la integración de Garbo (la misma que en este equipo) en una
# carpeta temporal y abre tu shell de inicio de sesión con ella, como lo abriría ssh: bash, zsh o
# fish. Ese shell borra la carpeta al cargarla: no queda nada instalado. Con otro shell, o si algo
# falla, tu shell de siempre. Garbo pone aquí los scripts al arrancar (ver `shell::remote_boot`).

# Desde PowerShell (garbo.ps1) llega con IFS vacío y sin comodines (`set -f`), para leerlo sin
# comillas dobles: todo como siempre.
unset IFS
set +f
s=${SHELL:-/bin/sh}
t=$(umask 077 && mktemp -d "${TMPDIR:-/tmp}/garbo-ssh.XXXXXX" 2>/dev/null) || exec "$s" -l
GARBO_REMOTE_TMP=$t
export GARBO_REMOTE_TMP
case ${s##*/} in
    bash)
        # Un shell de inicio de sesión no lee --rcfile: este lee lo mismo que él (ver garbo.bash).
        {
            echo '__garbo_login=1'
            cat <<'__GARBO_FIN__'
@GARBO_BASH@
__GARBO_FIN__
            echo 'builtin command rm -rf -- "$GARBO_REMOTE_TMP"; builtin unset GARBO_REMOTE_TMP'
        } >"$t/rc" && exec "$s" --rcfile "$t/rc" -i
        ;;
    zsh)
        mkdir "$t/zsh" &&
            cat >"$t/garbo.zsh" <<'__GARBO_FIN__' &&
@GARBO_ZSH@
__GARBO_FIN__
            cat >"$t/zsh/.zshenv" <<'__GARBO_FIN__' &&
@GARBO_ZSHENV@
__GARBO_FIN__
            ZDOTDIR=$t/zsh && export ZDOTDIR && exec "$s" -l
        ;;
    fish)
        cat >"$t/garbo.fish" <<'__GARBO_FIN__' &&
@GARBO_FISH@
__GARBO_FIN__
            exec "$s" -l --init-command \
                'source $GARBO_REMOTE_TMP/garbo.fish; rm -rf -- $GARBO_REMOTE_TMP; set -e GARBO_REMOTE_TMP'
        ;;
esac
rm -rf -- "$t"
unset GARBO_REMOTE_TMP
exec "$s" -l
