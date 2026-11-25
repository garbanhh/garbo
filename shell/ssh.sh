# garbo: `ssh` y `mosh` con Garbo también en el servidor.
#
# La integración del shell (garbo.bash, garbo.zsh, garbo.fish) define `ssh` y `mosh` como
# funciones que llaman a este script: `sh ssh.sh ssh|mosh ARGUMENTOS…`. Si es una sesión
# interactiva (en un terminal y sin orden para el servidor), conecta pidiendo al servidor que abra
# tu shell con la integración de Garbo (ver remoto.sh, que va en `remoto.b64`): bloques, prompt
# de Garbo, edición de la línea y el agente que se lance allí. Si no, el programa de siempre, tal
# cual. En el servidor no queda nada instalado.
#
# No lo hace con `command ssh`, con un alias o función `ssh` tuyos, con una orden
# (`ssh equipo ls`), con -N -f -T -W -O -G -V -Q -s, ni con `RemoteCommand`, `SessionType` o
# `RequestTTY no` en la configuración. Con GARBO_SSH vacía (Ajustes → "Garbo en sesiones ssh"),
# nunca.

prog=$1
shift
here=${0%/*}

# ¿Sesión interactiva de ssh? Las opciones pueden ir antes y después del equipo, como las lee
# ssh; lo primero que no es opción es el equipo y lo siguiente, una orden.
ssh_interactivo() {
    dest=
    while :; do
        OPTIND=1
        while getopts ':46AaCfGgKkMNnqsTtVvXxYyB:b:c:D:E:e:F:I:i:J:L:l:m:O:o:P:p:Q:R:S:W:w:' o; do
            case $o in
                N | f | G | V | T | s | W | O | Q) return 1 ;;
                o)
                    case $(printf '%s' "$OPTARG" | tr 'A-Z' 'a-z') in
                        remotecommand* | sessiontype* | requesttty*) return 1 ;;
                    esac
                    ;;
                '?' | :) return 1 ;;
            esac
        done
        shift $((OPTIND - 1))
        [ $# -eq 0 ] && break
        [ -n "$dest" ] && return 1
        dest=$1
        shift
    done
    [ -n "$dest" ]
}

# ¿Deja ~/.ssh/config abrir un shell en ese equipo? `ssh -G` lo dice sin conectar.
ssh_configurado() {
    conf=$(command ssh -G "$@" 2>/dev/null) || return 1
    ! printf '%s\n' "$conf" |
        grep -qiE '^(remotecommand |sessiontype (none|subsystem)|requesttty no)'
}

# ¿Sesión interactiva de mosh? Igual: el equipo y nada detrás.
mosh_interactivo() {
    dest=
    while [ $# -gt 0 ]; do
        case $1 in
            # Garbo pone su propio `--` antes de la orden: con uno tuyo, tal cual.
            --) return 1 ;;
            -p | --port | --ssh | --predict | --family | --server | --client | --bind-server | \
                --experimental-remote-ip)
                shift
                ;;
            -*) ;;
            *)
                [ -n "$dest" ] && return 1
                dest=$1
                ;;
        esac
        [ $# -gt 0 ] && shift
    done
    [ -n "$dest" ]
}

interactivo() {
    # Las pruebas de Garbo (GARBO_SSH_PRUEBA) no tienen terminal.
    [ -n "${GARBO_SSH_PRUEBA:-}" ] || { [ -t 0 ] && [ -t 1 ]; } || return 1
    case $prog in
        ssh) (ssh_interactivo "$@") && ssh_configurado "$@" ;;
        mosh) (mosh_interactivo "$@") ;;
        *) return 1 ;;
    esac
}

if [ -z "${GARBO_SSH:-}" ] || ! interactivo "$@" || ! b64=$(cat "$here/remoto.b64" 2>/dev/null) ||
    [ -z "$b64" ]; then
    [ -n "${GARBO_SSH_PRUEBA:-}" ] && { echo "tal cual"; exit 0; }
    exec "$prog" "$@"
fi

prompt=
[ -n "${GARBO_PROMPT:-}" ] && prompt=1
# Lo que ejecuta el servidor con tu shell de inicio de sesión (`$SHELL -c`), sea bash, zsh, fish u
# otro: solo comillas simples sin nada dentro que las cierre, que todos leen igual. Si el servidor
# no puede leerlo (sin base64), tu shell de siempre.
boot='c=$(echo '$b64' | base64 -d 2>/dev/null) && eval "$c"; exec "${SHELL:-/bin/sh}" -l'
[ -n "${GARBO_SSH_PRUEBA:-}" ] && { echo "con Garbo"; exit 0; }
case $prog in
    ssh) exec ssh -t "$@" "exec env GARBO_PROMPT=$prompt sh -c '$boot'" ;;
    # mosh pasa la orden al servidor ya separada en argumentos (sin pasar por un shell).
    mosh) exec mosh "$@" -- env "GARBO_PROMPT=$prompt" sh -c "$boot" ;;
esac
