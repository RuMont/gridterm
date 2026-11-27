#!/usr/bin/env bash
# Runs GitHub's release pipeline (.github/workflows/release.yml, which calls
# ci.yml) on this Mac, step by step, and reports every result.
#
# Usage: scripts/release-local.sh [vX.Y.Z] [options]
#
#   vX.Y.Z          Tag to release. Default: v<version in Cargo.toml>.
#   --ref <rev>     Commit to test. Default: the tag if it exists locally, else HEAD.
#   --skip-ci       Skip clippy and tests (only check, package and verify).
#   --keep-going    Keep running the CI steps after one fails, to see every failure.
#   --strict        Local-only checks (fmt, cargo deny, branch) fail the run instead of warning.
#   --fresh         Delete the cached checkout and its build cache first (cold build).
#   --publish       Also create the GitHub release (gh release create), as the workflow does.
#   -h, --help      This help.
#
# How it mirrors GitHub:
#   - It tests the committed tree, never the working tree: a git worktree at
#     target/deploy/checkout is reset to the commit (like actions/checkout).
#     Its target/ is kept between runs (like Swatinem/rust-cache); --fresh drops it.
#     Your own target/release/gridterm is never touched.
#   - Steps, in the workflow's order (stops at the first failing step, like a job):
#       checkout    local        reset the worktree to the commit
#       version     release.yml  scripts/package.sh --check <tag>
#       notes       release.yml  scripts/release-notes.sh <tag> <commit>  (printed, saved as release-notes.md)
#       fmt         local only   cargo fmt --all --check                (warn)
#       deny        local only   cargo deny check                       (warn)
#       branch      local only   git-flow: on release/* or hotfix/*, or a commit on master (warn)
#       clippy      ci.yml       cargo clippy --workspace --all-targets -- -D warnings
#       clippy-e2e  ci.yml       cargo clippy -p gridterm --all-targets --features e2e -- -D warnings
#       test        ci.yml       cargo test --workspace --no-fail-fast
#       package     release.yml  scripts/package.sh <tag>  (release build, gridterm.app, zip)
#       verify      local only   zip contents, Info.plist version, code signature
#       publish     release.yml  gh release create (only with --publish)
#   - Preflight warns when the workflows contain steps this script does not mirror.
#
# Publishing: pushing the tag already makes GitHub publish the release. Use
# --publish only when GitHub Actions cannot; it requires the tag on origin,
# pointing at the tested commit, and no existing release for it.
#
# Output, for humans and agents:
#   - Each step prints "==> [n/N] name", its command, its log file, and PASS / FAIL /
#     WARN / SKIP with the time. On failure it prints the relevant log lines
#     (errors, warnings, --> locations, failed tests, panics) and the log's tail.
#   - target/deploy/runs/<timestamp>/ (also runs/latest): NN-<step>.log,
#     run.log (this output), summary.txt, summary.json and the zip.
#   - The last line is always: RESULT: PASS|FAIL step=<failed step or -> ...
#
# Exit codes: 0 all blocking steps passed; 1 a blocking step failed;
#             2 usage or preflight error (nothing was run).

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 2
export CARGO_TERM_COLOR=never

TAG="" REF="" SKIP_CI=0 KEEP_GOING=0 STRICT=0 FRESH=0 PUBLISH=0
usage() { awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; }
while [[ $# -gt 0 ]]; do
  case $1 in
    -h | --help) usage; exit 0 ;;
    --ref) REF="${2:?--ref needs a revision}"; shift ;;
    --skip-ci) SKIP_CI=1 ;;
    --keep-going) KEEP_GOING=1 ;;
    --strict) STRICT=1 ;;
    --fresh) FRESH=1 ;;
    --publish) PUBLISH=1 ;;
    v[0-9]*) TAG=$1 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 2 ;;
  esac
  shift
done

DEPLOY="$ROOT/target/deploy"
WT="$DEPLOY/checkout"
RUN="$DEPLOY/runs/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$RUN" && ln -sfn "$RUN" "$DEPLOY/runs/latest"
LOG="$RUN/run.log"

