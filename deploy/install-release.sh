#!/usr/bin/env bash
# Install a prebuilt release package (binary + frontend/dist) to /opt/proxypool.
# Run as root from the extracted archive root: sudo bash install.sh
set -euo pipefail

INSTALL_DIR="${INSTALL_DIR:-/opt/proxypool}"
SERVICE_NAME="${SERVICE_NAME:-proxypool}"
PORT="${PORT:-9090}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_SRC="$SCRIPT_DIR/myproxy"
DIST_SRC="$SCRIPT_DIR/frontend/dist"
UNIT_SRC="$SCRIPT_DIR/deploy/${SERVICE_NAME}.service"
UNIT_DST="/etc/systemd/system/${SERVICE_NAME}.service"

log() { printf '[proxypool] %s\n' "$*"; }
die() { printf '[proxypool] ERROR: %s\n' "$*" >&2; exit 1; }

have_systemd() {
  command -v systemctl >/dev/null 2>&1 || return 1
  [[ -d /run/systemd/system ]] && return 0
  [[ "$(cat /proc/1/comm 2>/dev/null || true)" == "systemd" ]] && return 0
  return 1
}

[[ "$(id -u)" -eq 0 ]] || die "请用 root 运行: sudo bash $0"
[[ -x "$BIN_SRC" ]] || die "找不到可执行文件: $BIN_SRC"
[[ -f "$DIST_SRC/index.html" ]] || die "找不到前端产物: $DIST_SRC/index.html"
[[ -f "$UNIT_SRC" ]] || die "找不到 systemd 单元: $UNIT_SRC"
have_systemd || die "当前系统没有 systemd"
command -v install >/dev/null 2>&1 || die "缺少命令: install"
command -v systemctl >/dev/null 2>&1 || die "缺少命令: systemctl"

if systemctl cat "$SERVICE_NAME" >/dev/null 2>&1; then
  log "停止旧服务 ${SERVICE_NAME}"
  systemctl stop "$SERVICE_NAME" || true
fi

log "安装到 $INSTALL_DIR"
mkdir -p "$INSTALL_DIR/frontend" "$INSTALL_DIR/data"
install -m 0755 "$BIN_SRC" "$INSTALL_DIR/myproxy"
rm -rf "$INSTALL_DIR/frontend/dist"
cp -a "$DIST_SRC" "$INSTALL_DIR/frontend/dist"

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
