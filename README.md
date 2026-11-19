# Garbo

Garbo es un emulador de terminal acelerado por GPU, escrito en Rust y diseñado para un consumo
mínimo de recursos. Ofrece pestañas, bloques de comandos, paleta de comandos, temas y un panel de
ajustes con persistencia automática.

## Características

- Renderizado por GPU (wgpu: Vulkan, Metal, DX12 u OpenGL) con redibujado únicamente bajo demanda.
- Pestañas integradas en la barra de título, con herencia del directorio de trabajo.
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
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copiar / pegar |
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

### Bloques de comandos

Cada comando y su salida forman un bloque, delimitado por un separador. Una barra en el margen
izquierdo indica su estado (verde: correcto; rojo: error; azul: en ejecución) y, a la derecha, se
muestra el código de error o la duración cuando esta supera un segundo.

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
  las últimas líneas de esa pestaña sin necesidad de cambiar a ella. La propia pestaña muestra
  también un indicador. La detección se basa en el proceso en primer plano de cada terminal y en
  su actividad.
- **Subagentes**: bajo cada sesión de Claude Code se listan los subagentes que lanza, con su
  descripción, el tiempo en marcha y la última herramienta que están usando. Se obtienen, en modo
  de solo lectura, de los archivos de sesión que Claude Code guarda en `~/.claude/projects`.
- **Puertos**: servidores web locales del usuario (detectados mediante una petición HTTP), con el
  programa que los ejecuta; un clic los abre en el navegador. El resto de puertos en escucha se
  agrupa en una lista desplegable. Esta sección solo consulta el sistema mientras está visible.

### Ajustes

El panel de ajustes se abre con `Ctrl+,` o con el botón de menú de la barra de pestañas. Permite
modificar el tema, la fuente, el tamaño, el interlineado, el margen, la opacidad, el cursor, el
historial, los bloques y la búsqueda de actualizaciones. Los cambios se aplican de inmediato y se
guardan en `~/.config/garbo/config.toml`, conservando el resto del archivo y sus comentarios.

Los temas se presentan como una cuadrícula de muestras, cada una con los colores del tema; un clic
lo aplica. La rueda del ratón desplaza la lista sin cambiar el elemento seleccionado.

Navegación con teclado: `↑` `↓` para elegir, `←` `→` para cambiar el valor (también el tema),
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
