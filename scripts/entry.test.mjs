// 入口のスクリプト（scripts/*.mjs）のテスト（node --test）。
//
// 入口は「直接実行されたか」を判定せずに必ず処理を実行する（scripts/lib/proc.mjs の runMain）。以前は判定が
// シンボリックリンクを含むパスで成り立たず、何もせずに成功で終わっていたので、その形で起動して確かめる。

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const SCRIPTS_DIR = path.dirname(fileURLToPath(import.meta.url));

test("シンボリックリンクを含むパスで起動しても、入口のスクリプトは処理を実行する", (t) => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "genzo-scripts-"));
  t.after(() => fs.rmSync(tmp, { recursive: true, force: true }));
  const link = path.join(tmp, "scripts-link");
  fs.symlinkSync(SCRIPTS_DIR, link, process.platform === "win32" ? "junction" : "dir");

  const r = spawnSync(process.execPath, [path.join(link, "tauri.mjs"), "--version"], {
    encoding: "utf8",
    timeout: 60_000,
  });
  const out = `${r.stdout}${r.stderr}`;
  // Tauri の CLI があれば起動の記録（"$ tauri --version"）が、なければ（ui/ で npm ci をしていない）エラーが出る。
  // どちらでもよいが、何も出さずに終わってはならない。
  assert.match(out, /\$ tauri --version|Tauri の CLI がない/);
  if (/Tauri の CLI がない/.test(out)) assert.equal(r.status, 1);
});
