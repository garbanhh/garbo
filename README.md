# Garbo

Garbo es un emulador de terminal acelerado por GPU, escrito en Rust y diseñado para un consumo
mínimo de recursos. Ofrece pestañas, bloques de comandos, paleta de comandos, temas y un panel de
ajustes con persistencia automática.

## Características

- Renderizado por GPU (wgpu: Vulkan, Metal, DX12 u OpenGL) con redibujado únicamente bajo demanda.
- Pestañas integradas en la barra de título, con herencia del directorio de trabajo. Cada pestaña
  se titula con lo que se está haciendo: el título del programa, la orden en ejecución o la carpeta.
- Sugerencias de comandos a partir del historial, que se aceptan con `Tab`.
- Pegado de imágenes: una captura en el portapapeles se pega como ruta de archivo, que los agentes
  de código adjuntan como imagen.
- Bloques de comandos: cada comando y su salida se agrupan con su estado de salida y duración.
- Panel lateral con los agentes de código en ejecución en cada pestaña y los puertos en escucha.
- Paleta de comandos con búsqueda aproximada.
- Panel de ajustes con aplicación inmediata y guardado en `config.toml`.
- Emoji a color (incluido el formato COLRv1), CJK, true color y estilos de subrayado.
- Caracteres de dibujo de cajas y bloques generados por el propio renderizador, alineados al píxel.
- Actualizaciones desde la propia aplicación.

## Requisitos

- Linux con Wayland o X11 (desarrollado y probado en Fedora 45 con GNOME).
- GPU compatible con Vulkan, o en su defecto OpenGL.
- Bash para la integración de bloques de comandos (opcional).

## Instalación

