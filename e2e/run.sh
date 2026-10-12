#!/usr/bin/env bash
# アプリの E2E（Linux のコンテナ・開発環境用）。
#
# 使い方: e2e/run.sh <アプリの実行ファイル> <シナリオの .mjs> [出力のフォルダ]
#
# - 仮の画面（Xvfb）・tauri-driver（WebKitWebDriver の中継）を起動し、シナリオを実行する。
# - アプリは使い捨ての HOME で起動する（既定のカタログ・設定は HOME の下に作られる）。
# - 必要なもの: Xvfb、WebKitWebDriver（Ubuntu の webkit2gtk-driver）、tauri-driver（cargo install
#   tauri-driver）、Node 22。ワーカー（genzo-worker）はアプリと同じフォルダに置く。
# Windows・macOS の実機の確認の代わりにはならない（WebView が違う。docs/m5_acceptance.md）。
set -euo pipefail

APP="$(realpath "$1")"
SCENARIO="$(realpath "$2")"
OUT="$(realpath -m "${3:-e2e/out}")"
mkdir -p "$OUT"

WORK="$(mktemp -d)"
export HOME="$WORK/home"
mkdir -p "$HOME/Pictures" "$HOME/.config"
export XDG_CONFIG_HOME="$HOME/.config"
export XDG_DATA_HOME="$HOME/.local/share"
export XDG_PICTURES_DIR="$HOME/Pictures"
printf 'XDG_PICTURES_DIR="$HOME/Pictures"\n' > "$HOME/.config/user-dirs.dirs"
export GENZO_LOG="${GENZO_LOG:-info}"
# llvmpipe（ソフトウェアの GPU）は既定で使わない（決定 No.20）。CPU 版で処理する。

DISPLAY_NUM=":97"
Xvfb "$DISPLAY_NUM" -screen 0 1600x1000x24 >"$OUT/xvfb.log" 2>&1 &
XVFB_PID=$!
export DISPLAY="$DISPLAY_NUM"

tauri-driver --native-driver /usr/bin/WebKitWebDriver --port 4444 >"$OUT/tauri-driver.log" 2>&1 &
DRIVER_PID=$!

cleanup() {
  kill "$DRIVER_PID" 2>/dev/null || true
  kill "$XVFB_PID" 2>/dev/null || true
  # アプリは tauri-driver がセッションの終わりに終わらせる。残っていれば名前で終わらせる
  # （-f はこのスクリプト自身の引数にも一致するので使わない）。
  pkill -x "$(basename "$APP" | cut -c1-15)" 2>/dev/null || true
}
trap cleanup EXIT

# tauri-driver の起動を待つ。
for _ in $(seq 1 50); do
  if curl -s http://127.0.0.1:4444/status >/dev/null 2>&1; then break; fi
  sleep 0.2
done

E2E_APP="$APP" E2E_OUT="$OUT" E2E_WORK="$WORK" node "$SCENARIO"
