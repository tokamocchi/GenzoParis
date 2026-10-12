#!/usr/bin/env node
// Tauri の CLI（ui の devDependencies の @tauri-apps/cli）を、この構成（画面は ui/、Tauri のアプリは
// crates/genzo-app/）で起動する。
//
//   node scripts/tauri.mjs <Tauri の CLI の引数>      例: node scripts/tauri.mjs dev
//   （ui/ から: npm run app:dev）
//
// 環境変数 TAURI_APP_PATH に crates/genzo-app を入れ、今のフォルダを ui/ にして起動する（理由は
// scripts/lib/tauri-cli.mjs）。

import { runMain } from "./lib/proc.mjs";
import { runTauri } from "./lib/tauri-cli.mjs";

runMain(() => runTauri(process.argv.slice(2)));
