// 配布物に入れる LibRaw のライセンスの文書と案内（純粋な処理。ファイルの読み書きは scripts/lib/worker.mjs）。
//
// LibRaw は LGPL-2.1 と CDDL-1.0 のどちらかを選べる。このアプリは LGPL-2.1 を選んだ（決定事項ログ No.32）。
// 配布物には次を入れる（Windows はインストール先、macOS は .app の Contents/Resources の licenses/LibRaw）。
// - LICENSE.LGPL・COPYRIGHT: 上流の LibRaw のファイルをそのまま（crates/genzo-app/licenses/LibRaw。0.22.0 の
//   タグから取った。LICENSE.LGPL は 0.21.2・0.21.4・0.22.0・master で同じ内容）
// - README.txt: 案内（使っている版・同梱したファイル・ソースの入手先・差し替えの方法）。ビルドのときに作る
//   （版と同梱したファイルは、ビルドで入ったものによって変わるため）
// - build-recipe/: LibRaw をビルドした手順（vcpkg の port の定義とパッチ、Homebrew の formula）の写し。案内には、
//   その手順のリポジトリのコミット（ビルドの時点のもの）を固定した URL も書く（scripts/lib/worker.mjs が集める）
// - source/: 同梱した LibRaw をビルドしたソースのアーカイブ（vcpkg・Homebrew が取得したもの。SHA を確かめて同梱する。
//   vcpkg は LibRaw-cmake（ビルドの CMake のファイル）も。LGPL-2.1 の第 4 節・第 6 節。scripts/lib/worker.mjs が集める）
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
/** ビルドの手順の写しを置くフォルダ（LICENSES_BUILD_DIR・LICENSES_TARGET_DIR の中）。 */
export const RECIPE_DIR = "build-recipe";
/** ソースのアーカイブを置くフォルダ（LICENSES_BUILD_DIR・LICENSES_TARGET_DIR の中）。 */
export const SOURCE_DIR = "source";

/**
 * LibRaw の版のヘッダ（libraw_version.h）を探す場所。genzo-raw の build.rs と同じく、include のフォルダは
 * `libraw/libraw.h` のあるところ・`libraw.h` のあるところのどちらでもよい。
 *
 * @param {string[]} includeDirs
 * @returns {string[]} 探す順のパス（区切りは "/"。呼び出し側で path.join し直してよい）
 */
export function librawVersionHeaderCandidates(includeDirs) {
  return includeDirs.filter((d) => d).flatMap((d) => [`${d}/libraw/libraw_version.h`, `${d}/libraw_version.h`]);
}

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
 * Tauri の resources に足す対応（ビルドのときのファイル → 配布物の中の場所）。ファイルごとに対応させる
 * （フォルダのまま渡さない）。
 *
 * @param {string[]} [recipeFiles] ビルドの手順の写しのファイル（RECIPE_DIR からの相対。区切りは "/"）
 * @param {string[]} [sourceFiles] ソースのアーカイブ（SOURCE_DIR からの相対）
 * @returns {Record<string, string>}
 */
export function licenseResources(recipeFiles = [], sourceFiles = []) {
  const files = [
    ...LIBRAW_LICENSE_FILES,
    LIBRAW_NOTICE_FILE,
    ...[...recipeFiles].sort().map((f) => `${RECIPE_DIR}/${f}`),
    ...[...sourceFiles].sort().map((f) => `${SOURCE_DIR}/${f}`),
  ];
  return Object.fromEntries(files.map((f) => [`${LICENSES_BUILD_DIR}/${f}`, `${LICENSES_TARGET_DIR}/${f}`]));
}

/**
 * LibRaw をビルドした手順。
 *
 * @typedef {{ kind: "vcpkg" | "homebrew", version?: string, revision?: string, copied: boolean }
 *   | { kind: "unknown" }} LibrawRecipe
 *   `version` は port・formula の版、`revision` は手順のリポジトリ（vcpkg・homebrew-core）のコミット、`copied` は
 *   手順の写しを RECIPE_DIR に置いたか
 */