say() { printf '%s\n' "$*" | tee -a "$LOG"; }
indent() { sed 's/^/    /' | tee -a "$LOG"; }
WARNINGS=0
warn() { say "    WARN: $*"; WARNINGS=$((WARNINGS + 1)); }
die() { say "error: $*"; say "RESULT: FAIL step=preflight run=$RUN"; exit 2; }

# ---- preflight ---------------------------------------------------------------

say "==> preflight"
for tool in git cargo rustc ditto codesign shasum unzip; do
  command -v "$tool" >/dev/null || die "'$tool' not found"
done
[[ $PUBLISH == 1 ]] && { command -v gh >/dev/null || die "'gh' not found (needed by --publish)"; }

BRANCH="$(git symbolic-ref --short -q HEAD || echo '(detached)')"
if [[ -z $REF ]]; then
  if [[ -n $TAG ]] && git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then REF=$TAG; else REF=HEAD; fi
fi
SHA="$(git rev-parse -q --verify "$REF^{commit}")" || die "'$REF' is not a commit"
SHORT="${SHA:0:9}"
VERSION="$(git show "$SHA:Cargo.toml" | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)"
[[ -n $VERSION ]] || die "no version in Cargo.toml at $SHORT"
[[ -n $TAG ]] || TAG="v$VERSION"

say "    repo:      $ROOT (branch $BRANCH, HEAD $(git rev-parse --short=9 HEAD))"
say "    testing:   $REF = $SHORT  $(git log -1 --format=%s "$SHA")"
say "    tag:       $TAG   Cargo.toml version at $SHORT: $VERSION"
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  TAG_SHA="$(git rev-parse "$TAG^{commit}")"
  [[ $TAG_SHA == "$SHA" ]] && say "    tag $TAG exists locally and points at the tested commit" \
    || warn "local tag $TAG points at ${TAG_SHA:0:9}, not at the tested commit $SHORT"
else
  say "    tag $TAG does not exist yet: testing $SHORT as if it were tagged"
fi
DIRTY="$(git status --porcelain | wc -l | tr -d ' ')"
[[ $DIRTY == 0 ]] || warn "$DIRTY uncommitted/untracked path(s) in the repo; they are NOT part of this run"
say "    toolchain: $(rustc --version)  ($(cargo --version))"
if command -v rustup >/dev/null; then
  update="$(rustup check 2>/dev/null | grep -E '^stable.*Update available' || true)"
  [[ -z $update ]] && say "    GitHub uses the latest stable; local stable is up to date" \
    || warn "GitHub builds with the latest stable, newer than yours: $update (rustup update)"
fi
say "    host:      macOS $(sw_vers -productVersion) $(uname -m); GitHub: macos-latest arm64"
say "    run dir:   $RUN"

# Steps the workflows run that this script does not know about.
KNOWN_RUNS='scripts/package.sh --check "$GITHUB_REF_NAME"
scripts/release-notes.sh "$GITHUB_REF_NAME" > "$RUNNER_TEMP/notes.md"
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p gridterm --all-targets --features e2e -- -D warnings
cargo test --workspace --no-fail-fast
|'
WORKFLOWS="$(git show "$SHA:.github/workflows/ci.yml" "$SHA:.github/workflows/release.yml" 2>/dev/null)" \
  || die "cannot read .github/workflows/{ci,release}.yml at $SHORT"
drift=0
while IFS= read -r line; do
  grep -qxF -- "$line" <<<"$KNOWN_RUNS" || { warn "workflow step not mirrored here: run: $line"; drift=1; }
done < <(sed -n 's/^[[:space:]]*run:[[:space:]]*//p' <<<"$WORKFLOWS")
for needle in 'zip="$(scripts/package.sh "$GITHUB_REF_NAME")"' 'gh release create "$GITHUB_REF_NAME" "$ZIP"' '--notes-file "$RUNNER_TEMP/notes.md"'; do
  grep -qF -- "$needle" <<<"$WORKFLOWS" || { warn "workflow no longer contains: $needle"; drift=1; }
