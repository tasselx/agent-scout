#!/usr/bin/env bash
#
# agent-scout 测试搜索脚本
#
# 覆盖：冒烟测试 / config 子命令 / 错误处理 / 真实搜索 / JSON 可解析性。
# 用法：
#   ./test-search.sh [--quick] [--no-live] [--key <token>]
#
# 选项：
#   --quick     只跑静态/离线部分（config、错误处理），不做真实在线搜索
#   --no-live   跳过一切真实在线搜索
#   --key <t>   显式提供 API key（否则自动提取或使用已配置 key）
#
# 退出码：0=全部通过 1=存在失败 2=用法错误

set -u

# 定位二进制
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="${BIN:-$SCRIPT_DIR/target/debug/agent-scout}"

QUICK=0
NO_LIVE=0
KEY=""
PASS=0
FAIL=0

usage() {
  cat <<'EOF'
agent-scout 测试搜索脚本

覆盖：冒烟测试 / config 子命令 / 错误处理 / 真实搜索 / JSON 可解析性。
用法：
  ./test-search.sh [--quick] [--no-live] [--key <token>]

选项：
  --quick     只跑静态/离线部分（config、错误处理），不做真实在线搜索
  --no-live   跳过一切真实在线搜索
  --key <t>   显式提供 API key（否则自动提取或使用已配置 key）
  -h, --help  显示本帮助

退出码：0=全部通过 1=存在失败 2=用法错误
EOF
}

# 解析参数
while [[ $# -gt 0 ]]; do
  case "$1" in
    --quick) QUICK=1 ;;
    --no-live) NO_LIVE=1 ;;
    --key) KEY="${2:-}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "未知参数: $1" >&2; usage; exit 2 ;;
  esac
  shift
done

# 工具函数：断言 + 统计
check() {
  local desc="$1" got="$2" want="$3"
  if [[ "$got" == "$want" ]]; then
    echo "  ✅ $desc"
    PASS=$((PASS + 1))
  else
    echo "  ❌ $desc (期望 $want, 实际 $got)"
    FAIL=$((FAIL + 1))
  fi
}

check_contains() {
  local desc="$1" haystack="$2" needle="$3"
  if [[ "$haystack" == *"$needle"* ]]; then
    echo "  ✅ $desc"
    PASS=$((PASS + 1))
  else
    echo "  ❌ $desc (未找到 '$needle')"
    FAIL=$((FAIL + 1))
  fi
}

# 前置检查
if [[ ! -x "$BIN" ]]; then
  echo "错误: 未找到可执行文件 $BIN" >&2
  echo "请先运行: cargo build" >&2
  exit 1
fi

# 构建 key 参数
KEY_ARGS=()
if [[ -n "$KEY" ]]; then
  KEY_ARGS=(--api-key "$KEY")
fi

echo "=============================================="
echo " agent-scout 测试脚本"
echo " 二进制: $BIN"
[[ -n "$KEY" ]] && echo " 使用显式 --api-key"
echo "=============================================="

# ──────────────────────────────────────────────
echo ""
echo "【1】冒烟测试 (usage / 未知子命令)"
# ──────────────────────────────────────────────
"$BIN" >/dev/null 2>&1
check "无参数 → exit=2" "$?" "2"

"$BIN" config foo >/dev/null 2>&1
check "未知子命令 → exit=2" "$?" "2"

# ──────────────────────────────────────────────
echo ""
echo "【2】config 子命令 (临时 HOME 隔离)"
# ──────────────────────────────────────────────
TMP_HOME="$(mktemp -d)"
OLD_HOME="${HOME:-}"
export HOME="$TMP_HOME"

"$BIN" config set "devin-session-token\$test123" >/dev/null 2>&1
check "config set → exit=0" "$?" "0"

PERMS=$(stat -c "%a" "$TMP_HOME/.config/windsurf-search/api-key" 2>/dev/null || stat -f "%Lp" "$TMP_HOME/.config/windsurf-search/api-key")
check "key 文件权限 = 600" "$PERMS" "600"

