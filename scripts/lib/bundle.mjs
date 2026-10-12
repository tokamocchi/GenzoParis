// 同梱する LibRaw のライブラリの計画と、Tauri の追加の設定（純粋な処理）。

import { dllKey, isVcRuntimeDll, isWindowsSystemDll } from "./pe.mjs";

/** 同梱するライブラリを置くフォルダ（crates/genzo-app からの相対。.gitignore の /binaries の下）。 */
export const LIBS_DIR = "binaries/libs";

/**
 * Windows の LibRaw のフォルダ（.lib のあるところ）のファイル名から、リンクするライブラリの名前
 * （genzo-raw の build.rs の LIBRAW_LIB_NAME。.lib を除いた名前）を選ぶ。
 *
 * - vcpkg の libraw: スレッドセーフな版が `lib/raw_r.lib`（スレッドセーフでない版は `lib/manual-link/raw.lib`。
 *   vcpkg の ports/libraw の fix-install.patch。2026-10-12 に確認）
 * - 上流の `Makefile.msvc`: `lib/libraw.lib`（genzo-raw の既定の名前。スレッドセーフかは要確認）
 *
 * @param {string[]} fileNames フォルダの中のファイル名
 * @returns {string | null}
 */
export function chooseLibrawLibName(fileNames) {
  const lower = new Map(fileNames.map((f) => [f.toLowerCase(), f]));
  for (const candidate of ["raw_r.lib", "libraw_r.lib", "libraw.lib", "raw.lib"]) {
    const found = lower.get(candidate);
    if (found) return found.slice(0, -".lib".length);
  }
  return null;
}

/**
 * ワーカーの読み込む DLL をたどり、同梱する DLL を決める（Windows）。
 *
 * LibRaw のフォルダ（`findDll` が探す場所）にある DLL だけを同梱し、その DLL の依存もたどる。Visual C++ の
 * ランタイムと OS の DLL は同梱しない（docs/packaging.md）。
 *
 * @param {{ imports: string[], delayImports: string[] }} root ワーカーの読み込む DLL
 * @param {{
 *   findDll: (name: string) => string | null,
 *   readImports: (file: string) => { imports: string[], delayImports: string[] },
 * }} io ファイルを探す・読む処理（テストでは差し替える）
 * @returns {{
 *   bundled: Array<{ name: string, source: string }>,
 *   vcRuntime: string[],
 *   system: string[],
 *   notFound: string[],
 * }} `notFound` は、LibRaw のフォルダになく、OS の DLL の一覧にもない DLL（OS の DLL でなければ同梱が要る）
 */
export function planWindowsDlls(root, io) {
  const bundled = new Map();
  const vcRuntime = new Map();
  const system = new Map();
  const notFound = new Map();
  const queue = [root];
  while (queue.length > 0) {
    const current = queue.shift();
    for (const name of [...current.imports, ...current.delayImports]) {
      const key = dllKey(name);
      if (bundled.has(key) || vcRuntime.has(key) || system.has(key) || notFound.has(key)) continue;
      if (isVcRuntimeDll(name)) {
        vcRuntime.set(key, name);
        continue;
      }
      if (isWindowsSystemDll(name)) {
        system.set(key, name);
        continue;
      }
      const source = io.findDll(name);
      if (source === null) {
        notFound.set(key, name);
        continue;
      }
      bundled.set(key, { name, source });
      queue.push(io.readImports(source));
    }
  }
  return {
    bundled: [...bundled.values()],
    vcRuntime: [...vcRuntime.values()],
    system: [...system.values()],
    notFound: [...notFound.values()],
  };
}

/**
 * 同梱するライブラリを Tauri に渡す追加の設定（`tauri build --config` に渡す JSON）を作る。
 *
 * - Windows: `bundle.resources` の対応表で、インストール先のフォルダ（実行ファイルと同じ場所。Tauri の
 *   resource_dir）に置く。Windows はワーカーと同じフォルダの DLL を最初に探す。
 * - macOS: `bundle.macOS.frameworks` に dylib のパスを並べ、.app の Contents/Frameworks に置く。
 * - それ以外（Linux）: 同梱しない（確認用。OS のパッケージの LibRaw を使う）。
 *
 * macOS で `minimumSystemVersion` を渡したときは、.app の最小の版（Info.plist の LSMinimumSystemVersion）も
 * 上書きする（同梱する dylib の minos に合わせて上げるとき。scripts/lib/worker.mjs）。
 *
 * @param {"windows" | "macos" | "linux" | "other"} os
 * @param {string[]} fileNames 同梱するファイルの名前（LIBS_DIR の中）
 * @param {{ minimumSystemVersion?: string }} [options]
 */
export function libsTauriConfig(os, fileNames, options = {}) {
  const sorted = [...fileNames].sort();
  if (os === "windows") {
    if (sorted.length === 0) return {};
    return { bundle: { resources: Object.fromEntries(sorted.map((n) => [`${LIBS_DIR}/${n}`, n])) } };
  }
  if (os === "macos") {
    /** @type {{ frameworks?: string[], minimumSystemVersion?: string }} */
    const macOS = {};
    if (sorted.length > 0) macOS.frameworks = sorted.map((n) => `${LIBS_DIR}/${n}`);
    if (options.minimumSystemVersion) macOS.minimumSystemVersion = options.minimumSystemVersion;
    return Object.keys(macOS).length === 0 ? {} : { bundle: { macOS } };
  }
  return {};
}
