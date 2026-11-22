# Installs pi for Pistachio into ~/.pistachio/server without prompts or root access (POSIX sh).
#
# Uses the login shell's Node.js when it is 22.19 or newer (with npm); otherwise downloads the
# official standalone Node.js 22 build for this machine from nodejs.org, checks its SHA-256 and
# keeps it in ~/.pistachio/server/node. pi itself is installed from npm into
# ~/.pistachio/server/pi, and ~/.pistachio/server/bin/pi starts it with that Node.js.
#
# Progress lines start with PISTACHIO_STEP=; the last line is PISTACHIO_INSTALLED=<path to pi>.
set -eu
D=$HOME/.pistachio/server
PKG=@earendil-works/pi-coding-agent
step() { printf 'PISTACHIO_STEP=%s\n' "$1"; }
die() { printf '%s\n' "$1" >&2; exit 1; }
fetch() {
  if command -v curl >/dev/null 2>&1; then curl -fsSL --retry 2 "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then wget -q "$1" -O "$2"
  else die "Neither curl nor wget is installed on this machine."
  fi
}
node_ok() {
  "$1" -e 'const [a,b]=process.versions.node.split(".").map(Number);process.exit(a>22||(a===22&&b>=19)?0:1)' >/dev/null 2>&1
}

LOGIN_PATH
mkdir -p "$D"

step "Looking for Node.js"
NODE_BIN=
if command -v node >/dev/null 2>&1 && command -v npm >/dev/null 2>&1 && node_ok node; then
  NODE_BIN=$(dirname "$(command -v node)")
elif [ -x "$D/node/bin/node" ] && node_ok "$D/node/bin/node"; then
  NODE_BIN=$D/node/bin
else
  case $(uname -s) in Linux) os=linux ;; Darwin) os=darwin ;; *) die "Unsupported system: $(uname -s)" ;; esac
  case $(uname -m) in
    x86_64|amd64) arch=x64 ;; arm64|aarch64) arch=arm64 ;; armv7l) arch=armv7l ;;
    ppc64le) arch=ppc64le ;; s390x) arch=s390x ;; *) die "Unsupported processor: $(uname -m)" ;;
  esac
  base=https://nodejs.org/dist/latest-v22.x
  t=$(mktemp -d "${TMPDIR:-/tmp}/pistachio-node.XXXXXX")
  trap 'rm -rf "$t"' EXIT
  step "Downloading Node.js"
  fetch "$base/SHASUMS256.txt" "$t/SHASUMS256.txt"
  if command -v xz >/dev/null 2>&1; then ext=tar.xz; flag=J; else ext=tar.gz; flag=z; fi
  f=$(awk -v s="-$os-$arch.$ext" 'index($2, "node-v") == 1 && substr($2, length($2) - length(s) + 1) == s { print $2; exit }' "$t/SHASUMS256.txt")
  [ -n "$f" ] || die "No Node.js build is available for $os-$arch."
  fetch "$base/$f" "$t/$f"
  step "Verifying Node.js"
  want=$(awk -v f="$f" '$2 == f { print $1 }' "$t/SHASUMS256.txt")
  if command -v sha256sum >/dev/null 2>&1; then got=$(sha256sum "$t/$f" | cut -d' ' -f1)
  elif command -v shasum >/dev/null 2>&1; then got=$(shasum -a 256 "$t/$f" | cut -d' ' -f1)
  else got=$want
  fi
  [ "$got" = "$want" ] || die "The Node.js download is damaged (checksum mismatch)."
  step "Unpacking Node.js"
  rm -rf "$D/node.new"
  mkdir -p "$D/node.new"
  tar -x${flag}f "$t/$f" -C "$D/node.new"
  rm -rf "$D/node"
  mv "$D/node.new/${f%.$ext}" "$D/node"
  rm -rf "$D/node.new"
  NODE_BIN=$D/node/bin
fi

step "Installing pi"
PATH=$NODE_BIN:$PATH
NPM_CONFIG_UPDATE_NOTIFIER=false
export NPM_CONFIG_UPDATE_NOTIFIER
export PATH
npm install -g --prefix "$D/pi" --ignore-scripts --no-fund --no-audit --loglevel=error \
  --progress=false --min-release-age=0 "$PKG" 2>&1 \
  || npm install -g --prefix "$D/pi" --ignore-scripts --no-fund --no-audit --loglevel=error \
       --progress=false "$PKG" 2>&1
[ -x "$D/pi/bin/pi" ] || die "npm did not install pi."

mkdir -p "$D/bin"
q() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
{
  printf '#!/bin/sh\n'
  printf 'PATH=%s:$PATH\n' "$(q "$NODE_BIN")"
  printf 'export PATH\n'
  printf 'exec %s "$@"\n' "$(q "$D/pi/bin/pi")"
} > "$D/bin/pi.new"
chmod 755 "$D/bin/pi.new"
mv "$D/bin/pi.new" "$D/bin/pi"

step "Checking pi"
"$D/bin/pi" --version </dev/null
printf 'PISTACHIO_INSTALLED=%s\n' "$D/bin/pi"
