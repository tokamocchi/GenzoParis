// worker.mjs のテスト（node --test）。Windows の Visual Studio の再頒布用のフォルダの探し方を、偽のフォルダで確かめる。

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import { collectLibrawRecipe, findVcRedistDirs, readLibrawVersion } from "./worker.mjs";

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

test("LibRaw の版のヘッダを、build.rs と同じ include の置き方で探す（Windows の LIBRAW_INCLUDE_DIR）", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "genzo-libraw-"));
  try {
    const header = "#define LIBRAW_MAJOR_VERSION 0\n#define LIBRAW_MINOR_VERSION 22\n#define LIBRAW_PATCH_VERSION 1\n";
    // libraw/libraw.h のあるフォルダ（vcpkg の include）。
    fs.mkdirSync(path.join(root, "a", "libraw"), { recursive: true });
    fs.writeFileSync(path.join(root, "a", "libraw", "libraw_version.h"), header);
    assert.equal(readLibrawVersion("windows", { LIBRAW_INCLUDE_DIR: path.join(root, "a") }), "0.22.1");
    // libraw.h を直接含むフォルダ。
    fs.mkdirSync(path.join(root, "b"));
    fs.writeFileSync(path.join(root, "b", "libraw_version.h"), header);
    assert.equal(readLibrawVersion("windows", { LIBRAW_INCLUDE_DIR: path.join(root, "b") }), "0.22.1");
    assert.throws(() => readLibrawVersion("windows", { LIBRAW_INCLUDE_DIR: path.join(root, "none") }), /LibRaw の版が読めない/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("LibRaw をビルドした手順: vcpkg の port の写しと版（git のないところではコミットなし）", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "genzo-recipe-"));
  try {
    const vcpkg = path.join(root, "vcpkg");
    const port = path.join(vcpkg, "ports", "libraw");
    fs.mkdirSync(path.join(port, "sub"), { recursive: true });
    fs.writeFileSync(path.join(port, "vcpkg.json"), JSON.stringify({ name: "libraw", version: "0.22.0", "port-version": 1 }));
    fs.writeFileSync(path.join(port, "portfile.cmake"), "# portfile");
    fs.writeFileSync(path.join(port, "sub", "fix.patch"), "--- a\n+++ b\n");
    const out = path.join(root, "out");
    fs.mkdirSync(out);
    const r = collectLibrawRecipe("windows", out, vcpkg, { PATH: process.env.PATH, GIT_CEILING_DIRECTORIES: root });
    assert.equal(r.recipe.kind, "vcpkg");
    assert.equal(r.recipe.version, "0.22.0#1");
    assert.equal(r.recipe.copied, true);
    assert.deepEqual(r.files, ["vcpkg-ports-libraw/portfile.cmake", "vcpkg-ports-libraw/sub/fix.patch", "vcpkg-ports-libraw/vcpkg.json"]);
    assert.equal(fs.readFileSync(path.join(out, "vcpkg-ports-libraw", "sub", "fix.patch"), "utf8"), "--- a\n+++ b\n");
    // vcpkg を使っていない（LIBRAW_INCLUDE_DIR などを自分で指定した）なら、手順は分からない。
    assert.deepEqual(collectLibrawRecipe("windows", out, null, {}), { recipe: { kind: "unknown" }, files: [] });
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
