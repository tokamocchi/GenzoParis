// 配布物（インストーラー）を作る処理（入口は scripts/build-app.mjs）。
//
// 1. 第三者のライセンスの文書（crates/genzo-app/licenses/third-party。tauri.conf.json の resources で同梱する）が、
//    今の Cargo.lock・ui/package-lock.json・about.toml から作ったものかを確かめる（scripts/lib/third-party-files.mjs。古い文書を
//    同梱しないため。古ければ止める）。
// 2. ワーカーをビルドし、LibRaw のライブラリを集める（scripts/lib/worker.mjs）。`--skip-worker` なら
//    省く（前に作った crates/genzo-app/binaries を使う）。
// 3. `tauri build --config tauri.bundle.conf.json --config binaries/tauri.libs.conf.json <引数> -- --locked` を
//    ui/ で実行する（scripts/lib/tauri-cli.mjs。画面のビルドは tauri.conf.json の beforeBuildCommand
//    （npm run build）が行う）。本体 genzo-paris は既定の機能フラグ（LibRaw なし）と custom-protocol（Tauri の
//    CLI が付ける）でビルドされる。`--` の後の引数は cargo に渡る（`--locked` は常に付ける）。
// 4. 画面のバンドルに入った npm のパッケージ（Vite の build.license の出力）が、すべて npm の文書に載っているかを
//    確かめる（載っていなければ止める）。

import fs from "node:fs";
import path from "node:path";

import { BUNDLE_CONFIG, LIBS_CONFIG, REPO_ROOT } from "./proc.mjs";
import { runTauri } from "./tauri-cli.mjs";
import { checkNoticesForBuild, checkViteBundle, generateNpmNotices } from "./third-party-files.mjs";
import { prepareWorker } from "./worker.mjs";

/**
 * 配布物を作る。失敗したら例外を投げる。
 *
 * @param {string[]} argv scripts/build-app.mjs の引数
 */
export function buildApp(argv) {
  const { skipWorker, tauriArgs } = splitBuildArgs(argv);
  checkNoticesForBuild();
  if (skipWorker) {
    if (!fs.existsSync(LIBS_CONFIG)) {
      throw new Error(`${path.relative(REPO_ROOT, LIBS_CONFIG)} がない（--skip-worker を付けずに実行する）`);
    }
    console.log("ワーカーのビルドを省く（前に作ったものを使う）");
  } else {
    prepareWorker();
  }
  runTauri(["build", "--config", BUNDLE_CONFIG, "--config", LIBS_CONFIG, ...tauriArgs]);
  checkViteBundle(generateNpmNotices().model);
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
