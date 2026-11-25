# gridterm

Gestor de terminales nativo para macOS, escrito en Rust con GPUI (el framework de Zed) y `alacritty_terminal`.

Las terminales se colocan en un grid que se expande dentro de la ventana: primero en horizontal y después en vertical. Tres herramientas laterales siguen a la **terminal activa**, la que tiene el foco:

- **Git**: staging, commit, deshacer el último commit local, revertir un commit ya subido (con push), push y pull. El History es un git graph compacto de todas las ramas locales y remotas y los tags (líneas de 1 px, carriles de 8 px, hasta 24 carriles visibles): punto relleno = commit pusheado, punto hueco = solo local, anillo exterior = HEAD; chips con las ramas y tags de cada commit (la rama actual resaltada). Revertir solo se ofrece en commits pusheados de la historia de HEAD.
- **Explorer**: árbol de carpetas y archivos, y búsqueda por contenido. Los archivos se abren en un visor de solo lectura con resaltado de sintaxis, como pestañas de la terminal activa.
- **File Search** (⌘P): búsqueda fuzzy por nombre, en una paleta flotante (también disponible como panel del sidebar).

## Ejecutar

```sh
source ~/.cargo/env
cargo run --release            # binario en target/release/gridterm
scripts/bundle.sh              # target/release/bundle/gridterm.app
```

Al salir se guarda la sesión (grid, cwd y pestañas fijadas de cada terminal, terminal activa y ancho del sidebar) en `~/.config/gridterm/session.toml`, y se restaura al abrir. La primera vez (sin sesión) la app abre el selector de carpetas de macOS para la primera terminal; si lo cancelas, el grid queda vacío con la celda **+**.

## Atajos

