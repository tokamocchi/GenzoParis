#!/usr/bin/env node
// 配布物に入れる第三者のライセンスの文書（DIST-05）を作る・確かめる。
//
//   node scripts/third-party.mjs                  作り直して crates/genzo-app/licenses/third-party に書く
//   node scripts/third-party.mjs --check          作り直した内容がリポジトリの文書と同じかを確かめる（CI）
//   オプション: --only rust | --only npm（片方だけ）、--vite（npm: 画面のバンドルに入ったパッケージが
//   すべて一覧にあるかも確かめる。先に ui/ で npm run build を実行しておく）
//
// Cargo.lock・ui/package-lock.json を変えたら実行して、作り直した文書をコミットする。Rust は cargo-about
// （`cargo install cargo-about --version 0.9.2 --locked --features cli`）、npm は ui/ の `npm ci` が要る。
// 処理の本体は scripts/lib/third-party-files.mjs・scripts/lib/third-party.mjs、方式と人の確認待ちの点は
// docs/third_party.md の 1.4 節。

import { runMain } from "./lib/proc.mjs";
import { runThirdParty } from "./lib/third-party-files.mjs";

runMain(() => runThirdParty(parseArgs(process.argv.slice(2))));

/** @returns {{ check: boolean, only: "rust" | "npm" | null, vite: boolean }} */
function parseArgs(argv) {
  const options = { check: false, only: null, vite: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--check") options.check = true;
    else if (a === "--vite") options.vite = true;
    else if (a === "--only" && (argv[i + 1] === "rust" || argv[i + 1] === "npm")) options.only = argv[++i];
    else throw new Error(`分からない引数: ${a}（--check・--only rust|npm・--vite）`);
  }
  if (options.vite && options.only === "rust") throw new Error("--vite は npm の一覧を作るときだけ使える");
  return options;
}
