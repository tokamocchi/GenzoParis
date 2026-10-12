#!/usr/bin/env node
// 配布用のワーカー（genzo-worker。LibRaw あり）を release でビルドし、Tauri が同梱する場所に置く。
//
//   node scripts/prepare-worker.mjs
//   （ui/ から: npm run app:worker）
//
// 1. `cargo build --release -p genzo-worker --features libraw --locked` でワーカーをビルドする（本体は LibRaw に
//    依存させない。決定事項ログ No.32）。
// 2. crates/genzo-app/binaries/genzo-worker-<target triple> に置く（Tauri の externalBin）。
// 3. LibRaw の共有ライブラリとその依存を crates/genzo-app/binaries/libs に集め（Windows は DLL、macOS は dylib の
//    参照を書き換えて署名し直す）、Tauri に渡す追加の設定 binaries/tauri.libs.conf.json を作る。
// 4. 同梱したファイルの一覧を binaries/bundled-libs.txt に書く。
//
// 環境変数: LIBRAW_INCLUDE_DIR・LIBRAW_LIB_DIR・LIBRAW_LIB_NAME・LIBRAW_DLL_DIR・VCPKG_ROOT（Windows）、
// GENZO_ALLOW_NEWER_MINOS=1（macOS。同梱する dylib が .app の最小の版より新しい macOS 向けでも通し、.app の
// 最小の版を上げる）。処理の本体と詳しい説明は scripts/lib/worker.mjs、手順は docs/packaging.md。

import { runMain } from "./lib/proc.mjs";
import { prepareWorker } from "./lib/worker.mjs";

runMain(() => {
  prepareWorker();
});
