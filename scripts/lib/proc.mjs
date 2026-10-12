// スクリプトで使う場所と、外部のコマンドの実行。

import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

/** リポジトリの最上位。 */
export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
/** Tauri のアプリ（tauri.conf.json のあるところ）。 */
export const APP_DIR = path.join(REPO_ROOT, "crates", "genzo-app");
/** 画面（package.json のあるところ）。 */
export const UI_DIR = path.join(REPO_ROOT, "ui");
/** 配布用に作るファイルを置くフォルダ（crates/genzo-app/.gitignore で除外）。 */
export const BINARIES_DIR = path.join(APP_DIR, "binaries");
/** 配布のビルドのときだけ足す Tauri の設定。 */
export const BUNDLE_CONFIG = path.join(APP_DIR, "tauri.bundle.conf.json");
/** prepare-worker.mjs が作る、同梱するライブラリの Tauri の設定。 */
export const LIBS_CONFIG = path.join(BINARIES_DIR, "tauri.libs.conf.json");
/** Tauri の CLI（ui の devDependencies の @tauri-apps/cli）の入口。 */
export const TAURI_CLI = path.join(UI_DIR, "node_modules", "@tauri-apps", "cli", "tauri.js");

/**
 * コマンドを実行する（出力はそのまま端末に出す）。失敗したら例外を投げる。
 *
 * Windows でも .exe のコマンド（cargo・rustc など）は shell なしで起動できる。.cmd・.bat は
 * shell なしでは起動できないため使わない（npm の CLI は node で直接起動する）。
 */
export function run(cmd, args, options = {}) {
  console.log(`$ ${[cmd, ...args].map(quoteForLog).join(" ")}`);
  const r = spawnSync(cmd, args, { stdio: "inherit", ...options });
  if (r.error) throw new Error(`${cmd} を実行できない: ${r.error.message}`);
  if (r.status !== 0) {
    throw new Error(`${cmd} が失敗した（終了コード ${r.status ?? r.signal}）`);
  }
}

/** コマンドを実行して標準出力を返す。失敗したら例外を投げる。 */
export function capture(cmd, args, options = {}) {
  const r = spawnSync(cmd, args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 64 * 1024 * 1024,
    ...options,
  });
  if (r.error) throw new Error(`${cmd} を実行できない: ${r.error.message}`);
  if (r.status !== 0) {
    throw new Error(`${cmd} ${args.join(" ")} が失敗した（終了コード ${r.status ?? r.signal}）\n${r.stderr}`);
  }
  return r.stdout;
}

function quoteForLog(s) {
  return /[\s"']/.test(s) ? JSON.stringify(s) : s;
}

/**
 * スクリプトの入口（scripts/*.mjs）で処理を実行する。失敗したらエラーを表示し、終了コード 1 で終わる。
 *
 * 入口のファイルは「直接実行されたか」を判定せずに必ずこれを呼ぶ（判定は、シンボリックリンクを含むパスで
 * 起動したときなどに成り立たず、何もせずに成功で終わってしまうため）。ほかのスクリプトから使う処理は
 * scripts/lib/ に置く。
 *
 * @param {() => void} main
 */
export function runMain(main) {
  try {
    main();
  } catch (e) {
    console.error(`エラー: ${e instanceof Error ? e.message : e}`);
    process.exit(1);
  }
}
