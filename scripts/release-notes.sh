#!/usr/bin/env bash
# Prints the notes of a GitHub release: the commits since the previous version,
# grouped by Conventional Commits type, followed by .github/release-notes.md.
#
#   scripts/release-notes.sh vX.Y.Z [REF]
#
# REF is the commit being released (default: the tag if it exists, else HEAD).
# The previous version is the closest v* tag before REF; with none, the whole
# history is listed. Merge commits and version bumps (`chore(release): X.Y.Z`,
# or the older `release: X.Y.Z`) are left out.
# Commits that don't follow Conventional Commits go under "Other changes".
set -euo pipefail
cd "$(dirname "$0")/.."

TAG="${1:?usage: scripts/release-notes.sh vX.Y.Z [REF]}"
REF="${2:-}"
if [[ -z $REF ]]; then
  if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then REF=$TAG; else REF=HEAD; fi
fi
git rev-parse -q --verify "$REF^{commit}" >/dev/null || { echo "error: '$REF' is not a commit" >&2; exit 1; }

PREV="$(git describe --tags --abbrev=0 --match 'v[0-9]*' "$REF^" 2>/dev/null || true)"
RANGE="$REF"
[[ -n $PREV ]] && RANGE="$PREV..$REF"

if [[ -n ${GITHUB_SERVER_URL:-} && -n ${GITHUB_REPOSITORY:-} ]]; then
  REPO_URL="$GITHUB_SERVER_URL/$GITHUB_REPOSITORY"
else
  REPO_URL="$(git remote get-url origin 2>/dev/null | sed -E 's#^git@([^:]+):#https://\1/#; s#\.git$##')"
fi

BREAKING="" FEAT="" FIX="" PERF="" OTHER="" COUNT=0
CC='^(feat|fix|perf|refactor|style|test|docs|build|ci|chore|revert)(\(([^)]+)\))?(!)?: (.+)$'
while IFS=$'\x1f' read -r -d $'\x1e' sha subject body; do
  sha="${sha//$'\n'/}"
  [[ -z $sha ]] && continue
  short="${sha:0:7}"
  [[ $subject =~ ^release:\ [0-9] ]] && continue # pre-Conventional Commits version bumps
  if [[ $subject =~ $CC ]]; then
    type="${BASH_REMATCH[1]}" scope="${BASH_REMATCH[3]}" bang="${BASH_REMATCH[4]}" desc="${BASH_REMATCH[5]}"
    [[ $type == chore && $scope == release ]] && continue
    line="- ${scope:+**$scope:** }$desc ($short)"
    if [[ -n $bang ]] || grep -qE '^BREAKING[ -]CHANGE:' <<<"$body"; then
      note="$(sed -n -E 's/^BREAKING[ -]CHANGE: *//p' <<<"$body" | head -1)"
      BREAKING+="- ${scope:+**$scope:** }${note:-$desc} ($short)"$'\n'
    fi
    case $type in
      feat) FEAT+="$line"$'\n' ;;
      fix) FIX+="$line"$'\n' ;;
      perf) PERF+="$line"$'\n' ;;
      *) OTHER+="$line"$'\n' ;;
    esac
  else
    OTHER+="- $subject ($short)"$'\n'
  fi
  COUNT=$((COUNT + 1))
done < <(git log --no-merges --format='%H%x1f%s%x1f%b%x1e' "$RANGE")

section() { [[ -n $2 ]] && printf '### %s\n\n%s\n' "$1" "$2"; return 0; }

echo "## What's changed"
echo
if [[ $COUNT == 0 ]]; then
  echo "No changes${PREV:+ since $PREV}."
  echo
else
  section "⚠️ Breaking changes" "$BREAKING"
  section "Features" "$FEAT"
  section "Bug fixes" "$FIX"
  section "Performance" "$PERF"
  section "Other changes" "$OTHER"
fi
if [[ -n $PREV && -n $REPO_URL ]]; then
  echo "**Full changelog:** $REPO_URL/compare/$PREV...$TAG"
  echo
fi
cat .github/release-notes.md
