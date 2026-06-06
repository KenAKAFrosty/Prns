# Shared helper for the energy harnesses (`energy/build.sh` sources this). We never vendor
# upstream source — clone the pinned upstream into a gitignored `external/<impl>/.upstream/`.

# clone_pinned <repo-url> <ref> <dest-dir>: full clone + checkout the pinned ref (idempotent).
clone_pinned() {
  local repo="$1" ref="$2" dest="$3"
  if [ ! -d "$dest/.git" ]; then
    git clone "$repo" "$dest"
  fi
  git -C "$dest" checkout --quiet "$ref"
}
