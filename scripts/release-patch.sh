#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

repo="adrianmross/pump"
tap_repo="/Users/adross/dev/adrianmross/homebrew-tap"
version_input="${VERSION:-}"

run_ci=true
run_dist_plan=true
allow_dirty=false
create_tag=false
push_main=false
push_tag=false
dispatch_release=false
watch_release=false
copy_tap=false
validate_tap=false

usage() {
  cat <<'USAGE'
Usage: scripts/release-patch.sh [options]

Calculates the next patch VERSION by default, or accepts VERSION from
--version / VERSION. Runs local release checks and prints the manual release
commands. It does not push, dispatch, edit the tap, or run brew unless the
corresponding explicit flag is provided.

Options:
  --version VERSION      Use VERSION instead of calculating the next patch.
  --tap-repo PATH        Homebrew tap checkout path.
  --allow-dirty         Pass --allow-dirty to dist plan.
  --skip-ci             Do not run make ci.
  --skip-dist-plan      Do not run dist plan --tag.
  --create-tag          Create the local annotated release tag.
  --push-main           Push main to origin.
  --push-tag            Push the release tag to origin.
  --dispatch-release    Dispatch release.yml for the release tag.
  --watch-release       Watch the latest release.yml run.
  --copy-tap            Download pump.rb and copy it into the tap checkout.
  --validate-tap        Run Homebrew style/audit/install/test validation.
  -h, --help            Show this help.

Examples:
  scripts/release-patch.sh
  VERSION=0.1.2 scripts/release-patch.sh
  scripts/release-patch.sh --version v0.1.2 --push-tag --dispatch-release
USAGE
}

die() {
  echo "error: $*" >&2
  exit 1
}

print_command() {
  printf '+'
  while (($#)); do
    printf ' %q' "$1"
    shift
  done
  printf '\n'
}

run_cmd() {
  print_command "$@"
  "$@"
}

run_in_dir() {
  local dir="$1"
  local cmd=()
  shift
  cmd=("$@")
  printf '+ (cd %q &&' "$dir"
  for arg in "${cmd[@]}"; do
    printf ' %q' "$arg"
  done
  printf ')\n'
  (
    cd "$dir"
    "${cmd[@]}"
  )
}

package_version() {
  cargo pkgid | sed 's/.*[#@]//'
}

latest_release_tag() {
  git tag --list 'v[0-9]*' --sort=-version:refname | head -n1
}

next_patch_version() {
  local latest version major minor patch rest

  latest="$(latest_release_tag)"
  if [[ -z "$latest" ]]; then
    package_version
    return
  fi

  version="${latest#v}"
  IFS=. read -r major minor rest <<<"$version"
  patch="${rest%%[-+]*}"

  [[ "$major" =~ ^[0-9]+$ ]] || die "latest tag is not a simple semver tag: $latest"
  [[ "$minor" =~ ^[0-9]+$ ]] || die "latest tag is not a simple semver tag: $latest"
  [[ "$patch" =~ ^[0-9]+$ ]] || die "latest tag is not a simple semver tag: $latest"

  printf '%s.%s.%s\n' "$major" "$minor" "$((patch + 1))"
}

normalize_version() {
  local raw="$1"
  raw="${raw#v}"
  [[ "$raw" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] \
    || die "VERSION must look like vX.Y.Z or X.Y.Z: $1"
  printf '%s\n' "$raw"
}

while (($#)); do
  case "$1" in
    --version)
      (($# >= 2)) || die "--version requires a value"
      version_input="$2"
      shift 2
      ;;
    --tap-repo)
      (($# >= 2)) || die "--tap-repo requires a value"
      tap_repo="$2"
      shift 2
      ;;
    --allow-dirty)
      allow_dirty=true
      shift
      ;;
    --skip-ci)
      run_ci=false
      shift
      ;;
    --skip-dist-plan)
      run_dist_plan=false
      shift
      ;;
    --create-tag)
      create_tag=true
      shift
      ;;
    --push-main)
      push_main=true
      shift
      ;;
    --push-tag)
      push_tag=true
      shift
      ;;
    --dispatch-release)
      dispatch_release=true
      shift
      ;;
    --watch-release)
      watch_release=true
      shift
      ;;
    --copy-tap)
      copy_tap=true
      shift
      ;;
    --validate-tap)
      validate_tap=true
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

