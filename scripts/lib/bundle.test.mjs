// bundle.mjs・target.mjs のテスト（node --test）。

import assert from "node:assert/strict";
import { test } from "node:test";

import { splitBuildArgs } from "./build.mjs";
import { chooseLibrawLibName, libsTauriConfig, planWindowsDlls } from "./bundle.mjs";
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

test("ワーカーの DLL をたどり、LibRaw のフォルダにあるものだけ同梱する", () => {
  const dir = "C:/vcpkg/installed/x64-windows/bin";
  const files = {
    "raw_r.dll": { imports: ["lcms2.dll", "jasper.dll", "zlib1.dll", "MSVCP140.dll", "KERNEL32.dll"], delayImports: [] },
    "lcms2.dll": { imports: ["KERNEL32.dll", "VCRUNTIME140.dll"], delayImports: [] },
    "jasper.dll": { imports: ["jpeg62.dll", "zlib1.dll"], delayImports: ["WINHTTP.dll"] },
    "jpeg62.dll": { imports: ["KERNEL32.dll", "api-ms-win-crt-heap-l1-1-0.dll"], delayImports: [] },
    // 循環しても止まる。
    "zlib1.dll": { imports: ["raw_r.dll"], delayImports: [] },
  };
  const plan = planWindowsDlls(
    { imports: ["KERNEL32.dll", "RAW_R.dll", "VCRUNTIME140.dll", "api-ms-win-crt-runtime-l1-1-0.dll"], delayImports: [] },
    {
      findDll: (name) => (files[name.toLowerCase()] ? `${dir}/${name.toLowerCase()}` : null),
      readImports: (file) => files[file.slice(dir.length + 1)],
    },
  );
  assert.deepEqual(
    plan.bundled.map((b) => b.name),
    ["RAW_R.dll", "lcms2.dll", "jasper.dll", "zlib1.dll", "jpeg62.dll"],
  );
  assert.equal(plan.bundled[0].source, `${dir}/raw_r.dll`);
  assert.deepEqual(plan.vcRuntime, ["VCRUNTIME140.dll", "MSVCP140.dll"]);
  assert.deepEqual(plan.system, ["KERNEL32.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "api-ms-win-crt-heap-l1-1-0.dll"]);
  assert.deepEqual(plan.notFound, ["WINHTTP.dll"]);
});

test("同梱するライブラリの Tauri の設定", () => {
  assert.deepEqual(libsTauriConfig("windows", ["zlib1.dll", "raw_r.dll"]), {
    bundle: { resources: { "binaries/libs/raw_r.dll": "raw_r.dll", "binaries/libs/zlib1.dll": "zlib1.dll" } },
  });
  assert.deepEqual(libsTauriConfig("macos", ["libraw_r.24.dylib", "libomp.dylib"]), {
    bundle: { macOS: { frameworks: ["binaries/libs/libomp.dylib", "binaries/libs/libraw_r.24.dylib"] } },
  });
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
