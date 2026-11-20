#!/usr/bin/env bash
# Real-project checks for java-analyzer: see README.md in this directory.
#
#   scripts/real-projects/run.sh [check...]
#
# Everything (Maven, JDKs, the isolated HOME with its local repository, project checkouts,
# logs) lives under $JA_REALPROJ_WORK, by default ~/.cache/ja-realproj. The real ~/.m2 and
# user configuration are never touched.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$here/../.." && pwd)
work=${JA_REALPROJ_WORK:-$HOME/.cache/ja-realproj}
maven_version=3.9.9
mkdir -p "$work"
work=$(cd "$work" && pwd)

case $(uname -m) in
  x86_64 | amd64) arch=x64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) echo "unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac
[ "$(uname -s)" = Linux ] || { echo "run.sh only supports Linux" >&2; exit 1; }

# Maven
maven=$work/apache-maven-$maven_version
if [ ! -x "$maven/bin/mvn" ]; then
  echo "downloading Maven $maven_version"
  curl -fsSL "https://archive.apache.org/dist/maven/maven-3/$maven_version/binaries/apache-maven-$maven_version-bin.tar.gz" |
    tar -xz -C "$work"
fi

# JDKs: JA_JDK_<feature> points at an existing JDK, otherwise Temurin is downloaded.
# JDK 25 is JAVA_HOME; the others are only discoverable (symlinked into ~/.jdks).
# JDK 6 is optional (Temurin has none): set JA_JDK_6 to include it.
jdk() {
  local v=$1 var="JA_JDK_$1"
  if [ -n "${!var:-}" ]; then echo "${!var}"; return; fi
  local dir=$work/jdks/$v
  if [ ! -x "$dir/bin/java" ]; then
    echo "downloading Temurin JDK $v" >&2
    rm -rf "$dir.tmp" && mkdir -p "$dir.tmp"
    curl -fsSL "https://api.adoptium.net/v3/binary/latest/$v/ga/linux/$arch/jdk/hotspot/normal/eclipse" |
      tar -xz -C "$dir.tmp" --strip-components=1
    mv "$dir.tmp" "$dir"
  fi
  echo "$dir"
}
jdks=""
for v in 8 11 17 21 25; do jdks="$jdks $v=$(jdk $v)"; done
[ -n "${JA_JDK_6:-}" ] && jdks="$jdks 6=$JA_JDK_6"

# java-analyzer binary
if [ -z "${JA_BIN:-}" ]; then
  (cd "$repo" && cargo build -q --release -p java-analyzer)
  JA_BIN=${CARGO_TARGET_DIR:-$repo/target}/release/java-analyzer
fi

export JA_REALPROJ_WORK=$work JA_REALPROJ_MAVEN=$maven JA_REALPROJ_JDKS="${jdks# }" JA_BIN
exec python3 "$here/realproj.py" "$@"
