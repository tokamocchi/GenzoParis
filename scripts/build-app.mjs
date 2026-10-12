#!/usr/bin/env node
// 配布物（インストーラー）を作る。
//
//   node scripts/build-app.mjs [--skip-worker] [Tauri の build の引数...]
//   例: node scripts/build-app.mjs --bundles nsis          （Windows）
//       node scripts/build-app.mjs --bundles app,dmg       （macOS）
//       node scripts/build-app.mjs --bundles deb           （Linux。確認用）
//   （ui/ から: npm run app:build -- --bundles nsis）
//
// ワーカーのビルドと LibRaw のライブラリの収集（scripts/prepare-worker.mjs と同じ。`--skip-worker` なら省く）の
// 後、`tauri build --config tauri.bundle.conf.json --config binaries/tauri.libs.conf.json <引数> -- --locked` を
// ui/ で実行する。処理の本体は scripts/lib/build.mjs。
//
// 成果物: target/release/bundle/<種類>/ の下（Windows は nsis/GenzoParis_<版>_x64-setup.exe など）。

import { buildApp } from "./lib/build.mjs";
import { runMain } from "./lib/proc.mjs";

runMain(() => buildApp(process.argv.slice(2)));