done
[[ $drift == 0 ]] && say "    workflows: every run step is mirrored by this script" \
  || warn "update scripts/release-local.sh to match the workflows"

# ---- steps -------------------------------------------------------------------

PLAN="checkout version notes fmt deny branch clippy clippy-e2e test package verify"
[[ $PUBLISH == 1 ]] && PLAN="$PLAN publish"
TOTAL=$(wc -w <<<"$PLAN" | tr -d ' ')

# name -> "kind|origin|command shown" (kind: setup, ci, extra, check)
step_info() {
  case $1 in
    checkout) echo "setup|actions/checkout|reset $WT to $SHORT" ;;
    version) echo "ci|release.yml › version|scripts/package.sh --check $TAG" ;;
    notes) echo "ci|release.yml › Release notes|scripts/release-notes.sh $TAG $SHORT" ;;
    fmt) echo "extra|local only|cargo fmt --all --check" ;;
    deny) echo "extra|local only|cargo deny check" ;;
    branch) echo "extra|local only|git-flow: release/*, hotfix/* or master" ;;
    clippy) echo "ci|ci.yml › Clippy|cargo clippy --workspace --all-targets -- -D warnings" ;;
    clippy-e2e) echo "ci|ci.yml › Clippy (e2e runner)|cargo clippy -p gridterm --all-targets --features e2e -- -D warnings" ;;
    test) echo "ci|ci.yml › Tests|cargo test --workspace --no-fail-fast" ;;
    package) echo "ci|release.yml › Build and package|scripts/package.sh $TAG" ;;
    verify) echo "check|local only|zip contents, Info.plist version, codesign" ;;
    publish) echo "ci|release.yml › Create the release|gh release create $TAG <zip> --verify-tag ..." ;;
  esac
}

prepare_checkout() {
  if [[ $FRESH == 1 && -d $WT ]]; then
    echo "--fresh: removing $WT and its build cache"
    git worktree remove --force "$WT" || return 1
  fi
  git worktree prune
  if [[ -d $WT ]]; then
    git -C "$WT" checkout --detach --force "$SHA" && git -C "$WT" clean -ffdx -e /target || return 1
    echo "reused checkout (build cache kept: $WT/target)"
  else
    git worktree add --detach "$WT" "$SHA" || return 1
    echo "new checkout (cold build)"
  fi
  git -C "$WT" log -1 --format='%H %s'
}

