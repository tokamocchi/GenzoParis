// E2E のシナリオの補助（アプリのコマンドを WebView の中から呼んで、コアの状態を確かめる）。
//
// 画面の操作の結果を、画面の表示だけでなくコアの状態（評価・現像設定・件数）でも確かめるために使う。
// アプリのコマンドは Tauri の内部の invoke（window.__TAURI_INTERNALS__.invoke）で呼ぶ。

import { writeFileSync } from "node:fs";
import { join } from "node:path";

import { sleep } from "./webdriver.mjs";

/** アプリのコマンドを呼ぶ（失敗は例外）。 */
export async function invoke(wd, command, args = {}) {
  const r = await wd.execAsync(
    `const [command, args, done] = arguments;
     window.__TAURI_INTERNALS__.invoke(command, args)
       .then((v) => done({ ok: v }))
       .catch((e) => done({ err: e }));`,
    [command, args],
  );
  if (r && "err" in r) throw new Error(`${command}: ${JSON.stringify(r.err)}`);
  return r ? r.ok : undefined;
}

/** コアのコマンド（genzo-api の Command）を呼ぶ。 */
export function core(wd, command) {
  return invoke(wd, "core_execute", { command });
}

/** 今の検索の結果をすべて読む（件数が少ないカタログ用）。 */
export async function allItems(wd) {
  const s = await core(wd, { type: "search", filter: {}, sort: { key: "file_name", direction: "ascending" } });
  const r = await core(wd, { type: "range", generation: s.generation, start: 0, len: Math.max(1, s.count) });
  return r.items;
}

/** ジョブが終わるまで待つ。 */
export async function waitJob(wd, jobId, timeoutMs = 120000) {
  const end = Date.now() + timeoutMs;
  for (;;) {
    const j = await core(wd, { type: "job", job_id: jobId });
    if (j.state !== "running") return j;
    if (Date.now() > end) throw new Error(`ジョブ ${jobId} が終わりません`);
    await sleep(300);
  }
}

/** 画面を保存する（番号付きのファイル名）。 */
let shot = 0;
export async function screenshot(wd, name) {
  shot++;
  const file = join(process.env.E2E_OUT, `${String(shot).padStart(2, "0")}-${name}.png`);
  writeFileSync(file, await wd.screenshot());
  console.log(`  画面: ${file}`);
  return file;
}

/** 手順の見出しを出し、失敗したら画面を保存して例外を投げ直す。 */
export async function step(wd, title, fn) {
  console.log(`▶ ${title}`);
  const t0 = Date.now();
  try {
    const v = await fn();
    console.log(`  ✓ ${title}（${Date.now() - t0} ms）`);
    return v;
  } catch (e) {
    console.log(`  ✗ ${title}: ${e.message}`);
    try {
      await screenshot(wd, `failed-${title.replace(/[^\p{L}\p{N}]+/gu, "_")}`);
      console.log(await wd.exec("return document.body.innerText.slice(0, 2000)"));
    } catch {
      // 画面を取れなくても、元の失敗を報告する。
    }
    throw e;
  }
}

export function assert(cond, message) {
  if (!cond) throw new Error(`確認に失敗: ${message}`);
}
