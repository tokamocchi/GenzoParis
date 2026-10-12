// GitHub Actions のワークフロー（.github/workflows/*.yml）のテスト（node --test）。
//
// シェルの変数のすぐ後に ASCII でない文字（全角の括弧など）を書くと、macOS のランナーの bash は、その文字の
// 先頭のバイトまで変数名として読むことがあり、変数が空になって残りのバイトが文字化けする（2026-10-12 の
// release.yml の実行で、「LSMinimumSystemVersion: $min_actual（期待: …）」の行に版が出なかった。1 バイトの文字を
// Latin-1 として分類するロケールの bash で、同じ出力になることを確かめた）。`${名前}` で囲む。

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const WORKFLOWS_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", ".github", "workflows");

// `$名前` のすぐ後が ASCII でない文字のもの（`${名前}`・`$(…)`・`${{ … }}` は当たらない）。
const BARE_VARIABLE_BEFORE_NON_ASCII = /\$[A-Za-z_][A-Za-z0-9_]*(?=[^\x00-\x7F])/g;

test("ワークフローのシェルの変数は、すぐ後に ASCII でない文字を書くときに ${名前} で囲んである", () => {
  const files = fs.readdirSync(WORKFLOWS_DIR).filter((f) => /\.ya?ml$/.test(f));
  assert.ok(files.length > 0, ".github/workflows にファイルがない");
  const found = [];
  for (const file of files) {
    const lines = fs.readFileSync(path.join(WORKFLOWS_DIR, file), "utf8").split("\n");
    lines.forEach((line, i) => {
      if (/^\s*#/.test(line)) return;
      for (const m of line.matchAll(BARE_VARIABLE_BEFORE_NON_ASCII)) found.push(`${file}:${i + 1}: ${m[0]}`);
    });
  }
  assert.deepEqual(found, []);
});
