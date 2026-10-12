// bundle.mjs・target.mjs のテスト（node --test）。

import assert from "node:assert/strict";
import { test } from "node:test";

import { splitBuildArgs } from "./build.mjs";
import {
  chooseLibrawLibName,
  isVcRedistFolder,
  libsTauriConfig,
  pickVcRedistVersion,
  planWindowsDlls,
  vcRedistArch,
} from "./bundle.mjs";
import { parseHostTriple, sidecarFileName, targetOs } from "./target.mjs";

test("build-app の引数を分け、cargo に --locked を 1 回だけ渡す", () => {
  assert.deepEqual(splitBuildArgs(["--skip-worker", "--bundles", "nsis"]), {
    skipWorker: true,
    tauriArgs: ["--bundles", "nsis", "--", "--locked"],
  });
  assert.deepEqual(splitBuildArgs(["--bundles", "deb", "--", "--locked", "-v"]), {
    skipWorker: false,
    tauriArgs: ["--bundles", "deb", "--", "--locked", "-v"],
  });
  assert.deepEqual(splitBuildArgs(["--", "--offline"]), {
    skipWorker: false,
    tauriArgs: ["--", "--locked", "--offline"],
  });
});

test("Windows の LibRaw の .lib の名前を選ぶ（スレッドセーフな版を先に）", () => {
  assert.equal(chooseLibrawLibName(["raw.lib", "raw_r.lib", "lcms2.lib"]), "raw_r");
  assert.equal(chooseLibrawLibName(["RAW_R.LIB"]), "RAW_R");
  assert.equal(chooseLibrawLibName(["libraw.lib", "libraw_static.lib"]), "libraw");
  assert.equal(chooseLibrawLibName(["lcms2.lib", "pkgconfig"]), null);
});

