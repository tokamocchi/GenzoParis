// Tauri の CLI（ui の devDependencies の @tauri-apps/cli）を、この構成（画面は ui/、Tauri のアプリは
// crates/genzo-app/）で起動する処理（入口は scripts/tauri.mjs。scripts/lib/build.mjs からも使う）。
//
// Tauri の CLI は、tauri.conf.json を「今のフォルダとその下」から探し、package.json を「今のフォルダ」から
// 探す。ui/ で起動すると tauri.conf.json が見つからない（ui/ の下にない）ため、環境変数 TAURI_APP_PATH で
// crates/genzo-app を指定し、今のフォルダを ui/ にして起動する（beforeDevCommand・beforeBuildCommand の
// `npm run dev`・`npm run build` は ui/ で実行される）。TAURI_APP_PATH は、Tauri のアプリのフォルダが画面の
// フォルダの下にない構成のための環境変数（@tauri-apps/cli の CHANGELOG の 2.0 系、#11258）。
// @tauri-apps/cli 2.11.5 で、ui/ からは見つからず、TAURI_APP_PATH を付けると見つかることを確かめた（`tauri info`）。

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { APP_DIR, TAURI_CLI, UI_DIR } from "./proc.mjs";

/**
 * Tauri の CLI を実行する。失敗したら例外を投げる。
 *
 * @param {string[]} args CLI の引数
 */
export function runTauri(args) {
  if (!fs.existsSync(TAURI_CLI)) {
    throw new Error(`Tauri の CLI がない: ${TAURI_CLI}（先に ui/ で npm ci を実行する）`);
  }
  console.log(`$ tauri ${args.join(" ")}（TAURI_APP_PATH=${path.relative(UI_DIR, APP_DIR)}、ui/ で実行）`);
  const r = spawnSync(process.execPath, [TAURI_CLI, ...args], {
    cwd: UI_DIR,
    stdio: "inherit",
    env: { ...process.env, TAURI_APP_PATH: APP_DIR },
  });
  if (r.error) throw new Error(`Tauri の CLI を実行できない: ${r.error.message}`);
  if (r.status !== 0) throw new Error(`Tauri の CLI が失敗した（終了コード ${r.status ?? r.signal}）`);
}
