#!/usr/bin/env node
// 配布物（インストーラー）を作る。
//
//   node scripts/build-app.mjs [--skip-worker] [Tauri の build の引数...]
//   例: node scripts/build-app.mjs --bundles nsis          （Windows）
//       node scripts/build-app.mjs --bundles app,dmg       （macOS）
//       node scripts/build-app.mjs --bundles deb           （Linux。確認用）
//   （ui/ から: npm run app:build -- --bundles nsis）
//
// 1. ワーカーをビルドし、LibRaw のライブラリを集める（scripts/prepare-worker.mjs）。`--skip-worker` なら
//    省く（前に作った crates/genzo-app/binaries を使う）。
// 2. `tauri build --config tauri.bundle.conf.json --config binaries/tauri.libs.conf.json <引数> -- --locked` を
//    ui/ で実行する（scripts/tauri.mjs。画面のビルドは tauri.conf.json の beforeBuildCommand（npm run build）が
//    行う）。本体 genzo-paris は既定の機能フラグ（LibRaw なし）と custom-protocol（Tauri の CLI が付ける）で
//    ビルドされる。`--` の後の引数は cargo に渡る（`--locked` は常に付ける）。
//
// 成果物: target/release/bundle/<種類>/ の下（Windows は nsis/GenzoParis_<版>_x64-setup.exe など）。

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { prepareWorker } from "./prepare-worker.mjs";
import { BUNDLE_CONFIG, LIBS_CONFIG, REPO_ROOT } from "./lib/proc.mjs";
import { runTauri } from "./tauri.mjs";

export function buildApp(argv) {
  const { skipWorker, tauriArgs } = splitBuildArgs(argv);
  if (skipWorker) {
    if (!fs.existsSync(LIBS_CONFIG)) {
      throw new Error(`${path.relative(REPO_ROOT, LIBS_CONFIG)} がない（--skip-worker を付けずに実行する）`);
    }
    console.log("ワーカーのビルドを省く（前に作ったものを使う）");
  } else {
    prepareWorker();
  }
  runTauri(["build", "--config", BUNDLE_CONFIG, "--config", LIBS_CONFIG, ...tauriArgs]);
}

/**
 * 引数を、このスクリプトの引数（--skip-worker）と Tauri の build の引数に分ける。
 *
 * cargo には常に `--locked` を渡す（Cargo.lock を変えずにビルドする。ワーカーのビルド・CI と同じ）。`--` の
 * 後の引数は Tauri の CLI が cargo に渡す。
 *
 * @param {string[]} argv
 * @returns {{ skipWorker: boolean, tauriArgs: string[] }}
 */
export function splitBuildArgs(argv) {
  const sep = argv.indexOf("--");
  const own = sep === -1 ? argv : argv.slice(0, sep);
  const runner = sep === -1 ? [] : argv.slice(sep + 1);
  if (!runner.includes("--locked")) runner.unshift("--locked");
  return {
    skipWorker: own.includes("--skip-worker"),
    tauriArgs: [...own.filter((a) => a !== "--skip-worker"), "--", ...runner],
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    buildApp(process.argv.slice(2));
  } catch (e) {
    console.error(`エラー: ${e instanceof Error ? e.message : e}`);
    process.exit(1);
  }
}