if [[ -n "$version_input" ]]; then
  version="$(normalize_version "$version_input")"
else
  version="$(next_patch_version)"
fi

tag="v$version"
pkg_version="$(package_version)"
release_dir="/tmp/pump-release-$tag"

echo "Release helper for $tag"
echo "Package version: $pkg_version"
echo "Tap checkout: $tap_repo"

if [[ "$pkg_version" != "$version" ]]; then
  echo "warning: package version is $pkg_version, but release tag is $tag" >&2
  echo "warning: cargo-dist usually expects Cargo.toml to match the tag version" >&2
fi

if [[ "$run_ci" == true ]]; then
  run_cmd make ci
fi

if [[ "$run_dist_plan" == true ]]; then
  dist_args=(plan --tag "$tag")
  if [[ "$allow_dirty" == true ]]; then
    dist_args+=(--allow-dirty)
  fi
  run_cmd dist "${dist_args[@]}"
fi

cat <<MANUAL

Manual release commands for $tag

# Tag push
git tag -a $tag -m "Release $tag"
git push origin main
git push origin $tag

# Release dispatch
gh workflow run release.yml -f tag=$tag

# Release watch
gh run list --workflow release.yml --limit 5
gh run watch "\$(gh run list --workflow release.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
gh release view $tag --repo $repo

# Tap copy
rm -rf $release_dir
mkdir -p $release_dir
gh release download $tag --repo $repo --pattern pump.rb --dir $release_dir
cp $release_dir/pump.rb $tap_repo/Formula/pump.rb

# Tap validation
(cd $tap_repo && brew style --fix Formula/pump.rb)
(cd $tap_repo && brew style Formula/pump.rb)
brew audit --formula pump --tap=adrianmross/tap
brew install adrianmross/tap/pump
brew test adrianmross/tap/pump
pump --version

# Tap publish after validation
(cd $tap_repo && git status --short && git diff -- Formula/pump.rb)
(cd $tap_repo && git add Formula/pump.rb && git commit -m "Update pump to $tag" && git push origin main)
MANUAL

if [[ "$create_tag" == true ]]; then
  run_cmd git tag -a "$tag" -m "Release $tag"
fi

if [[ "$push_main" == true ]]; then
  run_cmd git push origin main
fi

if [[ "$push_tag" == true ]]; then
  run_cmd git push origin "$tag"
fi

if [[ "$dispatch_release" == true ]]; then
  run_cmd gh workflow run release.yml -f "tag=$tag"
fi

if [[ "$watch_release" == true ]]; then
  run_cmd gh run list --workflow release.yml --limit 5
  run_id="$(gh run list --workflow release.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
  [[ -n "$run_id" && "$run_id" != "null" ]] || die "no release.yml run found to watch"
  run_cmd gh run watch "$run_id"
fi

if [[ "$copy_tap" == true ]]; then
  [[ -d "$tap_repo" ]] || die "tap checkout not found: $tap_repo"
  run_cmd rm -rf "$release_dir"
  run_cmd mkdir -p "$release_dir"
  run_cmd gh release download "$tag" --repo "$repo" --pattern pump.rb --dir "$release_dir"
  run_cmd cp "$release_dir/pump.rb" "$tap_repo/Formula/pump.rb"
fi

if [[ "$validate_tap" == true ]]; then
  [[ -d "$tap_repo" ]] || die "tap checkout not found: $tap_repo"
  run_in_dir "$tap_repo" brew style --fix Formula/pump.rb
  run_in_dir "$tap_repo" brew style Formula/pump.rb
  run_cmd brew audit --formula pump --tap=adrianmross/tap
  run_cmd brew install adrianmross/tap/pump
  run_cmd brew test adrianmross/tap/pump
  run_cmd pump --version
fi
