#!/usr/bin/env bash
#
# build-release.sh — 本地打包 agent-scout release 产物
#
# 用法：
#   ./build-release.sh                 # 当前平台 release 打包到 dist/
#   ./build-release.sh --all           # 尝试交叉编译三平台（需要对应 target）
#   ./build-release.sh --target <t>    # 指定 target 打包
#
# 输出目录：dist/（含二进制 + 文档）

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

VERSION="$(grep '^version' Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
DIST="dist"
mkdir -p "$DIST"

# 主机平台默认 target
default_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os:$arch" in
    Darwin:arm64)   echo "aarch64-apple-darwin" ;;
    Darwin:x86_64)  echo "x86_64-apple-darwin" ;;
    Linux:x86_64)   echo "x86_64-unknown-linux-gnu" ;;
    Linux:aarch64)  echo "aarch64-unknown-linux-gnu" ;;
    MINGW*:x86_64)  echo "x86_64-pc-windows-msvc" ;;
    *) echo "unsupported"; exit 1 ;;
  esac
}

artifact_name() {
  local target="$1"
  case "$target" in
    *apple*)          echo "agent-scout-macos-${target%%-apple*}" ;;
    *linux-gnu*)      echo "agent-scout-linux-${target%%-unknown*}" ;;
    *windows*)        echo "agent-scout-windows-${target%%-pc*}.exe" ;;
    *) echo "agent-scout-$target" ;;
  esac
}

# 确保 target 已安装
ensure_target() {
  local target="$1"
  if ! rustup target list --installed 2>/dev/null | grep -q "^${target}$"; then
    echo ">> 添加 target: $target"
    rustup target add "$target"
  fi
}

build_one() {
  local target="$1"
  local artifact
  artifact="$(artifact_name "$target")"

  echo "=============================================="
  echo " 构建 target: $target  →  $artifact"
  echo "=============================================="
  ensure_target "$target"
  cargo build --release --target "$target"

  local src
  if [[ "$target" == *windows* ]]; then
    src="target/$target/release/agent-scout.exe"
  else
    src="target/$target/release/agent-scout"
  fi

  cp "$src" "$DIST/$artifact"
  cp README.md QUICKSTART.md LICENSE "$DIST/"
  echo ">> 已生成 $DIST/$artifact"
}

ALL_TARGETS=(
  "x86_64-unknown-linux-gnu"
  "aarch64-unknown-linux-gnu"
  "x86_64-apple-darwin"
  "aarch64-apple-darwin"
  "x86_64-pc-windows-msvc"
)

if [[ "${1:-}" == "--all" ]]; then
  for t in "${ALL_TARGETS[@]}"; do
    build_one "$t"
  done
elif [[ "${1:-}" == "--target" ]]; then
  build_one "${2:?usage: $0 --target <target-triple>}"
else
  build_one "$(default_target)"
fi

# 生成 SHA256 校验和（与 CI release 产物一致，方便使用者校验）
if command -v shasum >/dev/null 2>&1; then
  (cd "$DIST" && shasum -a 256 agent-scout-* > SHA256SUMS)
else
  (cd "$DIST" && sha256sum agent-scout-* > SHA256SUMS)
fi

echo
echo "打包完成 → $DIST/"
ls -lh "$DIST"
echo "校验和 → $DIST/SHA256SUMS"