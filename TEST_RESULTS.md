# Resultados — 215/215 PASS

| ID | Caso | Resultado | Detalle | Tipo |
|---|---|---|---|---|
| A1 | Con hueco hay una celda «+» en la siguiente posición (3×2: n = 1, 3, 5) | ✅ PASS | n=1→Some((0, 1)), n=3→Some((1, 0)), n=5→Some((1, 2)) | e2e |
| A2 | Con el grid lleno no hay celda «+» | ✅ PASS | 6/6 sin + | e2e |
| A3 | Clic en «+» → carpeta elegida → terminal ahí, activa | ✅ PASS | T2 en /private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-mcp-full2/fixture/grid-a (cwd leído del proceso) | e2e |
| A4 | Clic en «+» y cancelar: no se abre nada y el foco no cambia | ✅ PASS | 2 terminales, foco en Some(1) | e2e |
| A5 | ⌘T con hueco pide carpeta y abre la terminal en ella | ✅ PASS | T3 en /private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-mcp-full2/fixture/grid-b | e2e |
| A6 | ⌘T con el grid lleno no abre el selector y avisa | ✅ PASS | Grid full (3×2). Increase columns/rows in Settings (⌘,). | e2e |
| A7 | Con el selector abierto, otro ⌘T o «+» no abre un segundo selector | ✅ PASS | 1 selector; al responder se abre la terminal | e2e |
| A8 | Celda «+»: sin esquinas redondeadas, hover en acento | ✅ PASS | 4 esquinas = acento con hover | e2e |
| A9 | ⌘⌥→/↓ no llevan el foco a la celda «+» | ✅ PASS | el foco se queda en la última terminal | e2e |
| A10 | Arranque sin sesión: pide carpeta; si se cancela, grid vacío con «+» | ✅ PASS | 1 petición, 0 terminales, + de 986×825 | e2e |
| B1 | El cursor de la terminal activa parpadea | ✅ PASS | 3 cambios en 1,7 s | e2e |
| B2 | Al teclear deja de parpadear y vuelve tras una pausa | ✅ PASS | ok | e2e |
| B3 | Las inactivas no parpadean ni se repintan por ello | ✅ PASS | 0 renders | e2e |
| B4 | Respeta lo que pide el programa (\e[2 q fijo, \e[1 q parpadeante) | ✅ PASS | ok | e2e |
| B5 | Se puede desactivar en Ajustes | ✅ PASS | ok | e2e |
| C1 | Cambiar el foco cambia el contexto | ✅ PASS | repo ↔ plain | e2e |
| C2 | En un subdirectorio del repo la raíz es la del repo | ✅ PASS | root = repo | e2e |
| C3 | Fuera de un repo: raíz = cwd y Git muestra 'no es un repo' | ✅ PASS | ok | e2e |
| C4 | Cambiar entre terminales del mismo repo no recarga paneles | ✅ PASS | ok | e2e |
| C5 | El pin fija el contexto | ✅ PASS | ok | e2e |
| C6 | La cabecera muestra ruta, terminal y rama | ✅ PASS | branch main, T1 | e2e |
| D1 | Buscar archivo: resultados 250 ms después de dejar de teclear | ✅ PASS | aplicada a los 270.566ms (sondeo cada 20 ms), 1 vez por ráfaga | e2e |
| D2 | Una pausa más corta que el debounce no aplica la consulta a medias | ✅ PASS | solo se aplicó "guide" | e2e |
| D3 | ↓ (y ↩, ⌥↩, ⇧↩, ⌘⇧C) aplican al momento la consulta pendiente | ✅ PASS | aplicada sin esperar, una sola vez | e2e |
| D4 | Buscar en archivos: la búsqueda arranca 250 ms después de dejar de teclear | ✅ PASS | arrancó a los 271.606916ms; 2 archivos con coincidencias | e2e |
| D5 | ↩ en buscar en archivos busca al momento | ✅ PASS | arrancó a los 22.032459ms (sondeo cada 20 ms) | e2e |
| E1 | Árbol lazy, respeta .gitignore y toggle de ocultos | ✅ PASS | ok | e2e |
| E1-lib | read_dir: gitignore, ocultos, orden | ✅ PASS | 6/6 ok | unit/int |
| E2 | Teclado ↑↓←→; Espacio previsualiza, ↩ abre, ⌥↩ inserta la ruta | ✅ PASS | ok | e2e |
| E3 | Badges de estado git en archivos y carpetas | ✅ PASS | M en src/lib.rs + punto en src/ | e2e |
| E4 | Acciones: insertar ruta, cd, copiar ruta | ✅ PASS | cd + insert (E2) ok; Finder no se abre en tests | e2e |
| E5 | Búsqueda de contenido: literal, regex y mayúsculas | ✅ PASS | ok | e2e |
| E5-lib | Búsqueda de contenido (literal, regex, rangos, binarios, límite, cancelación) | ✅ PASS | 7/7 ok | unit/int |
| E6 | Una búsqueda nueva cancela la anterior | ✅ PASS | ok | e2e |
| E7 | Ámbito: carpeta seleccionada o raíz | ✅ PASS | ["src/main.rs", "src/many.txt"] | e2e |
| E8 | Límite de resultados configurable | ✅ PASS | ok | e2e |
| E9 | Watcher: archivos nuevos aparecen en carpetas expandidas | ✅ PASS | 125.420375ms | e2e |
| E9-lib | DirWatcher: create/remove/rename, debounce, ignora .git y escrituras | ✅ PASS | 10/10 ok | unit/int |
| F1-lib | Sesión: serialización | ✅ PASS | 2/2 ok | unit/int |
| F1a | Al salir se guarda la sesión (grid + cwd) | ✅ PASS | 5 terminals; checked by scripts/e2e.sh after quit | e2e |
| F1a | Al salir se guarda la sesión (fichero) | ✅ PASS | /private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-mcp-full2/config/session.toml | script |
| F1b | La sesión restaura grid y cwd de cada terminal | ✅ PASS | 5 terminals restored | e2e |
| F2 | Empaquetado .app | ✅ PASS | target/release/bundle/gridterm.app | script |
| F3 | Benchmarks criterion (index + matching) | ✅ PASS | crates/fs_tools/benches | script |
| G1 | compute_layout: horizontal→vertical, última fila estirada | ✅ PASS | 9/9 ok | unit/int |
| G2 | Estructura del grid en la app: 1,2,3,3+1,3+2,3+3 | ✅ PASS | ok | e2e |
| G3 | Grid lleno: ⌘T se rechaza con aviso | ✅ PASS | Grid full (3×2). Increase columns/rows in Settings (⌘,). | e2e |
| G4 | ⌘W cierra, recompacta y enfoca a la vecina | ✅ PASS | ok | e2e |
| G5 | ⌘1…9 y ⌘⌥flechas mueven el foco | ✅ PASS | ok | e2e |
| G6 | Zoom ⌘⇧↩: solo se pinta la activa y las demás siguen vivas | ✅ PASS | ok | e2e |
| G7 | Cols/rows desde la UI; reducir por debajo de las abiertas pide confirmación | ✅ PASS | ok | e2e |
| G7-lib | config.toml: defaults, límites, roundtrip | ✅ PASS | 5/5 ok | unit/int |
| Gi1 | Status, rama, ahead/behind | ✅ PASS | main ↔ origin/main | e2e |
| Gi2 | Stage/unstage por archivo y de todo | ✅ PASS | ok | e2e |
| Gi3 | Commit con ⌘↩ desde la caja de mensaje | ✅ PASS | ok | e2e |
| Gi4 | Deshacer solo si no está pusheado; cambios a staged y mensaje recuperado | ✅ PASS | ok | e2e |
| Gi5 | Revertir un commit pusheado (confirmación + push) | ✅ PASS | ok | e2e |
| Gi6 | Push sin upstream pide confirmación | ✅ PASS | ok | e2e |
| Gi8 | El progreso de push se ve en tiempo real | ✅ PASS | 4 updates | e2e |
| Gi9 | Error non-fast-forward con mensaje accionable | ✅ PASS | Push: Push rejected: the remote has commits you don't have. Pull first, then push again. | e2e |
| Gi10 | El panel se actualiza solo (watcher) en < 1 s, 3 veces | ✅ PASS | [Some(41.352083ms), Some(210.848291ms), Some(207.224291ms)] | e2e |
| Gi11 | El historial es una lista virtualizada | ✅ PASS | 24 rows for 200 commits | e2e |
| Gi12 | Push/pull se pueden cancelar | ✅ PASS | ok | e2e |
| Gi13 | git status 10k archivos < 100 ms | ✅ PASS | 1 tests ok | perf |
| Gi-lib | git_client: status, stage, commit, undo, revert, push/pull, watcher, progreso, cancelación, errores | ✅ PASS | 53/53 ok | unit/int |
| H1 | Diff de un archivo en hunks con números de línea | ✅ PASS | @@ -1,5 +1,5 @@ / @@ -25,6 +25,6 @@ line 24 | e2e |
| H2 | Stage de un solo hunk | ✅ PASS | solo el hunk 1 en el índice | e2e |
| H3 | Unstage de un solo hunk | ✅ PASS | índice limpio | e2e |
| H4 | Stage de líneas sueltas | ✅ PASS | solo NEW B | e2e |
| H5 | Archivo nuevo: stage de una línea | ✅ PASS | índice: "two" | e2e |
| H6 | Binarios: sin hunks, se ofrece stage del archivo entero | ✅ PASS | binario | e2e |
| H8 | Diff enorme: truncado, virtualizado y sin bloquear la UI | ✅ PASS | 5000 líneas cargadas, 22 filas pintadas, stalls p95 1.333792ms | e2e |
| H-lib | Diff por hunks y staging parcial (hunks, líneas, nuevos, borrados, CRLF, sin salto final, obsoletos) | ✅ PASS | 18/18 ok | unit/int |
| I1 | Tecla muerta: ´ + e → é (composición vía InputHandler) | ✅ PASS | é | e2e |
| I2 | Caracteres del teclado español: ñ ç ¿ € ü | ✅ PASS | ñç¿€ü | e2e |
| I2b | La shell recibe un locale UTF-8 aunque la app arranque sin LANG | ✅ PASS | app sin LANG: true; shell: UTF-8 | e2e |
| I3 | Texto de IME (CJK y emoji) llega en UTF-8 | ✅ PASS | 日本語🚀 | e2e |
| I4 | Teclear normal no duplica caracteres | ✅ PASS | [abc-XYZ_123] | e2e |
| I5 | El texto en composición se ve en el cursor y no se envía | ✅ PASS | pre-edit visible only as overlay (see I5-preedit.png) | e2e |
| I6 | Option como Meta activado: ⌥B envía ESC b | ✅ PASS | one Xtwo | e2e |
| I7 | La ventana de candidatos del IME se sitúa en la celda del cursor | ✅ PASS | cursor (1, 33) | e2e |
| I-lib | Clasificación texto/teclas especiales (IME) | ✅ PASS | 1/1 ok | unit/int |
| IDE1 | Las terminales reciben CLAUDE_CODE_SSE_PORT = puerto del .lock | ✅ PASS | puerto 54808, /private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-mcp-full2/ide-locks/54808.lock | e2e |
| IDE2 | Una conexión desde la celda 2 queda asignada a la celda 2; una de fuera, a ninguna | ✅ PASS | celda 2 = T9; la de fuera sin celda | e2e |
| IDE3 | Abrir un archivo en la celda 2 → selection_changed con filePath y sin texto | ✅ PASS | {"text":"","selection":{"start":{"line":0,"character":0},"end":{"line":0,"character":0},"isEmpty":true},"filePath":"/private/tmp/claude-502/-Users-ruben-motoro- | e2e |
| IDE4 | Seleccionar líneas 3–5 en el visor (teclado) → texto exacto y rango, < 300 ms | ✅ PASS | 129.622083ms: {"start":{"line":2,"character":0},"end":{"line":4,"character":5},"isEmpty":false} | e2e |
| IDE5 | Volver a la pestaña de la terminal mantiene lo último mostrado (sin limpiar) | ✅ PASS | sin cambios (1 reenvíos de lo mismo) | e2e |
| IDE6 | Cerrar la pestaña limpia el contexto; con otra pestaña de archivo visible, envía esa | ✅ PASS | limpieza y luego b.rs | e2e |
| IDE7 | Lo que se abre en la celda 1 no llega a la conexión de la celda 2 (ni a la de fuera) | ✅ PASS | nada | e2e |
| IDE8 | ⌥⌘K envía at_mentioned con archivo y líneas; sin conexión, aviso | ✅ PASS | {"filePath":"/private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-mcp-full2/fixture/ide/a.rs","lineStart":2,"li | e2e |
| IDE9 | Desactivar el ajuste cierra el servidor y borra el .lock; reactivar crea otro | ✅ PASS | cerrado, .lock borrado, reactivado | e2e |
| IDE9b | Al salir de la app se borra el .lock de /ide | ✅ PASS | /private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-mcp-full2/ide-locks vacío | script |
| IDE10 | 10 conexiones y selección continua: ningún frame > 16 ms; todas reciben la última | ✅ PASS | 10 conexiones, hueco máx. 8.428166ms | e2e |
| IDE11 | Cliente sin token (como opencode) desde la celda 2: el sistema lo asigna a su celda y recibe la vista | ✅ PASS | asignado en 195.12825ms (incluye lanzar el probe), recibe b.rs | e2e |
| IDE12 | Cliente sin token desde fuera de las celdas: no recibe nada | ✅ PASS | conectado pero sin datos; 11 conexiones asignadas a celdas | e2e |
| IDE13 | Un token incorrecto se rechaza | ✅ PASS | ide-probe: connect failed: HTTP error: 401 Unauthorized | e2e |
| IDE-lib | Claude Code /ide: lock 0600, token, subprotocolo mcp, MCP, notificaciones, varias conexiones | ✅ PASS | 12/12 ok | unit/int |
| IDE-pid | Procesos: padre y ascendencia (asignar la conexión a su celda) | ✅ PASS | 1/1 ok | unit/int |
| K1 | Reducir sin terminales afectadas: se aplica sin diálogo | ✅ PASS | 3×2 → 2×2 con 4 terminales | e2e |
| K2 | Reducir con terminales afectadas: diálogo con la lista, nada aplicado aún | ✅ PASS | cerrarían T[38, 39], sin procesos | e2e |
| K3 | Proceso en primer plano (sleep) → «los procesos morirán» | ✅ PASS | T39: ["sleep"] | e2e |
| K4 | Job en segundo plano (sleep &) también se detecta | ✅ PASS | T38: ["sleep"] | e2e |
| K5 | Shell inactivo con la configuración real del usuario: sin falsos positivos | ✅ PASS | 0 procesos; hijos del shell ignorados: [] | e2e |
| K6 | Cancelar (Esc o botón): config, terminales y Ajustes sin cambios | ✅ PASS | Esc y Cancelar | e2e |
| K7 | Confirmar: se cierran las condenadas, se aplica 2×2 y los procesos mueren | ✅ PASS | 4 terminales, 2×2; 2 sleep terminados en 50.96825ms | e2e |
| K8 | Con el diálogo abierto, las celdas condenadas muestran el velo | ✅ PASS | condenadas [332533, 332533], resto [181825, 161623] | e2e |
| K-lib | Procesos: hijos y grupo de proceso (detección de procesos activos) | ✅ PASS | 4/4 ok | unit/int |
| L1 | Con ⌘, pasar sobre una URL la subraya | ✅ PASS | segments [(0, 6..38)] | e2e |
| L2 | ⌘clic en una URL la abre | ✅ PASS | abierta | e2e |
| L3 | ⌘clic en una ruta (con :línea) la abre como pestaña en esa línea | ✅ PASS | src/main.rs, línea 2 | e2e |
| L4 | Enlaces OSC 8 | ✅ PASS | ok | e2e |
| L5 | Clic sin ⌘ no abre nada | ✅ PASS | ok | e2e |
| L6 | Detección de URLs y rutas | ✅ PASS | 3/3 ok | unit/int |
| L7 | URL más ancha que la terminal (partida en dos filas) | ✅ PASS | 164 caracteres en 39 columnas, 5 filas subrayadas | e2e |
| M1 | Las terminales reciben GRIDTERM_SOCKET y CLAUDE_CODE_PLUGIN_DIRS; socket 0600 | ✅ PASS | /tmp/gridterm-502/gridterm-80186.sock (0600) | e2e |
| M2 | Integración de zsh: tu ZDOTDIR restaurado, tus ficheros cargados, omp y copilot son funciones | ✅ PASS | ZDOTDIR=/private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-mcp-full2/zdotdir, .zshrc cargado, omp/copilot = f | e2e |
| M3 | tools/list desde una celda: get_selection y get_cell_context | ✅ PASS | get_selection, get_cell_context | e2e |
| M4 | Líneas 3–5 seleccionadas en la celda 2 → get_selection desde la celda 2 devuelve 3–5 y su texto | ✅ PASS | {"file":"/private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-mcp-full2/fixture/mcp/a.rs","start_line":3,"end_l | e2e |
| M5 | Celda 1 (sin archivo): file null; fuera de una celda o sin socket: error claro | ✅ PASS | null / «not running inside a gridterm cell» / «GRIDTERM_SOCKET is not set» | e2e |
| M6 | get_cell_context: cwd, repo, rama, pestañas y procesos | ✅ PASS | {"terminal":20,"cwd":"/private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-mcp-full2/fixture/repo","repo_root": | e2e |
| M7 | Desactivar el ajuste quita el socket y las llamadas dan error; reactivar funciona | ✅ PASS | apagado: «gridterm: cannot reach gridterm (/tmp/gridterm-502/gridterm-80186.sock): No such file or directory (os error 2)»; encendido: ok | e2e |
| M8 | Al salir de la app se borra el socket de agentes | ✅ PASS | /tmp/gridterm-502/gridterm-80186.sock no existe | script |
| M9 | Cada consulta por el socket responde en < 50 ms | ✅ PASS | peor 30.266833ms | e2e |
| M-lib | MCP de agentes: socket (pid del kernel, 0600), gridterm mcp, ficheros generados, integración de zsh real | ✅ PASS | 7/7 ok | unit/int |
| N1 | Las notificaciones aparecen sin animación y no se mueven | ✅ PASS | bounds estables Bounds { origin: Point { x: 1072px, y: 8px }, size: Size { 360px × 48.5px } } | e2e |
| N2 | Varias notificaciones se apilan una debajo de otra | ✅ PASS | 3 apiladas, separación 6 px | e2e |
| N3 | Info se cierra sola a los 5 s; error se queda hasta pulsar ✕ | ✅ PASS | ok | e2e |
| N4 | Sin radio: tema a 0 y esquinas cuadradas | ✅ PASS | esquinas = color del borde | e2e |
| N4-lib | Sin rounded_* en el código | ✅ PASS | 1/1 ok | unit/int |
| O1 | ⌘W termina la shell de esa terminal (y sus trabajos) | ✅ PASS | shell 56.102208ms, trabajo None | e2e |
| O2 | exit termina y recoge la shell (sin zombie) | ✅ PASS | 41.733583ms | e2e |
| O3 | Salir de la app no deja shells ni trabajos vivos | ✅ PASS | 6 procesos terminados | script |
| O4 | Si matan la app (SIGKILL), sus shells no sobreviven | ✅ PASS | 6 procesos terminados | script |
| O5 | Si termina la shell de una terminal inactiva, el foco no se mueve | ✅ PASS | foco sigue en T1 | e2e |
| P0 | Referencia: bloqueos del hilo de UI en reposo | ✅ PASS | stalls p50 1.02875ms p95 1.084458ms max 2.042792ms; frame-callback meter tops at 121.0/s (not used) | e2e |
| P1 | Arranque en frío hasta el primer frame < 300 ms | ✅ PASS | 213.607ms | e2e |
| P2 | Nueva terminal: primer frame < 50 ms | ✅ PASS | 30.642958ms (poll 20.344375ms) | e2e |
| P3 | 6 terminales con mucho output: ~60 fps | ✅ PASS | stalls p50 1.020709ms p95 4.899875ms max 14.707708ms | e2e |
| R1 | Arrastrar el separador entre celdas cambia anchos y columnas | ✅ PASS | 39 → 55 columnas | e2e |
| R2 | Arrastrar el separador entre filas cambia las alturas | ✅ PASS | 23 → 28 filas | e2e |
| R3 | Hay un tamaño mínimo por celda | ✅ PASS | 120 px | e2e |
| R4 | Doble clic en el separador reparte a partes iguales | ✅ PASS | [1.0, 1.0, 1.0] | e2e |
| R5 | Tamaños estables con foco/zoom, reparto al cambiar el número y guardados en la sesión | ✅ PASS | ok | e2e |
| R6 | El arrastre sigue fuera de la ventana y no se queda enganchado | ✅ PASS | izquierda 120 px | e2e |
| R-lib | Pesos del grid: arrastre, mínimo, reinicio | ✅ PASS | 2/2 ok | unit/int |
| S1 | Índice por raíz con resultados progresivos | ✅ PASS | ok | e2e |
| S1-lib | Índice fuzzy progresivo + rescan + watcher | ✅ PASS | 7/7 ok | unit/int |
| S2 | Caché LRU: volver a una raíz es instantáneo | ✅ PASS | 6 files, 2 roots cached | e2e |
| S3 | El watcher incorpora archivos nuevos al índice | ✅ PASS | 438.469709ms | e2e |
| S4 | ⌘P abre paleta flotante; Esc cierra y devuelve foco; ↩ inserta y cierra | ✅ PASS | ok | e2e |
| S5 | Atajos de la paleta: insertar, cd, copiar, mostrar en Explorer | ✅ PASS | ok | e2e |
| S6 | 100k archivos < 1 s, tecla < 16 ms | ✅ PASS | 1 tests ok | perf |
| S7 | Buscar archivo: los que coinciden en el nombre van antes que los que solo coinciden en la carpeta | ✅ PASS | ["src/bus/event.rs", "src/ui/event_handler.rs", "docs/prevent.md", "src/events/mod.rs", "src/eventual/a.rs", "src/events/queue.rs"] | e2e |
| T1 | La shell arranca y responde a lo tecleado | ✅ PASS | echo visible after 21.051041ms | e2e |
| T2 | ⌘T no bloquea la UI: la celda aparece al instante | ✅ PASS | handler 169.958µs (dispatch incl. pending frame draw 313.916µs) | e2e |
| T3 | Primer output de /bin/sh < 50 ms | ✅ PASS | 1 tests ok | perf |
| T4 | Teclas → secuencias xterm | ✅ PASS | 6/6 ok | unit/int |
| T5 | Option como Meta configurable | ✅ PASS | 1/1 ok | unit/int |
| T6 | Pegar (⌘V) y copiar selección (⌘C) | ✅ PASS | paste + copy ok | e2e |
| T6-lib | Copiar = texto de la selección | ✅ PASS | 1/1 ok | unit/int |
| T7 | Al redimensionar, tput cols = ancho de la celda | ✅ PASS | 44 columnas | e2e |
| T8 | Scroll del historial (⇧PgUp) y vuelta abajo al teclear | ✅ PASS | offset 49 → 0 | e2e |
| T9 | Colores 256 / truecolor | ✅ PASS | 2/2 ok | unit/int |
| T10 | exit cierra la celda | ✅ PASS | 5 → 4 | e2e |
| T11 | Las terminales sin actividad no se repintan | ✅ PASS | 0 renders | e2e |
| T12 | Salida agrupada: como mucho un render cada ~4 ms | ✅ PASS | 34 lotes, 20 renders, 20 frames en 166.398209ms | e2e |
| T13 | El cwd se detecta tras cd en < 1 s | ✅ PASS | 458.423208ms | e2e |
| T13b | Solo se sondea la terminal activa | ✅ PASS | 0 | e2e |
| Th1 | Cambiar a tema claro en vivo afecta a UI y terminales | ✅ PASS | terminal eff1f5, status dce0e8 | e2e |
| Th3 | El tema se guarda en config.toml | ✅ PASS | theme = "light" guardado; vuelta a oscuro ok | e2e |
| Th4 | Contraste WCAG AA de temas y paletas | ✅ PASS | 2/2 ok | unit/int |
| U1 | Sidebar redimensionable y ⌘B | ✅ PASS | 300 → 400 | e2e |
| U2 | Cabecera de celda y terminal activa (captura) | ✅ PASS | ver captura U2-U3-T9-visual.png | e2e |
| U3 | Atenuar inactivas es configurable | ✅ PASS | ok | e2e |
| U4 | Todos los atajos del plan están registrados | ✅ PASS | 15 atajos | e2e |
| U5 | Esc desde el sidebar (inputs incluidos) vuelve a la terminal | ✅ PASS | ok | e2e |
| U6 | Búsqueda pesada sin bloquear la UI y cancelable | ✅ PASS | stalls p50 1.034875ms p95 1.057833ms max 1.276833ms; frame callbacks 122/s; running at start: true | e2e |
| V1 | Clic en un archivo del Explorer: vista previa en la terminal activa | ✅ PASS | README.md en vista previa (cursiva) de T1; foco sigue en el Explorer | e2e |
| V2 | Otro clic simple reemplaza la vista previa | ✅ PASS | 1 pestaña: src/lib.rs (preview) | e2e |
| V3 | Doble clic fija la pestaña; la siguiente vista previa va aparte | ✅ PASS | lib.rs fijada + README.md preview | e2e |
| V4 | Enfocar el visor activa su terminal; el contexto no cambia | ✅ PASS | activa T1, raíz /private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-mcp-full2/fixture/repo | e2e |
| V5 | ⌘clic en ruta:línea abre la pestaña en ESA terminal, en esa línea | ✅ PASS | many.txt línea 7 en T2; long.txt:2500 visible (filas Some(2481..2502)); carpeta → Explorer | e2e |
| V6 | ⌘W cierra la pestaña visible, no la terminal | ✅ PASS | lib.rs cerrada; terminal visible | e2e |
| V7 | Cerrar una terminal cierra (y libera) sus pestañas | ✅ PASS | 2 pestañas liberadas con su terminal | e2e |
| V8 | Esc en un archivo vuelve a la pestaña de la terminal | ✅ PASS | terminal visible y enfocada | e2e |
| V9 | ⌘⇧] / ⌘⇧[ recorren las pestañas | ✅ PASS | [Some(0), Some(1), None, Some(1), Some(0)] | e2e |
| V10 | Archivo binario: aviso en lugar del contenido | ✅ PASS | aviso de binario | e2e |
| V11 | Archivo largo truncado con aviso; demasiado grande no se carga | ✅ PASS | 50000 de 60000 líneas; >64 MB → aviso | e2e |
| V12 | Se recarga al cambiar en disco; aviso si se borra | ✅ PASS | cambio visible en 127.639ms; reemplazo atómico y borrado ok | e2e |
| V12-lib | Watcher de un archivo (contenido, reemplazo atómico, borrado; no hermanos) | ✅ PASS | 1/1 ok | unit/int |
| V13 | La sesión restaura las pestañas fijadas de cada terminal | ✅ PASS | 2 pestañas, visible Some(1) | e2e |
| V13a | Preparar sesión con pestañas (se comprueba en V13 al restaurar) | ✅ PASS | ([("/private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-mcp-full2/fixture/repo/README.md", false), ("/private/ | e2e |
| V14 | Resaltado de sintaxis (tema oscuro y claro) | ✅ PASS | oscuro: rs 88 colores, txt 27; claro: rs 107 colores, txt 38 | e2e |
| V15 | Terminal oculta: no se repinta y su pestaña marca actividad | ✅ PASS | 0 repintados ocultos; punto de actividad hasta volver | e2e |
| V16 | Pestañas y visor sin esquinas redondeadas | ✅ PASS | 4 esquinas = color del borde | e2e |
| V-lib | Visor: carga, binario, truncado por líneas/bytes, lenguaje; resaltado en segundo plano | ✅ PASS | 5/5 ok | unit/int |
| VP0 | Primer archivo abierto tras arrancar: contenido visible < 100 ms | ✅ PASS | contenido en 16.791875ms, hueco máx entre frames 8.484333ms | e2e |
| VP1 | Abrir archivos de 1k/10k/50k líneas: contenido visible < 100 ms | ✅ PASS | 1000: contenido en 21.55875ms (lectura+set 8.397084ms), hueco máx entre frames 8.364ms (antes de abrir, en reposo: 9.09575ms) / 1000b: contenido en 23.692083ms  | e2e |
| VP2 | Abrir un archivo de 60 MB no bloquea la UI > 16 ms | ✅ PASS | carga Some(20.491333ms), truncado=true, bloqueo UI p50 1.2275ms p95 1.285416ms máx 7.9115ms | e2e |
| X1 | Sin abrir File Search no se indexa la carpeta grande | ✅ PASS | ningún índice hasta abrir la búsqueda | e2e |
| X2 | Indexando 200k archivos con la paleta abierta, la UI no se bloquea | ✅ PASS | stalls p50 1.247459ms p95 1.331791ms max 3.308917ms; 200001 archivos | e2e |
| X3 | Tope configurable de archivos indexados, avisado en la UI | ✅ PASS | 50000 archivos, aviso de límite (ver X3-index-limited.png) | e2e |
| X4 | Con el árbol grande, una búsqueda encuentra el archivo en < 1,5 s | ✅ PASS | 23.880083ms | e2e |
| X-lib | Índice con tope de archivos | ✅ PASS | 1/1 ok | unit/int |
| Y1 | El History tiene los commits de todas las ramas y tags | ✅ PASS | 6 commits: ["merge feature", "main work", "feature work", "wip work", "topic work", "root"] | e2e |
| Y2 | Mismo orden que git log --date-order | ✅ PASS | igual | e2e |
| Y3 | Chips de rama/tag en cada punta, sin origin/HEAD | ✅ PASS | main (HEAD), origin/main, v1, topic, origin/topic, wip, feature | e2e |
| Y4 | Merge con dos líneas a carriles distintos; topic en su propio carril | ✅ PASS | merge → carriles [0, 1]; main 0, topic 1, 2 carriles | e2e |
| Y5 | Líneas finas (≤ 2 px de dispositivo) y carriles a 8 px | ✅ PASS | anchos [2, 2] px de dispositivo, separación 8.0 px | e2e |
| Y6 | Nodo relleno si está pusheado, hueco si es local (wip) | ✅ PASS | main work=89b4fa, topic work=cba6f7, wip work=181825 | e2e |
| Y7 | Revertir solo en commits pusheados de la historia de HEAD | ✅ PASS | ["merge feature", "main work", "feature work", "root"] | e2e |
| Y8 | 1000 commits / 20 ramas: todos los nodos visibles, graph < 20 ms y scroll sin huecos > 16 ms | ✅ PASS | layout 13.279834ms, 20 carriles; huecos p50 8.33325ms, p95 8.368666ms, máx. 8.397209ms | e2e |
| Y9 | Chips sin esquinas redondeadas | ✅ PASS | 4 esquinas = fondo del chip | e2e |
| Y-lib | Git graph: carriles, merges, octopus, límite, reutilización; log con todas las ramas y refs | ✅ PASS | 7/7 ok | unit/int |
| Z1 | Abrir enlaces y "Mostrar en Finder" no deja zombies | ✅ PASS | 0 zombies tras 6 aperturas | e2e |

## Fases opcionales con agentes reales (se ejecutan aparte; no envían prompts)

`GRIDTERM_E2E_PHASE=agents`, sin las carpetas aisladas (`/ide` y socket reales). Versiones: Claude Code 2.1.331, opencode 2.2.5, omp 19.1.1, Copilot CLI 1.0.115.

| ID | Caso | Resultado | Detalle |
|---|---|---|---|
| AG1 | Claude Code en una celda: /mcp muestra gridterm conectado (sin configurar nada) | ✅ PASS | `gridterm … 2 tools` (plugin vía `CLAUDE_CODE_PLUGIN_DIRS`) |
| AG2 | opencode en una celda: se conecta por /ide a su celda y recibe el archivo mostrado | ✅ PASS | asignado a su celda en 272 ms; con 1 línea seleccionada y la celda en zoom (137 columnas) su barra muestra `README.md` |
| AG3 | omp en una celda: /mcp list muestra gridterm conectado | ✅ PASS | `gridterm:gridterm` (función de zsh → `--plugin-dir`) |
| AG4 | Copilot CLI en una celda: /mcp muestra gridterm | ✅ PASS | `mcp:gridterm` (función de zsh → `--additional-mcp-config`) |
| AG5 | La configuración global de los agentes no ha cambiado | ✅ PASS | `~/.claude.json` mcpServers, `~/.omp/agent/mcp.json`, `~/.copilot/mcp-config.json`, `~/.config/opencode/opencode.jsonc` iguales |

`GRIDTERM_E2E_PHASE=claude` (Claude Code 2.1.331): CC1 conexión automática a su celda (1,1 s), CC2 `⧉ In main.rs`, CC3 `⧉ 2 lines selected`; todos ✅ (ejecución anterior, con los casos aún llamados C1–C3).