test("ワーカーの DLL をたどり、LibRaw のフォルダにあるものと Visual C++ のランタイムを同梱する", () => {
  const dir = "C:/vcpkg/installed/x64-windows/bin";
  const files = {
    "raw_r.dll": { imports: ["lcms2.dll", "jasper.dll", "zlib1.dll", "MSVCP140.dll", "KERNEL32.dll"], delayImports: [] },
    "lcms2.dll": { imports: ["KERNEL32.dll", "VCRUNTIME140.dll"], delayImports: [] },
    "jasper.dll": { imports: ["jpeg62.dll", "zlib1.dll"], delayImports: ["WINHTTP.dll"] },
    "jpeg62.dll": { imports: ["KERNEL32.dll", "api-ms-win-crt-heap-l1-1-0.dll"], delayImports: [] },
    // 循環しても止まる。
    "zlib1.dll": { imports: ["raw_r.dll"], delayImports: [] },
  };
  // Visual Studio の再頒布用のフォルダ。msvcp140.dll は vcruntime140_1.dll にも依存する（たどって同梱する）。
  const crt = "C:/VS/VC/Redist/MSVC/14.44.35112/x64/Microsoft.VC143.CRT";
  const vc = {
    "vcruntime140.dll": { imports: ["KERNEL32.dll", "api-ms-win-crt-runtime-l1-1-0.dll"], delayImports: [] },
    "msvcp140.dll": { imports: ["VCRUNTIME140.dll", "VCRUNTIME140_1.dll", "KERNEL32.dll"], delayImports: [] },
    "vcruntime140_1.dll": { imports: ["VCRUNTIME140.dll", "KERNEL32.dll"], delayImports: [] },
  };
  const plan = planWindowsDlls(
    { imports: ["KERNEL32.dll", "RAW_R.dll", "VCRUNTIME140.dll", "api-ms-win-crt-runtime-l1-1-0.dll"], delayImports: [] },
    {
      findDll: (name) => (files[name.toLowerCase()] ? `${dir}/${name.toLowerCase()}` : null),
      findVcRuntimeDll: (name) => (vc[name.toLowerCase()] ? `${crt}/${name.toLowerCase()}` : null),
      readImports: (file) =>
        file.startsWith(crt) ? vc[file.slice(crt.length + 1)] : files[file.slice(dir.length + 1)],
    },
  );
  assert.deepEqual(
    plan.bundled.map((b) => b.name),
    ["RAW_R.dll", "lcms2.dll", "jasper.dll", "zlib1.dll", "jpeg62.dll"],
  );
  assert.equal(plan.bundled[0].source, `${dir}/raw_r.dll`);
  assert.deepEqual(
    plan.vcRuntime.map((v) => v.name),
    ["VCRUNTIME140.dll", "MSVCP140.dll", "VCRUNTIME140_1.dll"],
  );
  assert.equal(plan.vcRuntime[1].source, `${crt}/msvcp140.dll`);
  assert.deepEqual(plan.vcRuntimeMissing, []);
  assert.deepEqual(plan.system, ["KERNEL32.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "api-ms-win-crt-heap-l1-1-0.dll"]);
  assert.deepEqual(plan.notFound, ["WINHTTP.dll"]);
});

test("再頒布用のフォルダにない Visual C++ のランタイムは vcRuntimeMissing に入れる（同梱しない）", () => {
  const plan = planWindowsDlls(
    { imports: ["vcomp140.dll", "VCRUNTIME140.dll"], delayImports: [] },
    {
      findDll: () => null,
      findVcRuntimeDll: (name) => (name.toLowerCase() === "vcruntime140.dll" ? "C:/crt/vcruntime140.dll" : null),
      readImports: () => ({ imports: ["KERNEL32.dll"], delayImports: [] }),
    },
  );
  assert.deepEqual(plan.vcRuntimeMissing, ["vcomp140.dll"]);
  assert.deepEqual(plan.vcRuntime, [{ name: "VCRUNTIME140.dll", source: "C:/crt/vcruntime140.dll" }]);
  // 探す処理を渡さなければ、すべて見つからない扱い。
  assert.deepEqual(
    planWindowsDlls({ imports: ["MSVCP140.dll"], delayImports: [] }, { findDll: () => null, readImports: () => null })
      .vcRuntimeMissing,
    ["MSVCP140.dll"],
  );
});

test("Visual C++ の再頒布用のフォルダの選び方", () => {
  assert.equal(vcRedistArch("x86_64-pc-windows-msvc"), "x64");
  assert.equal(vcRedistArch("aarch64-pc-windows-msvc"), "arm64");
  assert.equal(vcRedistArch("i686-pc-windows-msvc"), "x86");
  assert.equal(vcRedistArch("riscv64gc-unknown-linux-gnu"), null);
  // 既定の版のファイルがあれば、それを使う。
  assert.equal(pickVcRedistVersion("14.38.33135\r\n", ["14.38.33135", "14.44.35112", "v143"]), "14.38.33135");
  // なければ（または一致しなければ）数字の版の最大（文字列の順ではなく数の順）。
  assert.equal(pickVcRedistVersion(null, ["14.9.1", "14.44.35112", "v143", "onecore"]), "14.44.35112");
  assert.equal(pickVcRedistVersion("14.50.0", ["14.44.35112"]), "14.44.35112");
  assert.equal(pickVcRedistVersion(null, ["v143"]), null);
  assert.equal(isVcRedistFolder("Microsoft.VC143.CRT"), true);
  assert.equal(isVcRedistFolder("Microsoft.VC143.OpenMP"), true);
  assert.equal(isVcRedistFolder("Microsoft.VC143.DebugCRT"), false);
  assert.equal(isVcRedistFolder("onecore"), false);
});

test("同梱するライブラリの Tauri の設定", () => {
  assert.deepEqual(libsTauriConfig("windows", ["zlib1.dll", "raw_r.dll"]), {
    bundle: { resources: { "binaries/libs/raw_r.dll": "raw_r.dll", "binaries/libs/zlib1.dll": "zlib1.dll" } },
  });
  assert.deepEqual(libsTauriConfig("macos", ["libraw_r.24.dylib", "libomp.dylib"]), {
    bundle: { macOS: { frameworks: ["binaries/libs/libomp.dylib", "binaries/libs/libraw_r.24.dylib"] } },
  });
  // .app の最小の版を上げるとき（scripts/lib/worker.mjs）。
  assert.deepEqual(libsTauriConfig("macos", ["libraw_r.24.dylib"], { minimumSystemVersion: "15.0" }), {
    bundle: { macOS: { frameworks: ["binaries/libs/libraw_r.24.dylib"], minimumSystemVersion: "15.0" } },
  });
  assert.deepEqual(libsTauriConfig("macos", [], {}), {});
  assert.deepEqual(libsTauriConfig("linux", ["x.so"]), {});
  assert.deepEqual(libsTauriConfig("windows", []), {});
});

test("target triple と externalBin のファイル名", () => {
  const text = "rustc 1.97.0 (2d8144b78 2026-07-07)\nbinary: rustc\nhost: aarch64-apple-darwin\nrelease: 1.97.0\n";
  assert.equal(parseHostTriple(text), "aarch64-apple-darwin");
  assert.throws(() => parseHostTriple("rustc 1.97.0\n"), /host:/);
  assert.equal(sidecarFileName("genzo-worker", "x86_64-pc-windows-msvc"), "genzo-worker-x86_64-pc-windows-msvc.exe");
  assert.equal(sidecarFileName("genzo-worker", "aarch64-apple-darwin"), "genzo-worker-aarch64-apple-darwin");
  assert.equal(targetOs("x86_64-pc-windows-msvc"), "windows");
  assert.equal(targetOs("aarch64-apple-darwin"), "macos");
  assert.equal(targetOs("x86_64-unknown-linux-gnu"), "linux");
});
