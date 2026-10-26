// onnxruntime.mjs のテスト（node --test）。配布物の取得は差し替え、展開は tar（Linux・macOS・Windows の tar）で行う。

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import {
  ONNXRUNTIME_ARCHIVES,
  ONNXRUNTIME_VERSION,
  archiveTopDir,
  fileSha256,
  onnxruntimeArchive,
  onnxruntimeArchiveUrl,
  onnxruntimeResources,
  prepareOnnxRuntime,
} from "./onnxruntime.mjs";

test("公式の配布物の表: 版・名前・SHA-256・置く名前", () => {
  assert.equal(onnxruntimeArchive("linux"), null);
  assert.equal(onnxruntimeArchive("other"), null);
  for (const key of ["windows", "macos"]) {
    const a = ONNXRUNTIME_ARCHIVES[key];
    assert.match(a.sha256, /^[0-9a-f]{64}$/, key);
    assert.ok(a.name.includes(ONNXRUNTIME_VERSION), key);
  }
  assert.equal(ONNXRUNTIME_ARCHIVES.windows.fileName, "onnxruntime.dll");
  assert.equal(ONNXRUNTIME_ARCHIVES.macos.fileName, "libonnxruntime.dylib");
  assert.equal(
    onnxruntimeArchiveUrl("onnxruntime-win-x64-1.31.0.zip"),
    "https://github.com/microsoft/onnxruntime/releases/download/v1.31.0/onnxruntime-win-x64-1.31.0.zip",
  );
  assert.equal(archiveTopDir("onnxruntime-osx-arm64-1.31.0.tgz"), "onnxruntime-osx-arm64-1.31.0");
  assert.equal(archiveTopDir("onnxruntime-win-x64-1.31.0.zip"), "onnxruntime-win-x64-1.31.0");
});

test("同梱する場所: ライブラリは同梱のモデルのフォルダ、ライセンスは licenses/third-party/onnxruntime", () => {
  assert.deepEqual(onnxruntimeResources("windows"), {
    "binaries/onnxruntime/onnxruntime.dll": "models/onnxruntime.dll",
    "binaries/onnxruntime/LICENSE.txt": "licenses/third-party/onnxruntime/LICENSE.txt",
    "binaries/onnxruntime/ThirdPartyNotices.txt": "licenses/third-party/onnxruntime/ThirdPartyNotices.txt",
  });
  assert.equal(onnxruntimeResources("macos")["binaries/onnxruntime/libonnxruntime.dylib"], "models/libonnxruntime.dylib");
  assert.deepEqual(onnxruntimeResources("linux"), {});
});

/** 配布物に似せた tgz を作る（最上位のフォルダ・lib/ の dylib・LICENSE・ThirdPartyNotices.txt）。 */
function makeArchive(dir, name) {
  const top = path.join(dir, "src", archiveTopDir(name));
  fs.mkdirSync(path.join(top, "lib"), { recursive: true });
  fs.writeFileSync(path.join(top, "lib", "libonnxruntime.9.9.9.dylib"), "fake dylib");
  fs.writeFileSync(path.join(top, "LICENSE"), "MIT License");
  fs.writeFileSync(path.join(top, "ThirdPartyNotices.txt"), "notices");
  const file = path.join(dir, name);
  const r = spawnSync("tar", ["-czf", file, "-C", path.join(dir, "src"), archiveTopDir(name)]);
  assert.equal(r.status, 0, String(r.stderr));
  return file;
}

test("取って SHA-256 を確かめ、ライブラリとライセンスの文書を置く。SHA-256 が違えば止める", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "genzo-ort-"));
  try {
    const name = "onnxruntime-osx-arm64-9.9.9.tgz";
    const source = makeArchive(dir, name);
    const appDir = path.join(dir, "app");
    const archives = {
      ...ONNXRUNTIME_ARCHIVES,
      macos: { name, sha256: fileSha256(source), library: "lib/libonnxruntime.9.9.9.dylib", fileName: "libonnxruntime.dylib" },
    };
    let downloads = 0;
    const io = {
      appDir,
      archives,
      run: (cmd, args) => {
        const r = spawnSync(cmd, args);
        if (r.status !== 0) throw new Error(`${cmd} が失敗した: ${r.stderr}`);
      },
      download: (url, file) => {
        downloads += 1;
        assert.ok(url.endsWith(`/${name}`), url);
        fs.copyFileSync(source, file);
      },
      readMinos: () => "14.0",
    };
    const r = prepareOnnxRuntime("macos", io);
    assert.ok(r);
    assert.equal(r.minos, "14.0");
    assert.equal(r.fileName, "libonnxruntime.dylib");
    assert.equal(r.resources["binaries/onnxruntime/libonnxruntime.dylib"], "models/libonnxruntime.dylib");
    assert.match(r.notes[0], /^# ONNX Runtime /);
    const out = path.join(appDir, "binaries", "onnxruntime");
    assert.equal(fs.readFileSync(path.join(out, "libonnxruntime.dylib"), "utf8"), "fake dylib");
    assert.equal(fs.readFileSync(path.join(out, "LICENSE.txt"), "utf8"), "MIT License");
    assert.equal(fs.readFileSync(path.join(out, "ThirdPartyNotices.txt"), "utf8"), "notices");
    assert.equal(downloads, 1);
    // 2 度目は、SHA-256 が合えば取り直さない。
    prepareOnnxRuntime("macos", io);
    assert.equal(downloads, 1);
    // 中身が違えば止める（取り直しても違う）。
    fs.writeFileSync(path.join(appDir, "binaries", "downloads", name), "changed");
    const bad = { ...io, download: (_url, file) => fs.writeFileSync(file, "tampered") };
    assert.throws(() => prepareOnnxRuntime("macos", bad), /SHA-256 が違う/);
    // Linux では何もしない。
    assert.equal(prepareOnnxRuntime("linux", io), null);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