Se descarga el paquete `.rpm` de la [última versión](https://github.com/garbanhh/garbo/releases/latest)
y se instala con:

```bash
sudo dnf install ./garbo-*.x86_64.rpm
```

La aplicación aparece como **Garbo** en el menú de aplicaciones.

## Compilación

```bash
sudo dnf install gcc rustup && rustup-init -y
cargo build --release
./target/release/garbo
```

## Uso

### Atajos de teclado

| Atajo | Acción |
|---|---|
| `Ctrl+Shift+T` / `Ctrl+Shift+W` | Abrir / cerrar pestaña |
| `Ctrl+Tab`, `Ctrl+AvPág` / `Ctrl+Shift+Tab`, `Ctrl+RePág` | Pestaña siguiente / anterior |
| `Ctrl+Shift+RePág` / `Ctrl+Shift+AvPág` | Mover la pestaña |
| `Alt+1` … `Alt+9` | Ir a una pestaña (`Alt+9`: la última) |
| `Ctrl+Shift+P` | Paleta de comandos |
| `Ctrl+,` | Ajustes |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copiar / pegar (también imágenes: ver más abajo) |
| `Tab` / `Ctrl+Espacio` | Menú de completado (sin candidatos, `Tab` llega al shell) |
| `→` o `Ctrl+F` / `Ctrl+→` | Aceptar la sugerencia en gris / solo su siguiente palabra |
| `Ctrl++` / `Ctrl+-` / `Ctrl+0` | Aumentar / reducir / restablecer el tamaño de fuente |
| `Shift+RePág` / `Shift+AvPág` / `Shift+Inicio` / `Shift+Fin` | Desplazamiento por el historial |
| `Ctrl+Shift+K` | Limpiar el historial |
| `Ctrl+Shift+Y` | Siguiente tema |
| `Ctrl+Shift+R` | Recargar la configuración |
| `Ctrl+Shift+↑` / `Ctrl+Shift+↓` | Bloque de comandos anterior / siguiente |
| `Ctrl+Shift+O` | Copiar la salida del bloque seleccionado o del último comando |
| `Ctrl+Shift+A` / `Ctrl+Shift+E` | Mostrar u ocultar los paneles de agentes / puertos |

Todos los atajos se pueden modificar en la sección `[keybindings]` de la configuración.

### Ratón

- Doble clic selecciona una palabra; triple clic, una línea; `Ctrl+Alt` y arrastrar, un bloque rectangular.
- El clic central pega la selección primaria; sobre una pestaña, la cierra.
- `Shift` fuerza la selección local en aplicaciones que capturan el ratón.
- Arrastrar un archivo a la ventana inserta su ruta.
- La barra de pestañas permite mover la ventana; el doble clic la maximiza. La ventana se
  redimensiona desde sus bordes.

### Completado y sugerencias

**Menú de completado.** `Tab` muestra lo que puede ir en la palabra que se está escribiendo: órdenes
en la primera palabra, solo carpetas tras `cd`, ramas tras `git checkout`/`switch`/`merge`,
subórdenes de `git`, `cargo`, `npm`, `dnf` o `systemctl` con su descripción, scripts de
`package.json`, objetivos de `make`, equipos de `~/.ssh/config` y archivos en el resto. La búsqueda
no distingue mayúsculas ni tildes (`doc` encuentra `Documentos`, `videos` encuentra `Vídeos`) y, si
nada empieza igual ni lo contiene, busca las letras en orden. `↑` `↓` (o `Tab`/`Shift+Tab`) eligen,
`Intro`, `→` o un clic aceptan y `Esc` cierra; seguir escribiendo refina la lista. Con un único
candidato se escribe directamente; si no hay ninguno (por ejemplo, opciones `-x`), `Tab` llega al
shell y funciona su autocompletado habitual.

**Sugerencia en gris.** Mientras se escribe (a partir de dos caracteres), Garbo muestra en gris el
resto de un comando ya usado que empieza igual: primero lo ejecutado en la carpeta actual y, si no,
lo más reciente. `→` o `Ctrl+F` lo aceptan y `Ctrl+→` acepta solo la siguiente palabra. Si el
historial no tiene nada, se sugiere el único candidato del completado (`cd Doc` → `umentos/`).

Garbo guarda su propio historial en `~/.local/share/garbo/history.tsv`, con la carpeta en la que se
ejecutó cada comando; lo combina con `$HISTFILE` (`~/.bash_history`). Un `cd` a una ruta relativa
(`cd Proyectos`) solo se sugiere en la carpeta donde se hizo. Solo se sugieren comandos que se pueden
ejecutar: su primera palabra debe ser un programa del `PATH`, una orden interna de bash o un alias o
función de los archivos de arranque (`~/.bashrc`, `~/.bashrc.d/`, `~/.bash_aliases`). Los comandos
que empiezan por un espacio no se guardan, y no hay sugerencias durante la búsqueda con `Ctrl+R`.

Requiere la integración con bash (véase *Bloques de comandos*). En *Ajustes → Completado* se puede
desactivar la sugerencia en gris, hacer que `Tab` la acepte (el menú queda en `Ctrl+Espacio`) y
abrir el menú automáticamente mientras se escribe.

### Pegar capturas de pantalla

Si al pegar con `Ctrl+Shift+V` el portapapeles no contiene texto pero sí una imagen (por ejemplo,
tras `ImprPant` en GNOME), Garbo la guarda en `~/.cache/garbo/capturas` y pega la ruta del archivo.
Claude Code y otros agentes reconocen la ruta y adjuntan la imagen. La imagen se lee con
`wl-paste` (Wayland) o `xclip` (X11); las capturas guardadas se eliminan al cabo de un día.

### Bloques de comandos

Cada comando y su salida forman un bloque, delimitado por un separador. Una barra en el margen
izquierdo indica su estado (verde: correcto; rojo: error; azul: en ejecución).

Con el **prompt de Garbo** (activado por defecto), cada bloque muestra encima del comando la carpeta
en la que se ejecutó y su duración (o el código de error, en rojo), y el comando en negrita. El
prompt actual es una etiqueta con la carpeta y, si se está en un repositorio, la rama de git; lo
que se escribe va debajo, desde la primera columna. Se desactiva en *Ajustes → Bloques → Prompt de
Garbo* (`garbo_prompt = false`) para usar el prompt propio del shell; en ese caso el código de error
o la duración (si supera un segundo) se muestran a la derecha.

Un bloque se selecciona con `Ctrl+Shift+↑` / `Ctrl+Shift+↓` o con un clic en el margen izquierdo.
Con un bloque seleccionado, `Ctrl+Shift+C` copia el bloque completo y `Ctrl+Shift+O` su salida; la
paleta de comandos permite además copiar únicamente el comando.

La integración funciona con bash: Garbo lo inicia con `--rcfile` apuntando a
[`shell/garbo.bash`](shell/garbo.bash), que carga `~/.bashrc` y emite marcas OSC 133 al comienzo
del prompt, del comando y de la salida, y al finalizar el comando. No requiere modificar la
configuración del shell. Se desactiva con `shell_integration = false`, o solo su representación
visual con `blocks = false`.

Cada bloque queda anclado al propio contenido de la rejilla mediante un hiperenlace interno en las
celdas del prompt, por lo que su posición se conserva ante el desplazamiento, el recorte del
historial y el reajuste de líneas al redimensionar.

### Panel lateral

Los botones de la barra de pestañas (o `Ctrl+Shift+A` y `Ctrl+Shift+E`) muestran un panel con dos
secciones:

- **Agentes**: pestañas que ejecutan un agente de código (Claude Code, Codex, Gemini CLI, Aider,
  OpenCode, entre otros), con su estado (*Trabajando*, *Terminado*, *Esperando tu mensaje*) y el
  tiempo transcurrido. Un clic activa la pestaña; al pasar el ratón, la fila se despliega y muestra
  las últimas líneas de la conversación de esa pestaña (sin el cuadro de entrada ni la barra de
  estado del agente) sin necesidad de cambiar a ella. La propia pestaña muestra también un
  indicador. La detección se basa en el proceso en primer plano de cada terminal y en su actividad;
  la salida que se produce justo al teclear no cuenta como trabajo del agente.
- **Subagentes**: bajo cada sesión de Claude Code se listan los subagentes que lanza, con su
  descripción, el tiempo en marcha y la última herramienta que están usando. Se obtienen, en modo
  de solo lectura, de los archivos de sesión que Claude Code guarda en `~/.claude/projects`.
- **Puertos**: servidores web locales del usuario (detectados mediante una petición HTTP), con el
  programa que los ejecuta; un clic los abre en el navegador. El resto de puertos en escucha se
  agrupa en una lista desplegable. Esta sección solo consulta el sistema mientras está visible.

### Ajustes

La ventana de ajustes se abre con `Ctrl+,` o con el botón de menú de la barra de pestañas. Muestra
las categorías a la izquierda (Tema, Texto, Terminal, Bloques, Ventana y Actualizaciones) y sus
opciones a la derecha. Los cambios se aplican de inmediato y se guardan en
`~/.config/garbo/config.toml`, conservando el resto del archivo y sus comentarios.

- **Tema**: cuadrícula de muestras con los colores de cada tema; un clic lo aplica.
- **Actualizaciones**: botón *Buscar* para comprobar en el momento si hay una versión nueva (y, si
  la hay, *Actualizar*), además de la búsqueda automática y la versión instalada.

La rueda del ratón desplaza la lista sin cambiar el elemento seleccionado. Navegación con teclado:
`Tab` / `Shift+Tab` para cambiar de categoría, `↑` `↓` para elegir, `←` `→` para cambiar el valor,
`Intro` para activar y `Esc` para cerrar.

## Configuración

El archivo de configuración es `~/.config/garbo/config.toml`; todas las opciones están
documentadas en [`config.example.toml`](config.example.toml). Los valores por defecto coinciden
con los de GNOME Terminal (Adwaita Mono 11 e interlineado natural). El tema por defecto es
`garbo-black` y la interfaz usa la misma fuente que el terminal (configurable con `ui_family`).

## Actualizaciones

Garbo consulta las versiones publicadas en GitHub al iniciarse y cada 24 horas. Cuando existe una
versión más reciente, la barra de pestañas muestra el aviso **Actualizar a X.Y.Z**. Al pulsarlo,
descarga el paquete, verifica su suma SHA-256 e instala mediante `pkexec dnf`, que solicita la
contraseña de administrador. La búsqueda de actualizaciones puede desactivarse en los ajustes.

## Rendimiento

| Técnica | Efecto |
|---|---|
| Redibujado solo ante cambios (`ControlFlow::Wait`) | Consumo de CPU nulo en reposo |
| Un único pipeline y una llamada de dibujo por fotograma | Fondos, texto, cursor e interfaz como instancias |
| Fusión de fondos contiguos | Menos instancias; el fondo por defecto no se dibuja |
| Atlas de glifos | Cada glifo se rasteriza y se sube a la GPU una sola vez |
| Pestañas inactivas sin renderizar | Su PTY sigue procesándose sin coste de GPU |
| Fuentes mapeadas en memoria y cargadas bajo demanda | Las fuentes de respaldo no ocupan memoria hasta usarse |
| Selección de controladores Vulkan por fabricante | Evita cargar controladores ajenos que arrastran libLLVM (~25 MB) |
| Compilación con LTO, `codegen-units = 1` y `panic = abort` | Binario compacto y rápido |

Mediciones en Fedora 45 con GPU Intel Arc integrada y Wayland (compilación release):

| Métrica | Valor |
|---|---|
| Memoria residente al iniciar | ~30 MB (~11 MB privados) |
| CPU en reposo | 0 % |
| `seq 1 3000000` | ~2,2 s |
| Tamaño del binario | ~8 MB |

## Arquitectura

```
src/
  main.rs         inicio de la aplicación
  app.rs          estado, pestañas, acciones y composición del fotograma
  events.rs       teclado, ratón y eventos del PTY
  terminal.rs     pestaña: PTY, estado del terminal (alacritty_terminal) y bloques
  pty_loop.rs     E/S del PTY y detección de marcas OSC 133
  blocks.rs       bloques de comandos: marcas, anclas, rangos y copia
  renderer.rs     renderizador wgpu
  shader.wgsl     primitivas: rectángulos redondeados, glifos, emoji, arcos y segmentos
  atlas.rs        atlas de glifos
  font.rs         descubrimiento (fontdb) y rasterizado (swash) de fuentes
  colr.rs         rasterizado de emoji COLRv1 (skrifa + tiny-skia)
  view.rs         conversión de la rejilla en instancias de dibujo
  ui.rs           barra de pestañas, paleta e iconos
  settings.rs     panel de ajustes y persistencia
  palette.rs      paleta de comandos y búsqueda
  input.rs        traducción de teclado y ratón a secuencias xterm
  keybindings.rs  atajos de teclado configurables
  config.rs       configuración y temas
  update.rs       comprobación e instalación de actualizaciones
  suggest.rs      sugerencias en gris desde el historial (con la carpeta de cada comando)
  complete.rs     menú de completado: candidatos según el contexto y búsqueda
  clipboard.rs    pegado de imágenes del portapapeles
  sessions.rs     detección de agentes en primer plano y su estado
  ports.rs        puertos en escucha y detección de servidores web
  subagents.rs    subagentes de Claude Code a partir de sus archivos de sesión
  panel.rs        panel lateral de agentes y puertos, con vista previa al pasar el ratón
shell/            integración con bash
packaging/        icono, entrada de escritorio y metadatos AppStream
scripts/          publicación de versiones
tools/            utilidades de prueba (capturas, sesiones simuladas)
```

## Desarrollo

- `scripts/publicar.sh X.Y.Z` actualiza la versión, ejecuta las pruebas, crea la etiqueta y la
  publica; GitHub Actions genera entonces el paquete RPM y la versión en GitHub.
- `GARBO_SCREENSHOT=/ruta/captura.ppm` guarda cada fotograma en disco.
- `GARBO_GPU_DEBUG=1` activa las capas de validación de la GPU.
- `RUST_LOG=debug` aumenta el nivel de registro.

## Licencia

Distribuido bajo la licencia MIT (véase [`LICENSE`](LICENSE)). `src/pty_loop.rs` deriva de
alacritty_terminal, distribuido bajo la licencia Apache 2.0 (véase [`NOTICE`](NOTICE)).
