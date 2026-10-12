// worker.mjs のテスト（node --test）。Windows の Visual Studio の再頒布用のフォルダの探し方を、偽のフォルダで確かめる。

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import { findVcRedistDirs } from "./worker.mjs";

test("Visual C++ のランタイムを探すフォルダ（環境変数の指定と VCToolsRedistDir）", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "genzo-vcredist-"));
  try {
    // 環境変数 GENZO_VC_REDIST_DIR はそのまま使う（複数は path.delimiter で区切る）。
    assert.deepEqual(findVcRedistDirs({ GENZO_VC_REDIST_DIR: ["a", "b"].join(path.delimiter) }, "x86_64-pc-windows-msvc"), [
      "a",
      "b",
    ]);

    // VCToolsRedistDir の <アーキテクチャ> の下の Microsoft.VC*.* を使う（デバッグ版・ほかのアーキテクチャは除く）。
    const version = path.join(root, "VC", "Redist", "MSVC", "14.44.35112");
    for (const d of ["x64/Microsoft.VC143.CRT", "x64/Microsoft.VC143.OpenMP", "x64/Microsoft.VC143.DebugCRT", "arm64/Microsoft.VC143.CRT"]) {
      fs.mkdirSync(path.join(version, d), { recursive: true });
    }
    fs.mkdirSync(path.join(version, "x64", "onecore"), { recursive: true });
    const dirs = findVcRedistDirs({ VCToolsRedistDir: version }, "x86_64-pc-windows-msvc").sort();
    assert.deepEqual(dirs, [path.join(version, "x64", "Microsoft.VC143.CRT"), path.join(version, "x64", "Microsoft.VC143.OpenMP")]);
    assert.deepEqual(findVcRedistDirs({ VCToolsRedistDir: version }, "aarch64-pc-windows-msvc"), [
      path.join(version, "arm64", "Microsoft.VC143.CRT"),
    ]);

    // どこにもなければ（vswhere もなければ）止まる。
    assert.throws(
      () => findVcRedistDirs({ VCToolsRedistDir: path.join(root, "none"), "ProgramFiles(x86)": root }, "x86_64-pc-windows-msvc"),
      /vswhere が見つからない/,
    );
    assert.throws(() => findVcRedistDirs({}, "riscv64gc-unknown-linux-gnu"), /アーキテクチャが分からない/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
