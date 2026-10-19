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
 * - LibRaw のフォルダ（`findDll` が探す場所）にある DLL を同梱し、その DLL の依存もたどる。
 * - Visual C++ のランタイムの DLL（`vcruntime140.dll`・`msvcp140.dll` など）は、Visual Studio の再頒布用のフォルダ
 *   （`findVcRuntimeDll` が探す場所）から同梱し、その依存もたどる（ワーカーと同じフォルダに置く「アプリと一緒に置く」
 *   配置。管理者の権限が要らない。決定事項ログ No.65・73、docs/packaging.md）。見つからないものは
 *   `vcRuntimeMissing` に入れる（呼び出し側でエラーにする）。
 * - OS の DLL（UCRT の `api-ms-win-crt-*`・`ucrtbase.dll` を含む。Windows 10 以降は OS の一部）は同梱しない。
 *
 * @param {{ imports: string[], delayImports: string[] }} root ワーカーの読み込む DLL
 * @param {{
 *   findDll: (name: string) => string | null,
 *   findVcRuntimeDll?: (name: string) => string | null,
 *   readImports: (file: string) => { imports: string[], delayImports: string[] },
 * }} io ファイルを探す・読む処理（テストでは差し替える）
 * @returns {{
 *   bundled: Array<{ name: string, source: string }>,
 *   vcRuntime: Array<{ name: string, source: string }>,
 *   vcRuntimeMissing: string[],
 *   system: string[],
 *   notFound: string[],
 * }} `notFound` は、LibRaw のフォルダになく、OS の DLL の一覧にもない DLL（OS の DLL でなければ同梱が要る）
 */
export function planWindowsDlls(root, io) {
  const bundled = new Map();
  const vcRuntime = new Map();
  const vcRuntimeMissing = new Map();
  const system = new Map();
  const notFound = new Map();
  const queue = [root];
  const seen = (key) =>
    bundled.has(key) || vcRuntime.has(key) || vcRuntimeMissing.has(key) || system.has(key) || notFound.has(key);
  while (queue.length > 0) {
    const current = queue.shift();
    for (const name of [...current.imports, ...current.delayImports]) {
      const key = dllKey(name);
      if (seen(key)) continue;
      if (isVcRuntimeDll(name)) {
        const source = io.findVcRuntimeDll?.(name) ?? null;
        if (source === null) {
          vcRuntimeMissing.set(key, name);
          continue;
        }
        vcRuntime.set(key, { name, source });
        queue.push(io.readImports(source));
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
    vcRuntimeMissing: [...vcRuntimeMissing.values()],
    system: [...system.values()],
    notFound: [...notFound.values()],
  };
}

/**
 * Visual Studio の再頒布用のフォルダ（`VC\Redist\MSVC\<版>\<アーキテクチャ>`）のアーキテクチャの名前。
 *
 * @param {string} triple Rust の target triple
 * @returns {string | null}
 */
export function vcRedistArch(triple) {
  const arch = triple.split("-")[0];
  if (arch === "x86_64") return "x64";
  if (arch === "aarch64") return "arm64";
  if (/^i[3-6]86$/.test(arch)) return "x86";
  return null;
}

/**
 * 再頒布用のフォルダの版（`VC\Redist\MSVC` の下のフォルダ名）を選ぶ。Visual Studio が既定として書く
 * `VC\Auxiliary\Build\Microsoft.VCRedistVersion.default.txt` の版があればそれを、なければ数字の版の最大を使う。
 *
 * @param {string | null} defaultVersionText `Microsoft.VCRedistVersion.default.txt` の中身（なければ null）
 * @param {string[]} dirNames `VC\Redist\MSVC` の下のフォルダ名
 * @returns {string | null}
 */
export function pickVcRedistVersion(defaultVersionText, dirNames) {
  const preferred = defaultVersionText?.trim();
  if (preferred && dirNames.includes(preferred)) return preferred;
  const versions = dirNames.filter((n) => /^\d+(\.\d+)*$/.test(n));
  if (versions.length === 0) return null;
  const cmp = (a, b) => {
    const x = a.split(".").map(Number);
    const y = b.split(".").map(Number);
    for (let i = 0; i < Math.max(x.length, y.length); i++) {
      const d = (x[i] ?? 0) - (y[i] ?? 0);
      if (d !== 0) return d;
    }
    return 0;
  };
  return versions.sort(cmp).at(-1) ?? null;
}

/**
 * 再頒布用のフォルダ（アーキテクチャの下）の中で、ランタイムの DLL を探すフォルダか（`Microsoft.VC143.CRT`・
 * `Microsoft.VC143.OpenMP` など）。デバッグ版（再頒布できない）は除く。
 *
 * @param {string} name
 */
export function isVcRedistFolder(name) {
  return /^Microsoft\.VC\d+\./i.test(name) && !/debug/i.test(name);
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
 * `resources` は、Windows・macOS で `bundle.resources` に足す対応表（LibRaw のライセンスの文書と案内。
 * scripts/lib/notice.mjs。Windows はインストール先、macOS は .app の Contents/Resources からの場所）。
 *
 * @param {"windows" | "macos" | "linux" | "other"} os
 * @param {string[]} fileNames 同梱するファイルの名前（LIBS_DIR の中）
 * @param {{ minimumSystemVersion?: string, resources?: Record<string, string> }} [options]
 */
export function libsTauriConfig(os, fileNames, options = {}) {
  const sorted = [...fileNames].sort();
  const extra = options.resources ?? {};
  if (os === "windows") {
    const resources = { ...Object.fromEntries(sorted.map((n) => [`${LIBS_DIR}/${n}`, n])), ...extra };
    return Object.keys(resources).length === 0 ? {} : { bundle: { resources } };
  }
  if (os === "macos") {
    /** @type {{ frameworks?: string[], minimumSystemVersion?: string }} */
    const macOS = {};
    if (sorted.length > 0) macOS.frameworks = sorted.map((n) => `${LIBS_DIR}/${n}`);
    if (options.minimumSystemVersion) macOS.minimumSystemVersion = options.minimumSystemVersion;
    /** @type {{ macOS?: typeof macOS, resources?: Record<string, string> }} */
    const bundle = {};
    if (Object.keys(macOS).length > 0) bundle.macOS = macOS;
    if (Object.keys(extra).length > 0) bundle.resources = { ...extra };
    return Object.keys(bundle).length === 0 ? {} : { bundle };
  }
  return {};
}
