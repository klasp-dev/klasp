#!/bin/sh
# DON'T REMOVE THIS LINE (lefthook)
if [ "$LEFTHOOK" = "0" ]; then
  exit 0
fi

call_lefthook()
{
  if test -n "$LEFTHOOK_BIN"
  then
    "$LEFTHOOK_BIN" "$@"
  elif lefthook -h >/dev/null 2>&1
  then
    lefthook "$@"
  else
    dir="$(git rev-parse --show-toplevel)"
    osArch=$(uname | tr '[:upper:]' '[:lower:]')
    cpuArch=$(uname -m | sed 's/aarch64/arm64/;s/x86_64/x64/')
    if test -f "$dir/node_modules/lefthook-${osArch}-${cpuArch}/bin/lefthook"
    then
      "$dir/node_modules/lefthook-${osArch}-${cpuArch}/bin/lefthook" "$@"
    elif bundle exec lefthook -h >/dev/null 2>&1
    then
      bundle exec lefthook "$@"
    elif yarn lefthook -h >/dev/null 2>&1
    then
      yarn lefthook "$@"
    elif pnpm lefthook -h >/dev/null 2>&1
    then
      pnpm lefthook "$@"
    elif swift package plugin lefthook >/dev/null 2>&1
    then
      swift package --disable-sandbox plugin lefthook "$@"
    elif command -v mint >/dev/null 2>&1
    then
      mint run csjones/lefthook-plugin "$@"
    else
      echo "Can't find lefthook in PATH"
    fi
  fi
}

call_lefthook run "pre-commit" "$@"
