#!/usr/bin/env bash
#
# install.sh — agent-scout 一键安装脚本（开箱即用）
#
# 从 GitHub Releases 下载当前平台的最新版二进制，安装到用户目录并激活到 PATH。
# 无需 Rust 工具链、无需手动编译。
#
# 用法：
#   curl -fsSL https://raw.githubusercontent.com/tasselx/agent-scout/main/install.sh | bash
#   # 或本地执行：
#   ./install.sh [--version vX.Y.Z] [--dir <安装目录>]
#
# 环境变量可覆盖默认值：
#   AGENT_SCOUT_REPO   GitHub 仓库，默认见下方 REPO
#   AGENT_SCOUT_PREFIX 安装目录，默认 ~/.local

set -euo pipefail

# ============ 可配置项 ============
REPO="${AGENT_SCOUT_REPO:-tasselx/agent-scout}"
if [[ -z "$REPO" ]]; then
  echo "install.sh: 未指定 GitHub 仓库，请设置环境变量 AGENT_SCOUT_REPO=<owner>/<repo>" >&2
  echo "示例: AGENT_SCOUT_REPO=tasselx/agent-scout curl -fsSL ... | bash" >&2
  exit 1
fi

VERSION="${1:-latest}"
if [[ "$VERSION" == "--version" ]]; then
  VERSION="${2:-latest}"
fi

PREFIX="${AGENT_SCOUT_PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
BIN="$BIN_DIR/agent-scout"

# ============ 平台/架构检测 ============
detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os:$arch" in
    Darwin:arm64)  echo "macos-aarch64" ;;
    Darwin:x86_64) echo "macos-x86_64" ;;
    Linux:x86_64)  echo "linux-x86_64" ;;
    Linux:aarch64) echo "linux-aarch64" ;;
    MINGW*:x86_64 | MSYS*:x86_64 | CYGWIN*:x86_64) echo "windows-x86_64" ;;
    *) echo "unsupported" >&2; echo "不支持的平台: $os / $arch" >&2; exit 1 ;;
  esac
}

TARGET="$(detect_target)"
if [[ "$TARGET" == windows-* ]]; then
  ARTIFACT="agent-scout-windows-x86_64.exe"
else
  ARTIFACT="agent-scout-${TARGET}"
fi

echo ">> 仓库:      $REPO"
echo ">> 版本:      $VERSION"
echo ">> 平台:      $TARGET"
echo ">> 安装到:    $BIN"

# ============ 下载 ============
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARTIFACT}"
echo ">> 下载: $URL"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if command -v curl >/dev/null 2>&1; then
  curl -fsSL "$URL" -o "$TMP/$ARTIFACT"
elif command -v wget >/dev/null 2>&1; then
  wget -q "$URL" -O "$TMP/$ARTIFACT"
else
  echo "需要 curl 或 wget 才能下载" >&2
  exit 1
fi

# ============ 安装 ============
mkdir -p "$BIN_DIR"
install -m 0755 "$TMP/$ARTIFACT" "$BIN"

# ============ 校验 ============
if command -v "$BIN" >/dev/null 2>&1; then
  echo ">> 已安装: $BIN"
  "$BIN" --help 2>&1 | head -5 || true
else
  echo ">> 安装完成，但 $BIN 不在 PATH 中" >&2
fi

# ============ PATH 提示 ============
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    echo
    echo "提示: 将 $BIN_DIR 加入 PATH 后即可直接使用:"
    echo "  export PATH=\"$BIN_DIR:\$PATH\""
    ;;
esac
