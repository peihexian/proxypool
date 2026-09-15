#!/usr/bin/env bash
# Build backend + frontend from the repo parent of this directory,
# install to /opt/proxypool, and enable the systemd unit.
set -euo pipefail

INSTALL_DIR="${INSTALL_DIR:-/opt/proxypool}"
SERVICE_NAME="${SERVICE_NAME:-proxypool}"
PORT="${PORT:-9090}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BACKEND_DIR="$ROOT_DIR/backend"
FRONTEND_DIR="$ROOT_DIR/frontend"
UNIT_SRC="$SCRIPT_DIR/${SERVICE_NAME}.service"
UNIT_DST="/etc/systemd/system/${SERVICE_NAME}.service"

log() { printf '[proxypool] %s\n' "$*"; }
die() { printf '[proxypool] ERROR: %s\n' "$*" >&2; exit 1; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "缺少命令: $1"
}

load_toolchains() {
  if [[ -f "${HOME}/.cargo/env" ]]; then
    # shellcheck source=/dev/null
    source "${HOME}/.cargo/env"
  fi
  if [[ -s "${HOME}/.nvm/nvm.sh" ]]; then
    # shellcheck source=/dev/null
    source "${HOME}/.nvm/nvm.sh"
  fi
}

have_systemd() {
  command -v systemctl >/dev/null 2>&1 || return 1
  # /run/systemd/system is a directory while systemd is PID 1, not a regular file.
  [[ -d /run/systemd/system ]] && return 0
  [[ "$(cat /proc/1/comm 2>/dev/null || true)" == "systemd" ]] && return 0
  return 1
}

[[ "$(id -u)" -eq 0 ]] || die "请用 root 运行: sudo bash $0"
[[ -d "$BACKEND_DIR" ]] || die "找不到 backend 目录: $BACKEND_DIR"
[[ -d "$FRONTEND_DIR" ]] || die "找不到 frontend 目录: $FRONTEND_DIR"
[[ -f "$UNIT_SRC" ]] || die "找不到 systemd 单元: $UNIT_SRC"
have_systemd || die "当前系统没有 systemd"

load_toolchains
need_cmd cargo
need_cmd npm
need_cmd install
need_cmd systemctl

log "编译 backend release: $BACKEND_DIR"
(
  cd "$BACKEND_DIR"
  cargo build --release
)
BIN="$BACKEND_DIR/target/release/myproxy"
[[ -x "$BIN" ]] || die "未生成可执行文件: $BIN"

log "构建 frontend: $FRONTEND_DIR"
(
  cd "$FRONTEND_DIR"
  npm install
  npm run build
)
[[ -f "$FRONTEND_DIR/dist/index.html" ]] || die "前端构建失败，缺少 dist/index.html"

if systemctl is-active --quiet "$SERVICE_NAME"; then
  log "停止已运行的 ${SERVICE_NAME}"
  systemctl stop "$SERVICE_NAME"
fi

log "安装到 $INSTALL_DIR"
mkdir -p "$INSTALL_DIR/frontend" "$INSTALL_DIR/data"
install -m 0755 "$BIN" "$INSTALL_DIR/myproxy"
rm -rf "$INSTALL_DIR/frontend/dist"
cp -a "$FRONTEND_DIR/dist" "$INSTALL_DIR/frontend/dist"

log "安装 systemd 单元: $UNIT_DST"
install -m 0644 "$UNIT_SRC" "$UNIT_DST"
systemctl daemon-reload
systemctl enable --now "$SERVICE_NAME"

log "部署完成"
log "管理端: http://0.0.0.0:${PORT}  （默认密码 admin）"
log "数据目录: ${INSTALL_DIR}/data"
log "查看状态: systemctl status ${SERVICE_NAME}"
log "查看日志: journalctl -u ${SERVICE_NAME} -f"
systemctl --no-pager --full status "$SERVICE_NAME" || true