check_branch() {
  local where
  where="$(git branch --contains "$SHA" --format='%(refname:short)' | tr '\n' ' ')"
  echo "branches containing $SHORT: ${where:-none}"
  echo "current branch: $BRANCH"
  if git merge-base --is-ancestor "$SHA" master 2>/dev/null; then
    echo "ok: $SHORT is on master"
    return 0
  fi
  case $BRANCH in
    release/* | hotfix/*) echo "ok: releases are cut from $BRANCH" ;;
    *) echo "git-flow: release from release/X.Y.Z or hotfix/X.Y.Z, or test a commit on master; $SHORT is on neither ('$BRANCH')"; return 1 ;;
  esac
}

zip_path() { cat "$RUN/zip-path" 2>/dev/null; }

verify_artifact() {
  local zip app plist v listing dots
  zip="$(zip_path)"
  [[ -n $zip && -f $zip ]] || { echo "error: package.sh did not print an existing zip (got '$zip')"; return 1; }
  ls -l "$zip"
  shasum -a 256 "$zip" | tee "$RUN/zip.sha256"
  # List first: `unzip | grep -q` fails at random under pipefail (SIGPIPE).
  listing="$(unzip -l "$zip")" || { echo "error: unzip -l failed"; return 1; }
  grep -qE 'gridterm\.app/Contents/MacOS/gridterm$' <<<"$listing" \
    || { echo "error: zip lacks gridterm.app/Contents/MacOS/gridterm"; head -20 <<<"$listing"; return 1; }
  echo "zip contains gridterm.app/Contents/MacOS/gridterm"
  dots="$(grep -c '/\._' <<<"$listing" || true)"
  [[ $dots == 0 ]] || echo "note: $dots AppleDouble (._*) entries: extended attributes of the local files, which ditto keeps; a GitHub build may have none"
  app="$WT/target/release/bundle/gridterm.app"
  plist="$app/Contents/Info.plist"
  v="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist")" || return 1
  [[ $v == "$VERSION" ]] || { echo "error: Info.plist version $v != Cargo.toml $VERSION"; return 1; }
  echo "Info.plist version: $v"
  file "$app/Contents/MacOS/gridterm"
  if codesign --verify --deep --strict "$app"; then echo "codesign: ad-hoc signature valid"
  else echo "warning: codesign --verify failed (package.sh only warns about this too)"; fi
  cp "$zip" "$RUN/" && echo "copied to $RUN/$(basename "$zip")"
}

publish_release() {
  local remote
  gh auth status || { echo "error: gh is not logged in: gh auth login"; return 1; }
  remote="$(git ls-remote origin "refs/tags/$TAG^{}" | cut -f1)"
  [[ -n $remote ]] || remote="$(git ls-remote origin "refs/tags/$TAG" | cut -f1)"
  [[ -n $remote ]] || { echo "error: tag $TAG is not on origin (--verify-tag needs it): git push origin $TAG"; return 1; }
  [[ $remote == "$SHA" ]] || { echo "error: origin's $TAG is ${remote:0:9}, but this run tested $SHORT"; return 1; }
  if gh release view "$TAG" >/dev/null 2>&1; then
    echo "error: release $TAG already exists on GitHub (the Release workflow probably published it)"
    return 1
  fi
  gh release create "$TAG" "$(zip_path)" --verify-tag --title "gridterm ${TAG#v}" \
    --notes-file "$RUN/release-notes.md"
}

step_run() {
  case $1 in
    checkout) prepare_checkout ;;
    version) scripts/package.sh --check "$TAG" ;;
    notes) scripts/release-notes.sh "$TAG" "$SHA" | tee "$RUN/release-notes.md" ;;
    fmt) cargo fmt --all --check ;;
    deny) cargo deny check ;;
    branch) check_branch ;;
    clippy) cargo clippy --workspace --all-targets -- -D warnings ;;
    clippy-e2e) cargo clippy -p gridterm --all-targets --features e2e -- -D warnings ;;
    test) cargo test --workspace --no-fail-fast ;;
    package) scripts/package.sh "$TAG" >"$RUN/zip-path.rel" && echo "$WT/$(cat "$RUN/zip-path.rel")" >"$RUN/zip-path" ;;
    verify) verify_artifact ;;
    publish) publish_release ;;
  esac
}

# Relevant lines of a failed step's log, then its tail.
digest() {
  local hits
  hits="$(grep -nE '^(error|warning|note)(\[[A-Za-z0-9_:]+\])?:|^ *--> |panicked at|^test .* FAILED$|^---- .* ----$|^failures:$|could not compile|^Diff in |^error: |not found|does not match' "$1" | head -n 40)"
  if [[ -n $hits ]]; then
    say "    --- relevant lines (log line:text) ---"
    indent <<<"$hits"
  fi
  # A short log was already shown in full by the relevant lines.
  if [[ -z $hits || $(wc -l <"$1") -gt $(wc -l <<<"$hits") ]]; then
    say "    --- last 15 lines of the log ---"
    tail -n 15 "$1" | indent
  fi
}

# --keep-going only continues across these: they don't depend on each other.
is_check() { case $1 in clippy | clippy-e2e | test) return 0 ;; esac; return 1; }

RESULTS=""   # lines: name|status|seconds|note
record() { RESULTS="$RESULTS$1|$2|$3|$4"$'\n'; }
FAILED="-"
N=0
for name in $PLAN; do
  N=$((N + 1))
  IFS='|' read -r kind origin shown <<<"$(step_info "$name")"
  say ""
  say "==> [$N/$TOTAL] $name  ($origin)"
  if [[ $FAILED != "-" ]] && ! { [[ $KEEP_GOING == 1 ]] && is_check "$name" && is_check "$FAILED"; }; then
    say "    NOT RUN (stopped after '$FAILED' failed)"
    record "$name" "NOT RUN" 0 "after $FAILED"
    continue
  fi
  if [[ $SKIP_CI == 1 && ($name == clippy || $name == clippy-e2e || $name == test) ]]; then
    say "    SKIP (--skip-ci)"
    record "$name" SKIP 0 "--skip-ci"
    continue
  fi
  if [[ $name == deny ]] && ! cargo deny --version >/dev/null 2>&1; then
    say "    SKIP (cargo-deny not installed: brew install cargo-deny)"
    record "$name" SKIP 0 "cargo-deny missing"
    continue
  fi
  steplog="$RUN/$(printf '%02d' "$N")-$name.log"
  say "    \$ $shown"
  say "    log: $steplog"
  dir=$WT
  [[ $name == checkout || $name == branch ]] && dir=$ROOT
  start=$SECONDS
  (cd "$dir" && step_run "$name") >"$steplog" 2>&1
  rc=$?
  secs=$((SECONDS - start))
  if [[ $rc == 0 ]]; then
    say "    PASS (${secs}s)"
    if [[ $name == notes ]]; then
      say "    --- release notes (until the install instructions; full text: $RUN/release-notes.md) ---"
      sed '/^## Install/,$d' "$steplog" | indent
    fi
    [[ $name == verify ]] && grep -E '^[0-9a-f]{64} |Info.plist|codesign|^note|^copied' "$steplog" | indent
    record "$name" PASS "$secs" ""
    continue
  fi
  status=FAIL
  [[ $kind == extra && $STRICT == 0 ]] && status=WARN
  say "    $status (exit $rc, ${secs}s)"
  digest "$steplog"
  record "$name" "$status" "$secs" "exit $rc, $steplog"
  if [[ $status == WARN ]]; then WARNINGS=$((WARNINGS + 1)); else [[ $FAILED == "-" ]] && FAILED=$name; fi
done

# ---- summary -----------------------------------------------------------------

{
  printf '%-11s %-8s %6s  %s\n' step status time note
  while IFS='|' read -r name status secs note; do
    [[ -n $name ]] && printf '%-11s %-8s %5ss  %s\n' "$name" "$status" "$secs" "$note"
  done <<<"$RESULTS"
} >"$RUN/summary.txt"

{
  printf '{\n  "result": "%s",\n  "failed_step": "%s",\n  "tag": "%s",\n  "version": "%s",\n  "commit": "%s",\n' \
    "$([[ $FAILED == "-" ]] && echo PASS || echo FAIL)" "$FAILED" "$TAG" "$VERSION" "$SHA"
  printf '  "zip": "%s",\n  "warnings": %s,\n  "run_dir": "%s",\n  "steps": [\n' "$(ls "$RUN"/*.zip 2>/dev/null | head -1)" "$WARNINGS" "$RUN"
  first=1
  while IFS='|' read -r name status secs note; do
    [[ -z $name ]] && continue
    [[ $first == 1 ]] || printf ',\n'
    first=0
    printf '    {"step": "%s", "status": "%s", "seconds": %s, "note": "%s"}' "$name" "$status" "$secs" "$note"
  done <<<"$RESULTS"
  printf '\n  ]\n}\n'
} >"$RUN/summary.json"

say ""
say "==> summary ($RUN)"
indent <"$RUN/summary.txt"
zip="$(ls "$RUN"/*.zip 2>/dev/null | head -1)"
[[ -n $zip ]] && say "    zip: $zip"
[[ $WARNINGS -gt 0 ]] && say "    $WARNINGS warning(s); see above"
if [[ $FAILED == "-" ]]; then
  say "RESULT: PASS step=- tag=$TAG commit=$SHORT warnings=$WARNINGS run=$RUN"
  exit 0
fi
say "RESULT: FAIL step=$FAILED tag=$TAG commit=$SHORT log=$(ls "$RUN"/*-"$FAILED".log) run=$RUN"
exit 1
