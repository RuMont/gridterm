# Resultados — 189/189 PASS

| ID | Caso | Resultado | Detalle | Tipo |
|---|---|---|---|---|
| A1 | Con hueco hay una celda «+» en la siguiente posición (3×2: n = 1, 3, 5) | ✅ PASS | n=1→Some((0, 1)), n=3→Some((1, 0)), n=5→Some((1, 2)) | e2e |
| A2 | Con el grid lleno no hay celda «+» | ✅ PASS | 6/6 sin + | e2e |
| A3 | Clic en «+» → carpeta elegida → terminal ahí, activa | ✅ PASS | T2 en /private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-rank-full2/fixture/grid-a (cwd leído del proceso) | e2e |
| A4 | Clic en «+» y cancelar: no se abre nada y el foco no cambia | ✅ PASS | 2 terminales, foco en Some(1) | e2e |
| A5 | ⌘T con hueco pide carpeta y abre la terminal en ella | ✅ PASS | T3 en /private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-rank-full2/fixture/grid-b | e2e |
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
| D1 | Buscar archivo: resultados 250 ms después de dejar de teclear | ✅ PASS | aplicada a los 269.485916ms (sondeo cada 20 ms), 1 vez por ráfaga | e2e |
| D2 | Una pausa más corta que el debounce no aplica la consulta a medias | ✅ PASS | solo se aplicó "guide" | e2e |
| D3 | ↓ (y ↩, ⌥↩, ⇧↩, ⌘⇧C) aplican al momento la consulta pendiente | ✅ PASS | aplicada sin esperar, una sola vez | e2e |
| D4 | Buscar en archivos: la búsqueda arranca 250 ms después de dejar de teclear | ✅ PASS | arrancó a los 271.622792ms; 2 archivos con coincidencias | e2e |
| D5 | ↩ en buscar en archivos busca al momento | ✅ PASS | arrancó a los 20.944583ms (sondeo cada 20 ms) | e2e |
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
| E9 | Watcher: archivos nuevos aparecen en carpetas expandidas | ✅ PASS | 129.0055ms | e2e |
| E9-lib | DirWatcher: create/remove/rename, debounce, ignora .git y escrituras | ✅ PASS | 10/10 ok | unit/int |
| F1-lib | Sesión: serialización | ✅ PASS | 2/2 ok | unit/int |
| F1a | Al salir se guarda la sesión (grid + cwd) | ✅ PASS | 5 terminals; checked by scripts/e2e.sh after quit | e2e |
| F1a | Al salir se guarda la sesión (fichero) | ✅ PASS | /private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-rank-full2/config/session.toml | script |
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
| Gi8 | El progreso de push se ve en tiempo real | ✅ PASS | 6 updates | e2e |
| Gi9 | Error non-fast-forward con mensaje accionable | ✅ PASS | Push: Push rejected: the remote has commits you don't have. Pull first, then push again. | e2e |
| Gi10 | El panel se actualiza solo (watcher) en < 1 s, 3 veces | ✅ PASS | [Some(44.391792ms), Some(207.534167ms), Some(229.582041ms)] | e2e |
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
| H8 | Diff enorme: truncado, virtualizado y sin bloquear la UI | ✅ PASS | 5000 líneas cargadas, 22 filas pintadas, stalls p95 1.08225ms | e2e |
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
| K1 | Reducir sin terminales afectadas: se aplica sin diálogo | ✅ PASS | 3×2 → 2×2 con 4 terminales | e2e |
| K2 | Reducir con terminales afectadas: diálogo con la lista, nada aplicado aún | ✅ PASS | cerrarían T[31, 32], sin procesos | e2e |
| K3 | Proceso en primer plano (sleep) → «los procesos morirán» | ✅ PASS | T32: ["sleep"] | e2e |
| K4 | Job en segundo plano (sleep &) también se detecta | ✅ PASS | T31: ["sleep"] | e2e |
| K5 | Shell inactivo con la configuración real del usuario: sin falsos positivos | ✅ PASS | 0 procesos; hijos del shell ignorados: [] | e2e |
| K6 | Cancelar (Esc o botón): config, terminales y Ajustes sin cambios | ✅ PASS | Esc y Cancelar | e2e |
| K7 | Confirmar: se cierran las condenadas, se aplica 2×2 y los procesos mueren | ✅ PASS | 4 terminales, 2×2; 2 sleep terminados en 52.492792ms | e2e |
| K8 | Con el diálogo abierto, las celdas condenadas muestran el velo | ✅ PASS | condenadas [332533, 332533], resto [181825, 161623] | e2e |
| K-lib | Procesos: hijos y grupo de proceso (detección de procesos activos) | ✅ PASS | 2/2 ok | unit/int |
| L1 | Con ⌘, pasar sobre una URL la subraya | ✅ PASS | segments [(0, 6..38)] | e2e |
| L2 | ⌘clic en una URL la abre | ✅ PASS | abierta | e2e |
| L3 | ⌘clic en una ruta (con :línea) la abre como pestaña en esa línea | ✅ PASS | src/main.rs, línea 2 | e2e |
| L4 | Enlaces OSC 8 | ✅ PASS | ok | e2e |
| L5 | Clic sin ⌘ no abre nada | ✅ PASS | ok | e2e |
| L6 | Detección de URLs y rutas | ✅ PASS | 3/3 ok | unit/int |
| L7 | URL más ancha que la terminal (partida en dos filas) | ✅ PASS | 164 caracteres en 39 columnas, 5 filas subrayadas | e2e |
| N1 | Las notificaciones aparecen sin animación y no se mueven | ✅ PASS | bounds estables Bounds { origin: Point { x: 1072px, y: 8px }, size: Size { 360px × 48.5px } } | e2e |
| N2 | Varias notificaciones se apilan una debajo de otra | ✅ PASS | 3 apiladas, separación 6 px | e2e |
| N3 | Info se cierra sola a los 5 s; error se queda hasta pulsar ✕ | ✅ PASS | ok | e2e |
| N4 | Sin radio: tema a 0 y esquinas cuadradas | ✅ PASS | esquinas = color del borde | e2e |
| N4-lib | Sin rounded_* en el código | ✅ PASS | 1/1 ok | unit/int |
| O1 | ⌘W termina la shell de esa terminal (y sus trabajos) | ✅ PASS | shell 70.6385ms, trabajo None | e2e |
| O2 | exit termina y recoge la shell (sin zombie) | ✅ PASS | 47.807333ms | e2e |
| O3 | Salir de la app no deja shells ni trabajos vivos | ✅ PASS | 6 procesos terminados | script |
| O4 | Si matan la app (SIGKILL), sus shells no sobreviven | ✅ PASS | 6 procesos terminados | script |
| O5 | Si termina la shell de una terminal inactiva, el foco no se mueve | ✅ PASS | foco sigue en T1 | e2e |
| P0 | Referencia: bloqueos del hilo de UI en reposo | ✅ PASS | stalls p50 1.05425ms p95 1.090584ms max 1.240792ms; frame-callback meter tops at 121.0/s (not used) | e2e |
| P1 | Arranque en frío hasta el primer frame < 300 ms | ✅ PASS | 220.755ms | e2e |
| P2 | Nueva terminal: primer frame < 50 ms | ✅ PASS | 35.018709ms (poll 20.847542ms) | e2e |
| P3 | 6 terminales con mucho output: ~60 fps | ✅ PASS | stalls p50 1.021375ms p95 3.572417ms max 27.5545ms | e2e |
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
| S3 | El watcher incorpora archivos nuevos al índice | ✅ PASS | 441.051542ms | e2e |
| S4 | ⌘P abre paleta flotante; Esc cierra y devuelve foco; ↩ inserta y cierra | ✅ PASS | ok | e2e |
| S5 | Atajos de la paleta: insertar, cd, copiar, mostrar en Explorer | ✅ PASS | ok | e2e |
| S6 | 100k archivos < 1 s, tecla < 16 ms | ✅ PASS | 1 tests ok | perf |
| S7 | Buscar archivo: los que coinciden en el nombre van antes que los que solo coinciden en la carpeta | ✅ PASS | ["src/bus/event.rs", "src/ui/event_handler.rs", "docs/prevent.md", "src/events/mod.rs", "src/eventual/a.rs", "src/events/queue.rs"] | e2e |
| T1 | La shell arranca y responde a lo tecleado | ✅ PASS | echo visible after 20.328667ms | e2e |
| T2 | ⌘T no bloquea la UI: la celda aparece al instante | ✅ PASS | handler 120.833µs (dispatch incl. pending frame draw 247.541µs) | e2e |
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
| T12 | Salida agrupada: como mucho un render cada ~4 ms | ✅ PASS | 32 lotes, 19 renders, 19 frames en 166.317667ms | e2e |
| T13 | El cwd se detecta tras cd en < 1 s | ✅ PASS | 440.44775ms | e2e |
| T13b | Solo se sondea la terminal activa | ✅ PASS | 0 | e2e |
| Th1 | Cambiar a tema claro en vivo afecta a UI y terminales | ✅ PASS | terminal eff1f5, status dce0e8 | e2e |
| Th3 | El tema se guarda en config.toml | ✅ PASS | theme = "light" guardado; vuelta a oscuro ok | e2e |
| Th4 | Contraste WCAG AA de temas y paletas | ✅ PASS | 2/2 ok | unit/int |
| U1 | Sidebar redimensionable y ⌘B | ✅ PASS | 300 → 400 | e2e |
| U2 | Cabecera de celda y terminal activa (captura) | ✅ PASS | ver captura U2-U3-T9-visual.png | e2e |
| U3 | Atenuar inactivas es configurable | ✅ PASS | ok | e2e |
| U4 | Todos los atajos del plan están registrados | ✅ PASS | 15 atajos | e2e |
| U5 | Esc desde el sidebar (inputs incluidos) vuelve a la terminal | ✅ PASS | ok | e2e |
| U6 | Búsqueda pesada sin bloquear la UI y cancelable | ✅ PASS | stalls p50 1.036375ms p95 1.051625ms max 1.576667ms; frame callbacks 122/s; running at start: true | e2e |
| V1 | Clic en un archivo del Explorer: vista previa en la terminal activa | ✅ PASS | README.md en vista previa (cursiva) de T1; foco sigue en el Explorer | e2e |
| V2 | Otro clic simple reemplaza la vista previa | ✅ PASS | 1 pestaña: src/lib.rs (preview) | e2e |
| V3 | Doble clic fija la pestaña; la siguiente vista previa va aparte | ✅ PASS | lib.rs fijada + README.md preview | e2e |
| V4 | Enfocar el visor activa su terminal; el contexto no cambia | ✅ PASS | activa T1, raíz /private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-rank-full2/fixture/repo | e2e |
| V5 | ⌘clic en ruta:línea abre la pestaña en ESA terminal, en esa línea | ✅ PASS | many.txt línea 7 en T2; long.txt:2500 visible (filas Some(2481..2502)); carpeta → Explorer | e2e |
| V6 | ⌘W cierra la pestaña visible, no la terminal | ✅ PASS | lib.rs cerrada; terminal visible | e2e |
| V7 | Cerrar una terminal cierra (y libera) sus pestañas | ✅ PASS | 2 pestañas liberadas con su terminal | e2e |
| V8 | Esc en un archivo vuelve a la pestaña de la terminal | ✅ PASS | terminal visible y enfocada | e2e |
| V9 | ⌘⇧] / ⌘⇧[ recorren las pestañas | ✅ PASS | [Some(0), Some(1), None, Some(1), Some(0)] | e2e |
| V10 | Archivo binario: aviso en lugar del contenido | ✅ PASS | aviso de binario | e2e |
| V11 | Archivo largo truncado con aviso; demasiado grande no se carga | ✅ PASS | 50000 de 60000 líneas; >64 MB → aviso | e2e |
| V12 | Se recarga al cambiar en disco; aviso si se borra | ✅ PASS | cambio visible en 127.892333ms; reemplazo atómico y borrado ok | e2e |
| V12-lib | Watcher de un archivo (contenido, reemplazo atómico, borrado; no hermanos) | ✅ PASS | 1/1 ok | unit/int |
| V13 | La sesión restaura las pestañas fijadas de cada terminal | ✅ PASS | 2 pestañas, visible Some(1) | e2e |
| V13a | Preparar sesión con pestañas (se comprueba en V13 al restaurar) | ✅ PASS | ([("/private/tmp/claude-502/-Users-ruben-motoro-ruben/3c4dd8fb-6e3a-4953-b49a-8531eb401c37/scratchpad/e2e-rank-full2/fixture/repo/README.md", false), ("/private | e2e |
| V14 | Resaltado de sintaxis (tema oscuro y claro) | ✅ PASS | oscuro: rs 88 colores, txt 27; claro: rs 107 colores, txt 38 | e2e |
| V15 | Terminal oculta: no se repinta y su pestaña marca actividad | ✅ PASS | 0 repintados ocultos; punto de actividad hasta volver | e2e |
| V16 | Pestañas y visor sin esquinas redondeadas | ✅ PASS | 4 esquinas = color del borde | e2e |
| V-lib | Visor: carga, binario, truncado por líneas/bytes, lenguaje; resaltado en segundo plano | ✅ PASS | 5/5 ok | unit/int |
| VP0 | Primer archivo abierto tras arrancar: contenido visible < 100 ms | ✅ PASS | contenido en 17.707416ms, hueco máx entre frames 9.396125ms | e2e |
| VP1 | Abrir archivos de 1k/10k/50k líneas: contenido visible < 100 ms | ✅ PASS | 1000: contenido en 22.9355ms (lectura+set 7.872792ms), hueco máx entre frames 8.342292ms (antes de abrir, en reposo: 8.367208ms) / 1000b: contenido en 22.162208 | e2e |
| VP2 | Abrir un archivo de 60 MB no bloquea la UI > 16 ms | ✅ PASS | carga Some(20.449791ms), truncado=true, bloqueo UI p50 1.025375ms p95 1.042125ms máx 2.159875ms | e2e |
| X1 | Sin abrir File Search no se indexa la carpeta grande | ✅ PASS | ningún índice hasta abrir la búsqueda | e2e |
| X2 | Indexando 200k archivos con la paleta abierta, la UI no se bloquea | ✅ PASS | stalls p50 1.053834ms p95 1.09175ms max 1.994542ms; 200001 archivos | e2e |
| X3 | Tope configurable de archivos indexados, avisado en la UI | ✅ PASS | 50000 archivos, aviso de límite (ver X3-index-limited.png) | e2e |
| X4 | Con el árbol grande, una búsqueda encuentra el archivo en < 1,5 s | ✅ PASS | 23.686333ms | e2e |
| X-lib | Índice con tope de archivos | ✅ PASS | 1/1 ok | unit/int |
| Y1 | El History tiene los commits de todas las ramas y tags | ✅ PASS | 6 commits: ["merge feature", "main work", "feature work", "wip work", "topic work", "root"] | e2e |
| Y2 | Mismo orden que git log --date-order | ✅ PASS | igual | e2e |
| Y3 | Chips de rama/tag en cada punta, sin origin/HEAD | ✅ PASS | main (HEAD), origin/main, v1, topic, origin/topic, wip, feature | e2e |
| Y4 | Merge con dos líneas a carriles distintos; topic en su propio carril | ✅ PASS | merge → carriles [0, 1]; main 0, topic 1, 2 carriles | e2e |
| Y5 | Líneas finas (≤ 2 px de dispositivo) y carriles a 8 px | ✅ PASS | anchos [2, 2] px de dispositivo, separación 8.0 px | e2e |
| Y6 | Nodo relleno si está pusheado, hueco si es local (wip) | ✅ PASS | main work=89b4fa, topic work=cba6f7, wip work=181825 | e2e |
| Y7 | Revertir solo en commits pusheados de la historia de HEAD | ✅ PASS | ["merge feature", "main work", "feature work", "root"] | e2e |
| Y8 | 1000 commits / 20 ramas: todos los nodos visibles, graph < 20 ms y scroll sin huecos > 16 ms | ✅ PASS | layout 12.749917ms, 20 carriles; huecos p50 8.332791ms, p95 8.374834ms, máx. 8.446833ms | e2e |
| Y9 | Chips sin esquinas redondeadas | ✅ PASS | 4 esquinas = fondo del chip | e2e |
| Y-lib | Git graph: carriles, merges, octopus, límite, reutilización; log con todas las ramas y refs | ✅ PASS | 7/7 ok | unit/int |
| Z1 | Abrir enlaces y "Mostrar en Finder" no deja zombies | ✅ PASS | 0 zombies tras 6 aperturas | e2e |
