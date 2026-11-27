---
paths:
  - "**/*.rs"
  - "**/Cargo.toml"
---

# Buenas prácticas de Rust en gridterm

Aplican al código nuevo y al que se toque. No reescribas código existente solo para cumplirlas salvo que se pida.

## Tipos y diseño
- Modela los estados con `enum` en vez de combinaciones de `bool`/`Option`; que los estados inválidos no se puedan representar.
- Usa *newtypes* cuando dos valores del mismo tipo base se puedan confundir (ids, pids, índices de fila/columna).
- Parámetros prestados (`&str`, `&[T]`, `&Path`, `impl AsRef<Path>`); devuelve tipos con dueño. No añadas `.clone()` solo para contentar al borrow checker: replantea el ownership primero.
- Visibilidad mínima: `pub(crate)` por defecto, `pub` solo lo que otro crate usa.
- Deriva o implementa los traits estándar que tengan sentido (`Debug`, `Clone`, `Default`, `PartialEq`, `From`/`TryFrom`, `Display`).
- `match` exhaustivo sobre enums propios: evita `_ =>` para que una variante nueva rompa la compilación donde importa.

## Errores
- En código de producción, nada de `unwrap()` (clippy lo rechaza). Si es una invariante real, `expect("por qué no puede fallar")`; mejor aún, reestructura para que el `Option`/`Result` no exista (`let ... else`, `get_or_insert_with`, `LazyLock` para regex estáticas). En tests, `benches/` y el harness e2e `unwrap()` está permitido.
- Propaga con `?` y añade contexto con `anyhow::Context` (`.with_context(|| format!("reading {}", path.display()))`).
- No descartes errores en silencio: `let _ = ...` o `.ok()` solo si ignorar el fallo es intencionado; si no es obvio, comenta por qué o regístralo con `log::warn!`.
- Diagnóstico interno con `log` (`debug!`, `warn!`, `error!`), no con `println!`/`eprintln!`. `eprintln!` sí vale para mensajes dirigidos al usuario en la CLI (`external.rs`, binarios en `src/bin/`) y en el harness e2e.

## `unsafe`
- Solo para FFI (`libc`) o cuando no haya alternativa segura. Antes, comprueba si `std` ya lo ofrece.
- Cada bloque `unsafe` lleva justo encima un comentario `// SAFETY:` (en inglés, como el resto del código) que explique la invariante que lo hace correcto.
- Bloques mínimos: solo la llamada insegura dentro del bloque, envuelta en una función segura que valide entradas y convierta el resultado (códigos de retorno → `io::Result`, buffers → tipos Rust).
- Comprueba siempre el valor de retorno de las llamadas a libc y usa `io::Error::last_os_error()` al fallar.
- Al leer cadenas C de buffers fijos, no asumas el terminador nulo: acota la longitud al tamaño del buffer.

## Concurrencia
- En código nuevo, locks de `parking_lot` y canales de `crossbeam-channel` (dependencias del workspace) en vez de `std::sync::Mutex` y `std::sync::mpsc`.
- Mantén los locks el menor tiempo posible: nunca durante I/O, llamadas a procesos o callbacks a la UI.
- Prefiere pasar mensajes a compartir estado mutable.
- El hilo de UI (gpui) no bloquea: I/O, git, búsquedas y procesos van en background y devuelven el resultado.

## Rendimiento
- Evita asignaciones en bucles calientes (render del terminal, pintado del grid): `with_capacity`, reutilizar buffers, iteradores en vez de `collect` intermedios, `Cow` cuando proceda.
- Iteradores antes que indexado manual.
- No optimices sin medir; los benchmarks de `fs_tools` (`tests/perf.rs`) son el modelo.

## Dependencias y manifiestos
- Versiones en `[workspace.dependencies]` del `Cargo.toml` raíz; los crates usan `dep = { workspace = true }`.
- Antes de añadir una dependencia nueva, comprueba si `std` o una ya existente lo resuelven.
- Desactiva `default-features` cuando no se necesiten.

## Estilo y documentación
- `snake_case` para funciones y variables, `CamelCase` para tipos, `SCREAMING_SNAKE_CASE` para constantes.
- `///` en items públicos de cada crate; añade `# Errors`, `# Panics` o `# Safety` cuando apliquen.
- Comentarios que expliquen el porqué, no el qué.

## Tests
- Unitarios junto al código en `#[cfg(test)] mod tests`; de integración en `crates/<crate>/tests/`.
- Cada bug corregido lleva un test que lo reproduzca si es razonable.

## Antes de dar algo por terminado
- `cargo fmt --all` (`rustfmt.toml`: `max_width = 120`).
- `cargo clippy --workspace --all-targets` sin warnings. Los lints viven en `[workspace.lints]` del `Cargo.toml` raíz: `unwrap_used`, `undocumented_unsafe_blocks`, `dbg_macro`, `todo` y `unimplemented` son `deny`. No los silencies con `#[allow]` sin un comentario que lo justifique.
- Si tocas `crates/app/src/workspace/e2e.rs`: `cargo clippy -p gridterm --features e2e`.
- `cargo test` del crate tocado (o `--workspace` si el cambio cruza crates).
- Si añades o actualizas dependencias: `cargo deny check` (licencias permitidas en `deny.toml`).
- Cobertura: `cargo cov-summary` (o `cargo cov` para el HTML en `target/llvm-cov/html`).
- El hook `scripts/git-hooks/pre-commit` ejecuta fmt y clippy sobre los crates del commit (`git config core.hooksPath scripts/git-hooks`).
