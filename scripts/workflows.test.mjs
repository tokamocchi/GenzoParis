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

test("CI・配布で入れる cargo-about の版は、第三者のライセンスの文書を作る版（CARGO_ABOUT_VERSION）と同じ", async () => {
  const { CARGO_ABOUT_VERSION } = await import("./lib/third-party.mjs");
  for (const file of ["ci.yml", "release.yml"]) {
    const text = fs.readFileSync(path.join(WORKFLOWS_DIR, file), "utf8");
    const m = /^\s*CARGO_ABOUT_VERSION:\s*"([^"]+)"/m.exec(text);
    assert.ok(m, `${file} に CARGO_ABOUT_VERSION がない`);
    assert.equal(m[1], CARGO_ABOUT_VERSION, file);
    assert.match(text, /node scripts\/third-party\.mjs --check --only rust/, file);
  }
  const ci = fs.readFileSync(path.join(WORKFLOWS_DIR, "ci.yml"), "utf8");
  assert.match(ci, /node scripts\/third-party\.mjs --check --only npm --vite/);
  // 配布物のジョブは、Rust の crate のライセンスの文書の確認が通ってから動く。
  const release = fs.readFileSync(path.join(WORKFLOWS_DIR, "release.yml"), "utf8");
  assert.match(release, /^  bundle:\n    needs: licenses\n/m);
});

test("配布物の確認で、同梱した C / C++ のライブラリのライセンスの文書と LibRaw のソースがあるかを見る（Windows・macOS）", async () => {
  const { NATIVE_NOTICES_TARGET_PATH } = await import("./lib/native-notice.mjs");
  assert.equal(NATIVE_NOTICES_TARGET_PATH, "licenses/third-party/native.txt");
  const release = fs.readFileSync(path.join(WORKFLOWS_DIR, "release.yml"), "utf8");
  assert.match(release, /'licenses\\third-party\\native\.txt'/);
  assert.match(release, /licenses\/third-party\/native\.txt"/);
  assert.match(release, /'licenses\\LibRaw\\source'/);
  assert.match(release, /'licenses\\third-party\\vcruntime-terms\.txt'/);
  assert.match(release, /licenses\/LibRaw\/source\/\*\.tar\.gz/);
});

test("配布物の確認で、AI マスクの同梱のモデル・ONNX Runtime・そのライセンスの文書があるかを見る（Windows・macOS）", async () => {
  const { ONNXRUNTIME_ARCHIVES } = await import("./lib/onnxruntime.mjs");
  const release = fs.readFileSync(path.join(WORKFLOWS_DIR, "release.yml"), "utf8");
  const tauri = JSON.parse(fs.readFileSync(path.join(WORKFLOWS_DIR, "..", "..", "crates", "genzo-app", "tauri.conf.json"), "utf8"));
  // tauri.conf.json の resources で同梱するモデルと台帳は、両 OS の確認に載っている。
  const models = Object.values(tauri.bundle.resources)
    .filter((v) => v.startsWith("models/"))
    .map((v) => v.slice("models/".length));
  assert.ok(models.length >= 4, models.join(", "));
  for (const name of [...models, ONNXRUNTIME_ARCHIVES.windows.fileName]) assert.ok(release.includes(`'${name}'`), name);
  for (const name of [...models, ONNXRUNTIME_ARCHIVES.macos.fileName]) {
    assert.match(release, new RegExp(`for name in [^\\n]*\\b${name.replace(/[.]/g, "\\.")}\\b`), name);
  }
  assert.match(release, /third-party\/ai-models\.txt/);
  assert.match(release, /'third-party\\ai-models\.txt'/);
  assert.equal((release.match(/scripts\/check-onnxruntime\.py/g) ?? []).length >= 2, true);
});
