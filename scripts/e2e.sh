#!/usr/bin/env bash
# Runs every test case of the initial plan and writes $OUT/RESULTS.md.
#   scripts/e2e.sh [out_dir]
# The app runs with an isolated config dir and a zsh without user rc files.
set -uo pipefail
cd "$(dirname "$0")/.."
if [[ -f "$HOME/.cargo/env" ]]; then source "$HOME/.cargo/env"; fi
OUT="${1:-$(mktemp -d /tmp/gridterm-e2e.XXXX)}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
CFG="$OUT/config"; ZD="$OUT/zdotdir"
rm -rf "$CFG" "$ZD"; mkdir -p "$CFG" "$ZD"
cat > "$CFG/config.toml" <<'TOML'
[terminal]
shell = "/bin/zsh"
TOML
: > "$OUT/script_results.tsv"
record() { printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" >> "$OUT/script_results.tsv"; }

echo "== build (release, e2e)"
cargo build --release -p gridterm --features e2e 2>&1 | tail -1
cp target/release/gridterm "$OUT/gridterm-e2e"
BIN="$OUT/gridterm-e2e"
cargo build --release -p agents --bin ide-probe 2>&1 | tail -1
export GRIDTERM_E2E_IDE_PROBE="$PWD/target/release/ide-probe"
# Claude Code /ide lock files of the test runs stay out of ~/.claude/ide.
export GRIDTERM_IDE_LOCK_DIR="$OUT/ide-locks"
# The agent MCP socket of the test runs stays out of $TMPDIR.
export GRIDTERM_SOCKET_DIR="$OUT/sockets"

run_app() { # phase timeout
  # Launched like a GUI app: no LANG/LC_* (gridterm must set a UTF-8 locale).
  ( env -u LANG -u LC_ALL -u LC_CTYPE GRIDTERM_E2E="$OUT" GRIDTERM_CONFIG_DIR="$CFG" ZDOTDIR="$ZD" \
      GRIDTERM_OPEN_LOG="$OUT/opened.log" GRIDTERM_OPEN_CMD=/usr/bin/true GRIDTERM_E2E_PHASE="$1" "$BIN" >"$OUT/app_$1.log" 2>&1 & echo $! > "$OUT/pid" )
  local pid; pid=$(cat "$OUT/pid")
  for _ in $(seq 1 "$2"); do sleep 1; kill -0 "$pid" 2>/dev/null || return 0; done
  kill "$pid" 2>/dev/null; echo "timeout" >> "$OUT/app_$1.log"; return 1
}

echo "== warm-up launch (first launch of a new binary)"
run_app warmup 30 || true
echo "   first launch: $(cat "$OUT/first_launch_ms" 2>/dev/null)"
rm -f "$CFG/session.toml"
echo "== e2e phase 1 (main)"
run_app main 400 || echo "phase 1 timed out"
grep -E "^  (PASS|FAIL)" "$OUT/app_main.log" | sed 's/^/   /' | tail -100

# F1a: session saved on quit
EXP="$(cat "$OUT/expected_cwds" 2>/dev/null)"
if [[ -f "$CFG/session.toml" ]] && [[ -n "$EXP" ]] && python3 - "$CFG/session.toml" "$EXP" <<'PY'
import sys; s=open(sys.argv[1]).read(); exp=[c for c in sys.argv[2].split(':') if c]
sys.exit(0 if all(c in s for c in exp) else 1)
PY
then record F1a "Al salir se guarda la sesión (fichero)" PASS "$CFG/session.toml"
else record F1a "Al salir se guarda la sesión (fichero)" FAIL "no session file with cwds ($CFG/session.toml)"; fi

# I9b: the IDE lock file is removed when the app quits.
LOCK="$(cat "$OUT/ide_lock_path" 2>/dev/null)"
if [[ -n "$LOCK" && ! -e "$LOCK" ]] && [[ -z "$(ls -A "$GRIDTERM_IDE_LOCK_DIR" 2>/dev/null)" ]]; then
  record IDE9b "Al salir de la app se borra el .lock de /ide" PASS "$GRIDTERM_IDE_LOCK_DIR vacío"
else record IDE9b "Al salir de la app se borra el .lock de /ide" FAIL "left: $(ls -A "$GRIDTERM_IDE_LOCK_DIR" 2>/dev/null)"; fi

# M8: the agent socket is removed when the app quits.
SOCK="$(cat "$OUT/agent_socket_path" 2>/dev/null)"
if [[ -n "$SOCK" && ! -e "$SOCK" ]]; then record M8 "Al salir de la app se borra el socket de agentes" PASS "$SOCK no existe"
else record M8 "Al salir de la app se borra el socket de agentes" FAIL "left: ${SOCK:-no path recorded}"; fi

echo "== e2e phase 2 (restore)"
GRIDTERM_E2E_EXPECT_CWDS="$EXP" run_app restore 60 || echo "phase 2 timed out"

check_pids() { # id name file
  local pids; pids="$(cat "$OUT/$3" 2>/dev/null)"
  if [[ -z "$pids" ]]; then record "$1" "$2" FAIL "no pids recorded"; return; fi
  local alive=""
  for _ in $(seq 1 20); do
    alive=""; for p in $pids; do st=$(ps -o stat= -p "$p" 2>/dev/null); [[ -n "$st" ]] && alive="$alive $p($st)"; done
    [[ -z "$alive" ]] && break; sleep 0.1
  done
  if [[ -z "$alive" ]]; then record "$1" "$2" PASS "$(echo $pids | wc -w | tr -d ' ') procesos terminados"
  else record "$1" "$2" FAIL "survivors:$alive"; fi
}

echo "== O3 quit / O4 SIGKILL"
rm -f "$CFG/session.toml"
run_app lifecycle 60 || true
check_pids O3 "Salir de la app no deja shells ni trabajos vivos" lifecycle_pids
rm -f "$CFG/session.toml"
( env -u LANG GRIDTERM_E2E="$OUT" GRIDTERM_CONFIG_DIR="$CFG" ZDOTDIR="$ZD" GRIDTERM_E2E_PHASE=lifecycle-kill "$BIN" >"$OUT/app_lifecycle_kill.log" 2>&1 & echo $! > "$OUT/pid_kill" )
for _ in $(seq 1 60); do [[ -s "$OUT/lifecycle_kill_pids" ]] && break; sleep 0.5; done
kill -9 "$(cat "$OUT/pid_kill")" 2>/dev/null
check_pids O4 "Si matan la app (SIGKILL), sus shells no sobreviven" lifecycle_kill_pids
rm -f "$CFG/session.toml"

echo "== A10 first launch, folder picker cancelled"
rm -f "$CFG/session.toml"
GRIDTERM_E2E_PICKER=cancel run_app startup 60 || echo "startup phase timed out"
rm -f "$CFG/session.toml"

echo "== E0 closing the last cell"
run_app empty 60 || echo "empty phase timed out"
run_app external 90 || echo "external phase timed out"
rm -f "$CFG/session.toml"

echo "== F2 bundle"
if [[ -x scripts/bundle.sh ]] && scripts/bundle.sh >"$OUT/bundle.log" 2>&1 \
   && [[ -x target/release/bundle/gridterm.app/Contents/MacOS/gridterm ]] \
   && plutil -lint target/release/bundle/gridterm.app/Contents/Info.plist >/dev/null 2>&1; then
  record F2 "Empaquetado .app" PASS "target/release/bundle/gridterm.app"
else record F2 "Empaquetado .app" FAIL "no scripts/bundle.sh or invalid bundle"; fi

echo "== F3 criterion benches"
if ls crates/fs_tools/benches/*.rs >/dev/null 2>&1 && grep -q criterion crates/fs_tools/Cargo.toml \
   && cargo bench -p fs_tools --no-run >"$OUT/bench.log" 2>&1; then
  record F3 "Benchmarks criterion (index + matching)" PASS "crates/fs_tools/benches"
else record F3 "Benchmarks criterion (index + matching)" FAIL "no criterion benches"; fi

echo "== unit/integration tests"
cargo test --workspace --no-fail-fast >"$OUT/cargo_test.log" 2>&1
echo "== perf tests (release)"
cargo test --release -p terminal --test plan -- --nocapture >"$OUT/perf_t3.log" 2>&1
cargo test --release -p fs_tools --test perf -- --ignored --nocapture >"$OUT/perf_s6.log" 2>&1
cargo test --release -p git_client --test perf -- --ignored --nocapture >"$OUT/perf_gi13.log" 2>&1

python3 - "$OUT" <<'PY'
import json, re, sys, os
out = sys.argv[1]
rows = []
def load(name):
    p = os.path.join(out, name)
    return json.load(open(p)) if os.path.exists(p) else []
for c in load("results.json") + load("results_restore.json") + load("results_startup.json") + load("results_empty.json") + load("results_external.json"):
    rows.append((c["id"], c["name"], "PASS" if c["pass"] else "FAIL", c["detail"].replace("\n", " ")[:160], "e2e"))
for line in open(os.path.join(out, "script_results.tsv")):
    i, n, st, d = line.rstrip("\n").split("\t")
    rows.append((i, n, st, d, "script"))
def cargo(log, mapping, kind):
    text = open(os.path.join(out, log)).read() if os.path.exists(os.path.join(out, log)) else ""
    for cid, (name, pattern) in mapping.items():
        oks = re.findall(r"test (\S*(?:%s)\S*) \.\.\. ok" % pattern, text)
        fails = re.findall(r"test (\S*(?:%s)\S*) \.\.\. FAILED" % pattern, text)
        if not oks and not fails:
            rows.append((cid, name, "FAIL", "test not found / did not run", kind)); continue
        detail = ""
        m = re.search(r"assertion.*?\n|panicked at.*?\n.*?\n", text) if fails else None
        rows.append((cid, name, "FAIL" if fails else "PASS", (f"{len(oks)} ok, {len(fails)} failed: {fails}" if fails else f"{len(oks)} tests ok"), kind))
def sections(log):
    """Splits cargo test output into {test binary stem: [(test, ok)]}."""
    path = os.path.join(out, log)
    text = open(path).read() if os.path.exists(path) else ""
    res, cur = {}, None
    for line in text.splitlines():
        m = re.search(r"Running .*/([A-Za-z0-9_]+)-[0-9a-f]+\)", line)
        if m:
            cur = m.group(1); res.setdefault(cur, []); continue
        m = re.match(r"test (\S+) \.\.\. (ok|FAILED)", line)
        if m and cur:
            res[cur].append((m.group(1), m.group(2) == "ok"))
    return res