| Atajo | Acción |
|---|---|
| ⌘T | Nueva terminal (si hay hueco): elige la carpeta en Finder |
| ⌘W | Cerrar la pestaña de archivo visible; si se ve la terminal, cerrarla (con sus archivos) |
| ⌘⇧] / ⌘⇧[ | Pestaña siguiente / anterior de la celda activa |
| Esc (en un archivo) | Volver a la pestaña de la terminal |
| Clic derecho en las pestañas de una celda | Menú: «Nueva terminal en celda» abre una terminal hija en la carpeta de la que se ve. Comparte la celda: sus archivos y lo que ve un agente que corra en ella. ⌘W o su ✕ la cierran; el ✕ de la derecha sigue cerrando la celda entera |
| Rueda, ⇧ + rueda o trackpad sobre las pestañas | Desplazarlas en horizontal cuando no caben (la pestaña que se abre se hace visible sola) |
| ⌘ + clic o F12 sobre un símbolo (en un archivo) | Ir a su declaración: sigue el `import`/`use` hasta el archivo (TS/JS y Rust, con alias de `tsconfig`, `node_modules` y re-exportaciones); si no, la busca en el mismo archivo y luego en el proyecto. Con ⌘ pulsado el símbolo se subraya |
| ⌘1…⌘9 | Enfocar la terminal N |
| ⌘⌥←↑→↓ | Mover el foco por el grid |
| ⌘⇧↩ | Zoom de la terminal activa |
| ⌘B | Mostrar u ocultar el sidebar |
| ⇧⌘G / ⇧⌘E / ⌘P / ⇧⌘F | Git / Explorer / buscar archivo / buscar en archivos |
| ⌘, | Ajustes (grid, fuente, Option como Meta, pull --rebase) |
| Arrastrar el borde del sidebar | Cambiar su ancho |
| Arrastrar un separador del grid (doble clic: igualar) | Cambiar el tamaño de celdas y filas |
| ⌘ + clic sobre una URL o ruta | Abrir la URL; un archivo (`ruta:línea`) se abre como pestaña de esa terminal en esa línea; una carpeta se muestra en el Explorer (también enlaces OSC 8 y URL partidas en varias filas) |
| Esc (en el sidebar) | Volver a la terminal activa |
| ⌘C / ⌘V / ⌘K | Copiar selección / pegar / limpiar (en una terminal). Con solo una imagen en el portapapeles, ⌘V envía ctrl+V al programa en primer plano (así Claude Code pega la imagen); en la shell no hace nada |
| Arrastrar, doble o triple clic | Seleccionar texto, también dentro de programas que solo reciben clics (Claude Code). En los que siguen el arrastre (vim con `mouse=a`, tmux), con ⇧ |
| ⇧PgUp / ⇧PgDn, ⌘Home / ⌘End | Scroll del historial |

En la paleta de File Search: ↩ (o doble clic) abre el archivo como pestaña de la celda activa, ⇧↩ inserta la ruta en la terminal activa, ⌥↩ hace `cd` a su carpeta, ⌘↩ la muestra en el Explorer, ⌘⇧C copia la ruta y Esc (o un clic fuera) cierra la paleta.

En el Explorer: clic en un archivo = vista previa (pestaña en cursiva que el siguiente clic reemplaza; el foco se queda en el Explorer), doble clic = pestaña fijada. Con teclado: Espacio previsualiza, ↩ abre, ⌥↩ inserta la ruta en la terminal. Clic en una línea de la búsqueda por contenido abre el archivo en esa línea. Si al pulsar ⇧⌘F hay texto seleccionado (en la terminal o en un archivo) y cabe en una línea, se busca directamente. Las acciones de cada fila son: insertar la ruta, `cd` a la carpeta, copiar la ruta y mostrar en Finder.

## Configuración

El fichero de configuración es `~/.config/gridterm/config.toml`. Todos los campos son opcionales; también se puede guardar desde Ajustes.

```toml
[grid]
max_cols = 3
max_rows = 2

[terminal]
font_family = "Menlo"
font_size = 13.0
option_as_meta = false
dim_inactive = true
cursor_blink = true

[git]
pull_rebase = false

[search]
max_content_results = 2000   # límite de líneas en la búsqueda por contenido

[ui]
theme = "dark"               # o "light"
```

`GRIDTERM_CONFIG_DIR` sustituye a `~/.config/gridterm` (lo usan las pruebas). `GRIDTERM_IDE_LOCK_DIR` sustituye a `~/.claude/ide` para el `.lock` de la integración con Claude Code (las pruebas lo aíslan así).

## Arquitectura

```
crates/
  app/         binario: Workspace (grid + contexto activo), sidebar y paneles (git, explorer, search, settings)
  terminal/    PTY + alacritty_terminal, TerminalView (render en canvas GPUI), teclas→xterm, cwd vía libproc
  layout/      algoritmo puro del grid (horizontal primero, la última fila se estira) y navegación
  git_client/  git2 para lecturas e index; CLI `git` para commit/revert/push/pull (hooks, firma, credenciales)
  fs_tools/    árbol lazy (`ignore`), índice fuzzy (`nucleo` + watcher), búsqueda de contenido (motor de ripgrep)
  config/      configuración TOML
```

**Contexto activo.** Cuando cambia el foco, y cada 500 ms (solo en la terminal activa), se lee el cwd del proceso en primer plano (`tcgetpgrp` + `proc_pidinfo`). La raíz del repo se busca subiendo hasta un `.git`, unos pocos `stat` sin abrir el repositorio. Los paneles solo se recargan si cambia esa raíz. El botón 📌 fija el contexto. Las terminales inactivas actualizan su cabecera solo cuando emiten salida.

**Nuevas terminales y tamaño del grid.** Mientras el grid tenga hueco, la siguiente celda vacía muestra un **+**. Al pulsarlo (o con ⌘T, o con el botón de la barra lateral) se abre el selector de carpetas de macOS y la terminal se abre en la carpeta elegida; cancelar no hace nada. Con el grid lleno no hay **+** y ⌘T solo avisa. Al reducir columnas o filas en Ajustes, si alguna terminal queda fuera (las últimas en orden, porque el grid se rellena en orden) aparece un diálogo de confirmación que las lista, marca en rojo las que tienen procesos en marcha (el trabajo en primer plano, o jobs en segundo plano) y vela sus celdas. Solo al confirmar se cierran (sus procesos mueren) y se aplica el tamaño nuevo; Esc o Cancelar lo dejan todo igual.

**Agentes: Claude Code `/ide`.** gridterm actúa como «IDE» para Claude Code, con el mismo protocolo que usan VS Code y JetBrains (no está documentado; probado con Claude Code 2.1.331). Un `claude` lanzado en una celda se conecta solo, porque cada shell recibe `CLAUDE_CODE_SSE_PORT`. `/ide` también lo muestra como «gridterm».

Desde ese momento Claude sabe qué archivo se está viendo **en su celda**: la última pestaña de archivo mostrada, aunque hayas vuelto a la pestaña de la terminal para escribirle. Si hay líneas seleccionadas en esa pestaña, sabe cuáles (`⧉ 3 lines selected`). Con ⌥⌘K en el visor se inserta `@archivo#Lx-y` en su prompt.

No se comparten las pestañas de otras celdas ni el texto de la terminal. Las ediciones se siguen aprobando en la terminal: gridterm no implementa el diff en el IDE.

Por dentro, gridterm escucha solo en `127.0.0.1`, con un token aleatorio, y publica `~/.claude/ide/<puerto>.lock` con permisos 0600; el fichero se borra al salir. Se desactiva en Ajustes → Agents, o con `[agents] claude_ide = false`.

**Agentes: servidor MCP.** Los agentes que se abren en una celda reciben herramientas MCP de gridterm **sin configurar nada** (sin `mcp add`). Las herramientas son de solo lectura y siempre sobre la celda del agente:

- `get_selection`: el archivo que se ve en la celda y las líneas seleccionadas (desde 1, con su texto).
- `get_cell_context`: cwd, repositorio y rama, procesos en marcha, archivo visto y pestañas abiertas.

Cómo llega a cada agente (probado con estas versiones):

| Agente | Mecanismo |
|---|---|
| Claude Code 2.1.331 | Variable `CLAUDE_CODE_PLUGIN_DIRS`, que apunta a un plugin generado por gridterm. |
| omp 19.1.1 | `--plugin-dir`, añadido por la integración de zsh. |
| Copilot CLI 1.0.115 | `--additional-mcp-config`, añadido por la integración de zsh. |
| opencode 2.2.5 | No usa MCP: va por el protocolo `/ide`, como Claude, y la selección le llega sola. La muestra en su barra si la terminal tiene unas 100 columnas o más (por ejemplo, con la celda en zoom). |

La integración de zsh es la técnica de VS Code y Ghostty. gridterm arranca zsh con su propio `ZDOTDIR`; su `.zshenv` restaura el tuyo y carga **tus** ficheros como siempre, sin modificarlos. Después define las funciones `omp` y `copilot`, que añaden la opción solo al lanzar el agente, nunca a sus subcomandos (`omp config`, `copilot mcp`…). Si defines tu propio `omp` o `copilot`, prevalece el tuyo. En bash y fish no hay integración, así que solo funcionan Claude Code y opencode.

`gridterm mcp` (el mismo ejecutable) es el servidor MCP. Pregunta a la app por un socket Unix privado (`GRIDTERM_SOCKET`), y la app identifica el proceso que pregunta por el sistema, no por lo que diga el cliente. Así un agente solo puede ver su celda, y fuera de gridterm las herramientas responden «not running inside a gridterm cell».

La configuración global de los agentes (`~/.claude.json`, `~/.omp/agent/mcp.json`, `~/.copilot/mcp-config.json`, la de opencode) no se toca. Los ficheros generados están en `~/.config/gridterm/agents/`. Se desactiva en Ajustes → Agents o con `[agents] mcp = false`.

**Archivos como pestañas.** Cada celda del grid es una terminal más los archivos abiertos desde ella: el Explorer y la búsqueda muestran la carpeta de la terminal activa, así que un archivo pertenece a la terminal que estaba activa al abrirlo (o a la terminal donde se hizo ⌘-clic). El grid sigue contando solo terminales. Al enfocar un archivo, su terminal pasa a ser la activa y las herramientas no cambian de carpeta. Una terminal oculta tras una pestaña sigue viva pero no se repinta; si emite salida, su pestaña muestra un punto.

**Visor.** Solo lectura (`Editor` de gpui-component): números de línea, selección, copia y ⌘F. El archivo se lee en segundo plano y se recarga si cambia en disco. Binarios (NUL en los primeros 8 KB) y archivos de más de 64 MB muestran un aviso; de los muy largos se muestran las primeras 50.000 líneas / 4 MB con un aviso. El resaltado usa syntect (lenguaje por extensión) con un analizador propio que trabaja siempre fuera del hilo de UI: el de la librería analizaba en línea los archivos de hasta 64 KB y bloqueaba la ventana 75–110 ms (medido).

**Estilo.** Sin esquinas redondeadas en ninguna parte (el tema de componentes usa radio 0). Las notificaciones aparecen arriba a la derecha, sin animación, apiladas una debajo de otra. Las informativas se cierran solas a los 5 s y las de error se quedan hasta pulsar ✕.

**Procesos.**
- Cada terminal lanza tu shell directamente como shell de login (`$SHELL -l`), sin el envoltorio `/usr/bin/login`. Así, si gridterm muere, la shell termina con su terminal y no quedan procesos huérfanos. A cambio, no aparece el mensaje "Last login".
- Al cerrar una terminal o salir de la app, las shells (y sus trabajos) reciben SIGHUP. Si en 1 s no han terminado, se les manda SIGKILL.
- Los procesos auxiliares (`open`) se recogen, así que no quedan zombies.

**Debounce de búsquedas.** Tanto File Search como la búsqueda por contenido muestran resultados 250 ms después de dejar de teclear. En File Search, ↩, ⇧↩, ⌥↩, ↑↓ y ⌘⇧C aplican antes la consulta pendiente; en la búsqueda por contenido, ↩ busca al momento.

**File Search.** El índice solo se construye mientras la búsqueda está visible, de modo que una terminal en `~` no cuesta nada hasta que buscas. Tiene un tope configurable (`[search] max_indexed_files`, 300.000 por defecto) y se actualiza desde la interfaz como mucho cada 50 ms. Los resultados cuyo **nombre de archivo** coincide con la búsqueda van primero (y se resalta el nombre); después, los que solo coinciden por alguna carpeta de su ruta. Así, `event` muestra `ui/event_handler.rs` antes que `events/mod.rs`.

**Watchers.**
- El panel Git se refresca con un watcher sobre `.git` (`index`, `HEAD`, `refs`) y el árbol de trabajo no ignorado, con debounce y sin sondeo.
- El Explorer relee las carpetas cargadas que cambian en disco.
- El índice de File Search se reindexa al crear, borrar o renombrar archivos.

**Git.** Clic en un archivo despliega su diff: cada hunk tiene su botón de Stage/Unstage, y se pueden seleccionar líneas sueltas para aplicar solo esas, como `git add -p` o GitKraken. El botón ⤢ de la cabecera History abre el historial en una ventana flotante grande, con la ruta del repositorio, la rama y la celda a la que pertenece (número, fila y columna, con un minimapa del grid); Esc, ✕ o un clic fuera la cierran. Un clic en un commit del historial del panel abre la ventana con ese commit desplegado; dentro de la ventana, un clic en un commit lo despliega o lo pliega: arriba, una banda con el sha completo, los padres, autor y fecha, el mensaje entero y el enlace al commit en el remoto (GitHub, GitLab, Bitbucket, Azure DevOps); debajo, los archivos del commit, cerrados: un clic muestra su diff contra el primer padre, y al hacer scroll su cabecera queda fija arriba (un clic en ella lo cierra). Los diffs (también los del panel) tienen resaltado de sintaxis, calculado sobre el archivo entero de cada lado para acertar aunque el hunk empiece dentro de un comentario o un string. El botón ▾ junto a Commit abre Undo last commit (`git reset --soft HEAD~1`, solo commits sin push), Pull, Push y Refresh, con los commits pendientes de cada lado. Push y pull muestran el progreso de git en vivo y se pueden cancelar. Si la rama no tiene upstream, se pide confirmación antes de `push --set-upstream`. Los errores se resumen en un mensaje accionable, por ejemplo: "Push rejected: … Pull first, then push again.".

**Performance.**
- El fork de la shell se hace fuera del hilo de UI.
- La salida del PTY se agrupa (como mucho un refresco cada 4 ms).
- Cada terminal es una vista cacheada: las que están idle no se repintan.
- Solo se pintan las líneas visibles.
- Todas las listas están virtualizadas.

## Pruebas

```sh
cargo test --workspace     # unitarias e integración
scripts/e2e.sh [out_dir]   # todos los casos del plan → out_dir/RESULTS.md
```

`scripts/e2e.sh` ejecuta lo siguiente:
- Compila la app en release con la feature `e2e` y la lanza en un entorno aislado (directorio de configuración temporal y `zsh` sin rc de usuario).
- Un runner dentro de la app crea un fixture: un repo con remoto bare, un segundo clon, una carpeta sin repo y 1.000 commits de historial.
- El runner pulsa teclas y ratón de verdad y comprueba el estado. En cada fallo guarda una captura y un volcado (`FAIL-<id>.png/.txt`).
- Relanza la app para comprobar la restauración de la sesión.
- Pasa los tests de rendimiento en release (`--ignored`) y comprueba el `.app` y los benchmarks de criterion (`cargo bench -p fs_tools`).

`GRIDTERM_E2E_STOP_AFTER=<id>` detiene el runner tras un caso.

## CI y releases

- **CI** (`.github/workflows/ci.yml`): en cada push a `master` y en cada pull request, una máquina macOS de GitHub pasa clippy (normal y con `--features e2e`, sin avisos) y `cargo test --workspace`. La suite e2e no se ejecuta ahí porque necesita pantalla: se sigue lanzando en local con `scripts/e2e.sh`.
- **Release** (`.github/workflows/release.yml`): se publica subiendo una etiqueta igual a la versión de `Cargo.toml`:

  ```sh
  # 1. subir version = "X.Y.Z" en Cargo.toml y hacer commit
  git tag vX.Y.Z
  git push origin master vX.Y.Z
  ```

  El workflow comprueba que la etiqueta coincide con la versión, pasa la CI, genera la app con `scripts/package.sh` (que usa `scripts/bundle.sh` y comprime con `ditto`) y crea la release con `gridterm-X.Y.Z-arm64.zip`. Las notas son las instrucciones de instalación de `.github/release-notes.md` y la lista de cambios que genera GitHub.
- La app se firma **ad-hoc**, así que macOS la bloquea la primera vez que se abre tras descargarla; las notas de la release explican cómo permitirla. Para evitarlo hacen falta un certificado *Developer ID* y la notarización de Apple.

## Limitaciones conocidas / próximos pasos

- **Arranque en frío**: entre 190 y 390 ms hasta el primer frame según la carga de la máquina; el presupuesto es de 300 ms. El código propio de gridterm supone ~6 ms; el resto es AppKit y GPUI. El plan fijaba al principio 200 ms y se amplió tras medir:
  - Una app GPUI vacía ya tarda 225–268 ms en esta máquina.
  - Los shaders de Metal se compilan en tiempo de ejecución (`gpui-kit` fuerza `runtime_shaders`), pero macOS los guarda en caché: precompilarlos solo ahorraría 4–11 ms. Únicamente el primer lanzamiento tras instalar o actualizar paga unos 130 ms más de compilación.
  - El primer lanzamiento de un binario nuevo suma entre 0,7 y 1,2 s por el análisis de seguridad de macOS.
- **IME**: verificado contra el contrato de `InputHandler` (el que usa macOS para `setMarkedText`/`insertText`). La composición con un teclado e IME reales del sistema no se puede simular en los tests; conviene probarla a mano (´ + e, ñ, japonés).
