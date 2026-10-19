// 配布物に入れる LibRaw のライセンスの文書と案内（純粋な処理。ファイルの読み書きは scripts/lib/worker.mjs）。
//
// LibRaw は LGPL-2.1 と CDDL-1.0 のどちらかを選べる。このアプリは LGPL-2.1 を選んだ（決定事項ログ No.32）。
// 配布物には次を入れる（Windows はインストール先、macOS は .app の Contents/Resources の licenses/LibRaw）。
// - LICENSE.LGPL・COPYRIGHT: 上流の LibRaw のファイルをそのまま（crates/genzo-app/licenses/LibRaw。0.22.0 の
//   タグから取った。LICENSE.LGPL は 0.21.2・0.21.4・0.22.0・master で同じ内容）
// - README.txt: 案内（使っている版・同梱したファイル・ソースの入手先・差し替えの方法）。ビルドのときに作る
//   （版と同梱したファイルは、ビルドで入ったものによって変わるため）
// 案内の文言が LGPL の条件を満たすかは、人（必要なら専門家）が確かめる（docs/packaging.md の 8 章）。

/** 上流の LibRaw のファイル（crates/genzo-app からの相対）。 */
export const LIBRAW_LICENSE_SOURCE_DIR = "licenses/LibRaw";
/** 上流のファイルの名前（そのまま同梱する）。 */
export const LIBRAW_LICENSE_FILES = ["LICENSE.LGPL", "COPYRIGHT"];
/** 案内のファイルの名前。 */
export const LIBRAW_NOTICE_FILE = "README.txt";
/** ビルドのときに文書を集めるフォルダ（crates/genzo-app からの相対。.gitignore の /binaries の下）。 */
export const LICENSES_BUILD_DIR = "binaries/licenses/LibRaw";
/** 配布物の中の置き場所（Tauri の resources の行き先。Windows はインストール先、macOS は Contents/Resources から）。 */
export const LICENSES_TARGET_DIR = "licenses/LibRaw";

/**
 * LibRaw のヘッダ（libraw/libraw_version.h）から版（"0.22.0" など）を読む。読めなければ null。
 *
 * @param {string} header
 * @returns {string | null}
 */
export function parseLibrawVersion(header) {
  const part = (name) => {
    const m = new RegExp(`^\\s*#\\s*define\\s+${name}\\s+(\\d+)\\s*$`, "m").exec(header);
    return m ? m[1] : null;
  };
  const parts = ["LIBRAW_MAJOR_VERSION", "LIBRAW_MINOR_VERSION", "LIBRAW_PATCH_VERSION"].map(part);
  return parts.every((p) => p !== null) ? parts.join(".") : null;
}

/**
 * Tauri の resources に足す対応（ビルドのときのファイル → 配布物の中の場所）。
 *
 * @returns {Record<string, string>}
 */
export function licenseResources() {
  return Object.fromEntries(
    [...LIBRAW_LICENSE_FILES, LIBRAW_NOTICE_FILE].map((f) => [`${LICENSES_BUILD_DIR}/${f}`, `${LICENSES_TARGET_DIR}/${f}`]),
  );
}

/**
 * 案内（README.txt）の本文。
 *
 * @param {{ os: "windows" | "macos", version: string, bundled: string[] }} info
 *   `bundled` は同梱した共有ライブラリのファイル名（LibRaw とその依存。Windows は Visual C++ のランタイムを含む）
 * @returns {string}
 */
