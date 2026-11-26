// onnxruntime.mjs のテスト（node --test）。配布物の取得は差し替え、展開は tar（Linux・macOS・Windows の tar）で行う。

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import {
  ONNXRUNTIME_ARCHIVES,
  ONNXRUNTIME_PACKAGE,
  ONNXRUNTIME_VERSION,
  fileSha256,
  onnxruntimeArchive,
  onnxruntimeArchiveUrl,
  onnxruntimeResources,
  prepareOnnxRuntime,
} from "./onnxruntime.mjs";

test("公式の配布物の表: 版・名前・SHA-256・置く名前", () => {
  assert.equal(onnxruntimeArchive("linux"), null);
  assert.equal(onnxruntimeArchive("other"), null);
  assert.match(ONNXRUNTIME_PACKAGE.sha256, /^[0-9a-f]{64}$/);
  assert.ok(ONNXRUNTIME_PACKAGE.name.includes(ONNXRUNTIME_VERSION));
  for (const key of ["windows", "macos"]) {
    const a = ONNXRUNTIME_ARCHIVES[key];
    // 両 OS とも、同じパッケージから取り出す。
    assert.equal(a.name, ONNXRUNTIME_PACKAGE.name, key);
    assert.equal(a.sha256, ONNXRUNTIME_PACKAGE.sha256, key);
    assert.match(a.librarySha256, /^[0-9a-f]{64}$/, key);
    assert.match(a.library, /^runtimes\/[a-z0-9-]+\/native\//, key);
  }
  assert.notEqual(ONNXRUNTIME_ARCHIVES.windows.librarySha256, ONNXRUNTIME_ARCHIVES.macos.librarySha256);
  assert.equal(ONNXRUNTIME_ARCHIVES.windows.fileName, "onnxruntime.dll");
  assert.equal(ONNXRUNTIME_ARCHIVES.macos.fileName, "libonnxruntime.dylib");
  assert.equal(
    onnxruntimeArchiveUrl("microsoft.ml.onnxruntime.1.31.0.nupkg"),
    "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/1.31.0/microsoft.ml.onnxruntime.1.31.0.nupkg",
  );
  // URL は小文字（nuget.org の flat container の決まり）。
  const url = onnxruntimeArchiveUrl(ONNXRUNTIME_PACKAGE.name);
  assert.equal(url, url.toLowerCase());
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

const LIBRARY = "runtimes/osx-arm64/native/libonnxruntime.dylib";

/**
 * パッケージに似せた書庫を作る（最上位に LICENSE・ThirdPartyNotices.txt、runtimes/ にライブラリ。ほかの OS のものも入れる）。
 * 本物は zip だが、ここでは tar で作る（Linux の tar は zip を展開できない。取り出す処理は tar に任せていて、形式を見ない）。
 */
function makePackage(dir, name) {
  const src = path.join(dir, "src");
  fs.mkdirSync(path.join(src, "runtimes", "osx-arm64", "native"), { recursive: true });
  fs.mkdirSync(path.join(src, "runtimes", "win-x64", "native"), { recursive: true });
  fs.writeFileSync(path.join(src, ...LIBRARY.split("/")), "fake dylib");
  fs.writeFileSync(path.join(src, "runtimes", "win-x64", "native", "onnxruntime.dll"), "fake dll");
  fs.writeFileSync(path.join(src, "LICENSE"), "MIT License");
  fs.writeFileSync(path.join(src, "ThirdPartyNotices.txt"), "notices");
  fs.writeFileSync(path.join(src, "Privacy.md"), "privacy");
  const file = path.join(dir, name);
  // 最上位のフォルダを作らない（本物と同じ）ように、中身の名前を並べて作る。
  const r = spawnSync("tar", ["-czf", name, "-C", "src", "runtimes", "LICENSE", "ThirdPartyNotices.txt", "Privacy.md"], {
    cwd: dir,
  });
  assert.equal(r.status, 0, String(r.stderr));
  return file;
}

test("取って SHA-256 を確かめ、ライブラリとライセンスの文書を置く。SHA-256 が違えば止める", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "genzo-ort-"));
  try {
    const name = "microsoft.ml.onnxruntime.9.9.9.nupkg";
    const source = makePackage(dir, name);
    const appDir = path.join(dir, "app");
    const librarySha256 = fileSha256(path.join(dir, "src", ...LIBRARY.split("/")));
    const archives = {
      ...ONNXRUNTIME_ARCHIVES,
      macos: { name, sha256: fileSha256(source), library: LIBRARY, librarySha256, fileName: "libonnxruntime.dylib" },
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
        assert.ok(url.startsWith("https://api.nuget.org/v3-flatcontainer/"), url);
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
    // 同梱の一覧の注記に、置いたライブラリとパッケージの SHA-256、パッケージの中の場所が載る。
    assert.ok(r.notes[0].includes(librarySha256), r.notes[0]);
    assert.ok(r.notes[0].includes(archives.macos.sha256), r.notes[0]);
    assert.ok(r.notes[0].includes(LIBRARY), r.notes[0]);
    const out = path.join(appDir, "binaries", "onnxruntime");
    assert.equal(fs.readFileSync(path.join(out, "libonnxruntime.dylib"), "utf8"), "fake dylib");
    assert.equal(fs.readFileSync(path.join(out, "LICENSE.txt"), "utf8"), "MIT License");
    assert.equal(fs.readFileSync(path.join(out, "ThirdPartyNotices.txt"), "utf8"), "notices");
    // 置くのは使うものだけ（ほかの OS のライブラリなどは置かない）。
    assert.deepEqual(fs.readdirSync(out).sort(), ["LICENSE.txt", "ThirdPartyNotices.txt", "libonnxruntime.dylib"]);
    assert.equal(fs.existsSync(path.join(appDir, "binaries", "downloads", "extract")), false);
    assert.equal(downloads, 1);
    // 2 度目は、SHA-256 が合えば取り直さない。
    prepareOnnxRuntime("macos", io);
    assert.equal(downloads, 1);
    // パッケージは合っていても、取り出したライブラリの SHA-256 が違えば止める。
    const wrongLibrary = { ...io, archives: { ...archives, macos: { ...archives.macos, librarySha256: "0".repeat(64) } } };
    assert.throws(() => prepareOnnxRuntime("macos", wrongLibrary), /libonnxruntime\.dylib の SHA-256 が違う/);
    assert.equal(fs.existsSync(path.join(appDir, "binaries", "downloads", "extract")), false);
    // パッケージの中身が違えば止める（取り直しても違う）。
    fs.writeFileSync(path.join(appDir, "binaries", "downloads", name), "changed");
    const bad = { ...io, download: (_url, file) => fs.writeFileSync(file, "tampered") };
    assert.throws(() => prepareOnnxRuntime("macos", bad), /SHA-256 が違う/);
    // Linux では何もしない。
    assert.equal(prepareOnnxRuntime("linux", io), null);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