secs = sections("cargo_test.log")
def by_binary(cid, name, binaries, name_filter=None):
    tests = [(t, ok) for b in binaries for (t, ok) in secs.get(b, []) if not name_filter or re.search(name_filter, t)]
    if not tests:
        rows.append((cid, name, "FAIL", "tests not found / did not run", "unit/int")); return
    bad = [t for t, ok in tests if not ok]
    rows.append((cid, name, "FAIL" if bad else "PASS", f"{len(tests)-len(bad)}/{len(tests)} ok" + (f"; failed: {bad}" if bad else ""), "unit/int"))
by_binary("G1", "compute_layout: horizontal→vertical, última fila estirada", ["layout"])
by_binary("T4", "Teclas → secuencias xterm", ["terminal"], r"keys::")
by_binary("Y-lib", "Git graph: carriles, merges, octopus, límite, reutilización; log con todas las ramas y refs", ["git_client"], r"graph::|log_includes_all_branches")
by_binary("IDE-lib", "Claude Code /ide: lock 0600, token, subprotocolo mcp, MCP, notificaciones, varias conexiones", ["agents", "ide"])
by_binary("M-lib", "MCP de agentes: socket (pid del kernel, 0600), gridterm mcp, ficheros generados, integración de zsh real", ["mcp"])
by_binary("IDE-pid", "Procesos: padre y ascendencia (asignar la conexión a su celda)", ["terminal"], r"parent_and_ancestry")
by_binary("K-lib", "Procesos: hijos y grupo de proceso (detección de procesos activos)", ["terminal"], r"cwd::")
by_binary("T5", "Option como Meta configurable", ["terminal"], r"t5_option")
by_binary("T6-lib", "Copiar = texto de la selección", ["plan"], r"t6_")
by_binary("T9", "Colores 256 / truecolor", ["terminal"], r"colors::")
by_binary("G7-lib", "config.toml: defaults, límites, roundtrip", ["config"])
by_binary("E1-lib", "read_dir: gitignore, ocultos, orden", ["tree"])
by_binary("E5-lib", "Búsqueda de contenido (literal, regex, rangos, binarios, límite, cancelación)", ["content"])
by_binary("E9-lib", "DirWatcher: create/remove/rename, debounce, ignora .git y escrituras", ["watch"])
by_binary("S1-lib", "Índice fuzzy progresivo + rescan + watcher", ["index"])
by_binary("Gi-lib", "git_client: status, stage, commit, undo, revert, push/pull, watcher, progreso, cancelación, errores", ["git_client"])
by_binary("F1-lib", "Sesión: serialización", ["gridterm"], r"session")
by_binary("H-lib", "Diff por hunks y staging parcial (hunks, líneas, nuevos, borrados, CRLF, sin salto final, obsoletos)", ["staging"])
by_binary("L6", "Detección de URLs y rutas", ["terminal"], r"links::")
by_binary("Th4", "Contraste WCAG AA de temas y paletas", ["gridterm", "terminal"], r"wcag")
by_binary("R-lib", "Pesos del grid: arrastre, mínimo, reinicio", ["layout"], r"weights|drag")
by_binary("N4-lib", "Sin rounded_* en el código", ["gridterm"], r"no_rounded")
by_binary("X-lib", "Índice con tope de archivos", ["index"], r"max_files")
by_binary("I-lib", "Clasificación texto/teclas especiales (IME)", ["terminal"], r"text_input")
by_binary("V-lib", "Visor: carga, binario, truncado por líneas/bytes, lenguaje; resaltado en segundo plano", ["gridterm"], r"viewer::|highlight::")
by_binary("V12-lib", "Watcher de un archivo (contenido, reemplazo atómico, borrado; no hermanos)", ["fs_tools"], r"file_watcher")
cargo("perf_t3.log", {"T3": ("Primer output de /bin/sh < 50 ms", "t3_first_output")}, "perf")
cargo("perf_s6.log", {"S6": ("100k archivos < 1 s, tecla < 16 ms", "s6_index_100k")}, "perf")
cargo("perf_gi13.log", {"Gi13": ("git status 10k archivos < 100 ms", "gi13_status")}, "perf")
order = lambda r: (re.sub(r"\d.*", "", r[0]), int(re.findall(r"\d+", r[0])[0]) if re.findall(r"\d+", r[0]) else 0, r[0])
rows.sort(key=order)
passed = sum(r[2] == "PASS" for r in rows)
md = [f"# Resultados — {passed}/{len(rows)} PASS", "", "| ID | Caso | Resultado | Detalle | Tipo |", "|---|---|---|---|---|"]
md += [f"| {i} | {n} | {'✅' if s=='PASS' else '❌'} {s} | {d.replace('|','/')} | {k} |" for i, n, s, d, k in rows]
open(os.path.join(out, "RESULTS.md"), "w").write("\n".join(md) + "\n")
print("\n".join(md))
PY
echo; echo "Artifacts: $OUT"