export function librawNotice({ os, version, bundled }) {
  const libraw = bundled.filter((n) => /raw/i.test(n));
  if (libraw.length === 0) throw new Error("同梱したファイルに LibRaw がない");
  const others = bundled.filter((n) => !libraw.includes(n)).sort();
  // 場所と、続ける助詞（括弧で終わるときは間を空けない）。
  const where =
    os === "windows" ? "インストール先のフォルダ（既定は %LOCALAPPDATA%\\GenzoParis）" : "GenzoParis.app/Contents/Frameworks ";
  const whereEn = os === "windows" ? "the installation folder (default: %LOCALAPPDATA%\\GenzoParis)" : "GenzoParis.app/Contents/Frameworks";
  const recipe =
    os === "windows"
      ? "vcpkg の port「libraw」（https://github.com/microsoft/vcpkg/tree/master/ports/libraw 。ビルドの時点の版。port が当てるパッチを含む）"
      : "Homebrew の formula「libraw」（https://github.com/Homebrew/homebrew-core/blob/HEAD/Formula/lib/libraw.rb 。ビルドの時点の版）";
  const recipeEn =
    os === "windows"
      ? "the vcpkg port \"libraw\" (https://github.com/microsoft/vcpkg/tree/master/ports/libraw, including its patches)"
      : "the Homebrew formula \"libraw\" (https://github.com/Homebrew/homebrew-core/blob/HEAD/Formula/lib/libraw.rb)";
  const replace =
    os === "windows"
      ? [
          `  ${where}の ${libraw.join("・")} を、同じ名前の、互換のある LibRaw（スレッドセーフな版。`,
          "  同じ ABI の版）のファイルで置き換えてください。",
        ]
      : [
          `  ${where}の ${libraw.join("・")} を、互換のある LibRaw（スレッドセーフな版。同じ ABI の版）の`,
          "  ファイルで置き換えてください。依存する dylib の参照は @loader_path にそろえてください（install_name_tool）。",
          "  置き換えると .app の署名が壊れるので、次のようにアドホック署名をし直してください（.app の場所に合わせて）。",
          "    codesign --force --deep --sign - /Applications/GenzoParis.app",
        ];
  const lines = [
    "GenzoParis が使っている LibRaw について",
    "========================================",
    "",
    `GenzoParis は、RAW の展開に LibRaw ${version} を使っています。LibRaw は、RAW を開く専用のプログラム`,
    "（genzo-worker）が共有ライブラリとして読み込みます。アプリの本体は LibRaw に依存しません。",
    "",
    "■ ライセンス",
    "  LibRaw は Copyright (C) LibRaw LLC（https://www.libraw.org）です。LibRaw は GNU Lesser General Public",
    "  License version 2.1（LGPL-2.1）と CDDL-1.0 のどちらかを選んで使えます。GenzoParis は LGPL-2.1 を選んで",
    "  います。",
    "  - LICENSE.LGPL: LGPL-2.1 の全文（上流の LibRaw の配布物のファイルをそのまま）",
    "  - COPYRIGHT: LibRaw の著作権表示（上流の LibRaw の配布物のファイルをそのまま）",
    "",
    "■ 同梱したファイル",
    `  ${where}に、次のファイルを同梱しています。`,
    `  - LibRaw: ${libraw.join(", ")}`,
    ...(others.length > 0 ? [`  - LibRaw などが使うライブラリ: ${others.join(", ")}`] : []),
    "",
    "■ LibRaw のソースの入手先",
    `  - LibRaw ${version} のソース: https://github.com/LibRaw/LibRaw/tree/${version}`,
    "  - LibRaw の配布ページ: https://www.libraw.org/download",
    `  - 同梱したファイルのビルドの手順: ${recipe}`,
    "",
    "■ LibRaw の差し替え",
    ...replace,
    "  差し替えた LibRaw での動作は確かめていません。",
    "",
    "----------------------------------------",
    "About LibRaw in GenzoParis",
    "",
    `GenzoParis uses LibRaw ${version} (Copyright (C) LibRaw LLC, https://www.libraw.org) to decode raw images.`,
    "LibRaw is dual-licensed under LGPL-2.1 and CDDL-1.0; GenzoParis uses it under the GNU Lesser General Public",
    "License version 2.1. See LICENSE.LGPL for the full text and COPYRIGHT for the copyright notice (both copied",
    "unchanged from the upstream LibRaw distribution).",
    `LibRaw is bundled as a shared library (${libraw.join(", ")}) in ${whereEn}, loaded only by the`,
    "genzo-worker helper program. You may replace it with a compatible (thread-safe, ABI-compatible) build of LibRaw.",
    `Source code: https://github.com/LibRaw/LibRaw/tree/${version} (built with ${recipeEn}).`,
    "",
  ];
  return `${lines.join("\n")}`;
}
