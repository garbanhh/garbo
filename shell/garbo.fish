# garbo: integración con fish para los bloques de comandos. Garbo arranca fish con
# `--init-command`, que carga este archivo después de tu config.fish. Marcas OSC 133 (invisibles):
#   A = empieza el prompt   B = empieza el comando   C = empieza la salida   D;n = terminó (código n)
#   P;k=r = empieza el prompt derecho; la B que lo cierra no mueve el inicio del comando.
# fish 4 ya emite A, B, C y D por su cuenta; con fish 3 se añaden aquí. El prompt derecho se marca
# siempre, para que Garbo no lo tome por parte del comando. Con GARBO_PROMPT=1 (opción "Prompt de
# Garbo") el prompt es el de Garbo: una línea con la carpeta, que Garbo dibuja como etiqueta
# (`A;garbo` lo anuncia), y debajo lo que se escribe. Con GARBO_AUTOSUGGEST=1 (opción
# "Sugerencias") se apagan las sugerencias propias de fish.

status is-interactive; or return
set -q __garbo_loaded; and return
set -g __garbo_loaded 1

# Con las sugerencias de Garbo activadas, las de fish (texto gris detrás del cursor) se apagan en
# esta sesión: ocuparían el mismo sitio y Garbo las tomaría por texto escrito.
if test -n "$GARBO_AUTOSUGGEST"
    set -g fish_autosuggestion_enabled 0
end

# fish 3 no marca el prompt por sí mismo.
set -l own_marks
string match -qr '^[0-3]\.' -- $version; and set own_marks 1

function __garbo_prompt_line
    set -l dir $PWD
    if test "$PWD" = "$HOME"
        set dir '~'
    else if string match -q -- "$HOME/*" "$PWD"
        set dir '~'(string sub -s (math (string length -- "$HOME") + 1) -- "$PWD")
    end
    set -l venv
    set -q VIRTUAL_ENV[1]; and set venv '  ('(string replace -r '.*/' '' -- $VIRTUAL_ENV)')'
    printf '\e]133;A;garbo\a  %s%s\n' $dir "$venv"
end

if test -n "$GARBO_PROMPT"
    if set -q own_marks[1]
        function fish_prompt
            __garbo_prompt_line
            printf '\e]133;B\a'
        end
    else
        # fish toma la salida del prompt como una lista de líneas y quita el salto final: con
        # otro más, la última línea queda vacía y lo escrito empieza en la primera columna.
        function fish_prompt
            __garbo_prompt_line
            echo
        end
    end
else if set -q own_marks[1]
    functions -q fish_prompt; and functions -c fish_prompt __garbo_user_prompt
    function fish_prompt
        printf '\e]133;A\a'
        functions -q __garbo_user_prompt; and __garbo_user_prompt
        printf '\e]133;B\a'
    end
end

if set -q own_marks[1]
    function __garbo_preexec --on-event fish_preexec
        printf '\e]133;C\a'
    end
    function __garbo_postexec --on-event fish_postexec
        printf '\e]133;D;%s\a' $status
    end
end

if functions -q fish_right_prompt
    functions -c fish_right_prompt __garbo_user_right_prompt
    function fish_right_prompt
        printf '\e]133;P;k=r\a'
        __garbo_user_right_prompt
        printf '\e]133;B\a'
    end
end