/**
 * ビルドの手順の案内（日本語と英語の 1 行ずつ）。コミットが分かれば、それを固定した URL にする。
 *
 * @param {LibrawRecipe} recipe
 * @returns {{ ja: string, en: string }}
 */
export function recipeLines(recipe) {
  if (recipe.kind === "unknown") {
    return {
      ja: "LibRaw を独自にビルドしたもの（ビルドの手順は記録していません）",
      en: "a custom build of LibRaw (the build recipe was not recorded)",
    };
  }
  const vcpkg = recipe.kind === "vcpkg";
  const repo = vcpkg ? "https://github.com/microsoft/vcpkg" : "https://github.com/Homebrew/homebrew-core";
  const file = vcpkg ? "ports/libraw" : "Formula/lib/libraw.rb";
  const url = recipe.revision ? `${repo}/${vcpkg ? "tree" : "blob"}/${recipe.revision}/${file}` : null;
  const name = vcpkg ? "vcpkg の port「libraw」" : "Homebrew の formula「libraw」";
  const nameEn = vcpkg ? "the vcpkg port \"libraw\"" : "the Homebrew formula \"libraw\"";
  const ver = recipe.version ? `版 ${recipe.version}。` : "";
  const where = url ?? `${repo}（ビルドの時点のコミットは記録できませんでした）`;
  const copied = recipe.copied ? `。写しを ${RECIPE_DIR} に同梱${vcpkg ? "（port が当てるパッチを含む）" : ""}` : "";
  return {
    ja: `${name}（${ver}${where}${copied}）`,
    en: `${nameEn}${recipe.version ? ` ${recipe.version}` : ""} (${url ?? `${repo}, revision not recorded`}${recipe.copied ? `; a copy is included in ${RECIPE_DIR}` : ""})`,
  };
}

/**
 * 案内（README.txt）の本文。
 *
 * @param {{ os: "windows" | "macos", version: string, bundled: string[], recipe: LibrawRecipe, source?: string[] }} info
 *   `bundled` は同梱した共有ライブラリのファイル名（LibRaw とその依存。Windows は Visual C++ のランタイムを含む）、
 *   `source` は同梱したソースのアーカイブ（SOURCE_DIR の中の名前）
 * @returns {string}
 */
export function librawNotice({ os, version, bundled, recipe, source = [] }) {
  const libraw = bundled.filter((n) => /raw/i.test(n));
  if (libraw.length === 0) throw new Error("同梱したファイルに LibRaw がない");
  const others = bundled.filter((n) => !libraw.includes(n)).sort();
  // 場所と、続ける助詞（括弧で終わるときは間を空けない）。
  const where =
    os === "windows" ? "インストール先のフォルダ（既定は %LOCALAPPDATA%\\GenzoParis）" : "GenzoParis.app/Contents/Frameworks ";
  const whereEn = os === "windows" ? "the installation folder (default: %LOCALAPPDATA%\\GenzoParis)" : "GenzoParis.app/Contents/Frameworks";
  const { ja: recipeJa, en: recipeEn } = recipeLines(recipe);
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
    "■ LibRaw のソース",
    ...(source.length > 0
      ? [
          `  - 同梱したファイルをビルドしたソース（このフォルダの ${SOURCE_DIR}。入手元が取得したアーカイブをそのまま）:`,
          ...[...source].sort().map((f) => `      ${SOURCE_DIR}/${f}`),
        ]
      : []),
    `  - LibRaw ${version} のソース: https://github.com/LibRaw/LibRaw/tree/${version}`,
    "  - LibRaw の配布ページ: https://www.libraw.org/download",
    `  - 同梱したファイルのビルドの手順: ${recipeJa}`,
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
    ...(source.length > 0
      ? [`The source archives used for the bundled build are included in the ${SOURCE_DIR} folder: ${[...source].sort().join(", ")}.`]
      : []),
    "",
  ];
  return `${lines.join("\n")}`;
}
