# Ramas y commits en gridterm

Aplica de aquí en adelante; no reescribas la historia ya existente.

## Ramas: git-flow original (Vincent Driessen)

Ramas permanentes:
- `master`: solo lo publicado. Cada commit de `master` es una versión y lleva su tag `vX.Y.Z` (como `v0.5.0`). Nunca se commitea directamente en `master`.
- `develop`: integración de la próxima versión. Las features salen de aquí y vuelven aquí.

Ramas de soporte (todas se integran con `git merge --no-ff` para conservar la rama en la historia, y se borran después):

| Tipo | Sale de | Se integra en | Nombre |
|---|---|---|---|
| Feature | `develop` | `develop` | `feature/<descripcion-corta>` |
| Release | `develop` | `master` **y** `develop` | `release/X.Y.Z` |
| Hotfix | `master` | `master` **y** `develop` (o la `release/*` abierta, si la hay) | `hotfix/X.Y.Z` |

- Nombres en kebab-case y en inglés, como el código: `feature/blame-gutter`, `release/0.6.0`, `hotfix/0.5.1`.
- **Release**: en la rama solo se corrigen bugs y se prepara la versión (subir `version` en `[workspace.package]` del `Cargo.toml` raíz, `Cargo.lock`, notas). Al cerrarla: merge `--no-ff` a `master`, tag anotado `vX.Y.Z` en ese merge, merge `--no-ff` de vuelta a `develop`.
- **Hotfix**: sube el patch (`X.Y.Z+1`), arregla, y se cierra como una release: `master` + tag, y luego `develop`.
- **Antes de cerrar una release o un hotfix** (tras el commit `chore(release): X.Y.Z`, antes del merge a `master` y del tag), ejecuta `scripts/release-local.sh --ref HEAD`: reproduce en local el deploy de GitHub (`release.yml` + `ci.yml`) sobre lo commiteado. Lee su línea final `RESULT:` y, si falla, el log del paso que indica. Sin `--publish`: publicar lo hace GitHub al recibir el tag. `--help` explica cada paso.
- Antes de empezar cualquier trabajo, comprueba en qué rama estás; si estás en `master` o `develop`, crea la rama de soporte que corresponda.
- No hagas merge, tag, push ni borres ramas sin que el usuario lo pida.

## Commits: Conventional Commits 1.0.0

```
<tipo>(<scope>)!: <descripción>

[cuerpo opcional: el porqué, no el qué]

[footers opcionales, p. ej. BREAKING CHANGE: ...]
```

- **Tipos**: `feat` (funcionalidad nueva), `fix` (bug), `perf`, `refactor` (sin cambio de comportamiento), `style` (formato, sin cambio de código), `test`, `docs`, `build` (Cargo, dependencias, bundle), `ci`, `chore` (mantenimiento, releases), `revert`.
- **Scope** opcional, en minúsculas: el crate (`agents`, `app`, `config`, `fs_tools`, `git_client`, `layout`, `terminal`) o el área de la app (`git`, `explorer`, `search`, `viewer`, `settings`, `grid`, `e2e`, `deps`, `release`).
- **Descripción**: en inglés, en imperativo, minúscula inicial, sin punto final, ~72 caracteres como máximo. `feat(viewer): show git change marks next to line numbers`.
- **Cambio incompatible** (configuración, formato de sesión, CLI `gridterm open`, atajos por defecto): `!` tras el tipo/scope y un footer `BREAKING CHANGE: <qué cambia y cómo migrar>`.
- Un commit = un cambio lógico. No mezcles formateo o refactor con funcionalidad: van en commits separados (`style:` / `refactor:` primero).
- El commit de versión de una release o hotfix: `chore(release): X.Y.Z`.
- **Los mensajes son las notas de la release**: `scripts/release-notes.sh` agrupa los commits desde el tag anterior por tipo (`feat` → Features, `fix` → Bug fixes, `perf` → Performance, breaking changes arriba, el resto en Other changes) y GitHub los publica tal cual. Escribe la descripción para quien usa gridterm, no para quien lee el diff.
- El tipo marca la versión siguiente (SemVer): `fix` → patch, `feat` → minor, `BREAKING CHANGE` → major (mientras estemos en `0.x`, un breaking sube el minor).