SHOW_OUT=$("$BIN" config show 2>&1)
check "config show → exit=0" "$?" "0"
check_contains "config show 显示掩码" "$SHOW_OUT" "…"

"$BIN" config clear >/dev/null 2>&1
check "config clear → exit=0" "$?" "0"

export HOME="$OLD_HOME"
rm -rf "$TMP_HOME"

# ──────────────────────────────────────────────
echo ""
echo "【3】错误处理 (无 key 场景)"
# ──────────────────────────────────────────────
TMP_HOME="$(mktemp -d)"
export HOME="$TMP_HOME"

"$BIN" "some query" >/dev/null 2>&1
check "无任何 key → exit=1" "$?" "1"

ERR_OUT=$("$BIN" "q" --api-key "devin-session-token\$fake" 2>&1)
check "无效 key → exit=1" "$?" "1"
check_contains "无效 key 报 401" "$ERR_OUT" "401"

export HOME="$OLD_HOME"
rm -rf "$TMP_HOME"

# ──────────────────────────────────────────────
echo ""
echo "【4】真实搜索"
# ──────────────────────────────────────────────
if [[ "$QUICK" == "1" || "$NO_LIVE" == "1" ]]; then
  echo "  ⏭  (--quick/--no-live 跳过真实搜索)"
else
  # 真实搜索主命令
  if [[ -n "$KEY" ]]; then
    # 显式 key：CLI 搜索
    OUT=$("$BIN" "tauri window drag region" --limit 2 "${KEY_ARGS[@]:-}" 2>&1)
    RC=$?
    check "CLI 真实搜索 → exit=0" "$RC" "0"
    check_contains "输出含 hits" "$OUT" '"hits"'
    check_contains "hit 含 title" "$OUT" '"title"'
    check_contains "hit 含 source=windsurf" "$OUT" '"source":"windsurf"'
  else
    # 自动提取 key 的场景
    OUT=$("$BIN" config test 'connectivity' 2>&1)
    RC=$?
    check "config test (自动提取 key) → exit=0" "$RC" "0"
    check_contains "config test 显示 OK" "$OUT" "OK: got"

    OUT=$("$BIN" "rust async runtime" --limit 2 2>&1)
    RC=$?
    check "CLI 真实搜索 (自动提取) → exit=0" "$RC" "0"
    check_contains "输出含 hits" "$OUT" '"hits"'
  fi

  # domain 过滤
  OUT=$("$BIN" "tokio" --limit 1 --domain "github.com" "${KEY_ARGS[@]:-}" 2>&1)
  RC=$?
  check "domain 过滤 → exit=0" "$RC" "0"
  check_contains "结果含 github.com" "$OUT" "github.com"

  # JSON 可解析性（管道）
  if command -v python3 >/dev/null 2>&1; then
    PARSED=$("$BIN" "devin" --limit 2 "${KEY_ARGS[@]:-}" 2>/dev/null \
      | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d['hits']))" 2>/dev/null)
    check "stdout 可被 JSON 解析 (hits=$PARSED)" "$?" "0"
  fi
fi

# ──────────────────────────────────────────────
echo ""
echo "【5】MCP 协议冒烟 (NDJSON)"
# ──────────────────────────────────────────────
MCP_OUT=$(printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}\n{"jsonrpc":"2.0","id":2,"method":"tools/list"}\n' \
  | "$BIN" --mcp 2>/dev/null)
check_contains "MCP initialize 响应" "$MCP_OUT" '"serverInfo"'
check_contains "MCP tools/list 含 web_search" "$MCP_OUT" 'web_search'

# ──────────────────────────────────────────────
echo ""
echo "=============================================="
echo " 结果: $PASS 通过, $FAIL 失败"
echo "=============================================="
[[ "$FAIL" -eq 0 ]] && exit 0 || exit 1