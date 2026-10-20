// 配布物に同梱した C / C++ の共有ライブラリ（LibRaw とその依存・Visual C++ のランタイム）のライセンスの文書
// （licenses/third-party/native.txt）と、LibRaw のソースの収集の、純粋な処理（DIST-05。ファイルの読み書きと
// コマンドの実行は scripts/lib/worker.mjs）。
//
// - 同梱したファイルごとに、それを入れたパッケージ（Windows は vcpkg の port、macOS は Homebrew の formula）を
//   調べ、そのパッケージがインストールしたライセンスの文書を載せる。
//   - vcpkg: installed/vcpkg/info/<port>_<版>_<triplet>.list（port がインストールしたファイルの一覧）で持ち主の
//     port を決め、installed/<triplet>/share/<port>/copyright（vcpkg がすべての port に求める著作権の文書）を載せる。
//     ライセンスの表示とホームページは ports/<port>/vcpkg.json。
//   - Homebrew: dylib の実体の場所（Cellar/<formula>/<版>/…）で formula を決め、keg の直下と share/doc の下の
//     ライセンス・NOTICE のファイル（Homebrew は、ソースの最上位の LICENSE・COPYING などを keg の直下に写す）と、
//     IJG のライセンスの README（README.ijg）を載せる。ライセンスの表示・ホームページ・formula の場所は
//     `brew info --json=v2`。
//   - ライセンスの文書が 1 つも見つからないパッケージがあれば止める（文書の欠けた配布物を作らない）。
// - libjpeg（IJG のコード）を同梱したときは、IJG のライセンスが実行形式だけを配るときに求める一文
//   （IJG_STATEMENT）を書く。
// - Visual C++ のランタイム（Windows）は、ファイルの名前・著作権者・入手元（Visual Studio の再頒布用のフォルダの
//   名前）だけを書く（Microsoft の条項の扱いは人の確認待ち。docs/license_compliance.md）。
// - 文書には、ビルドした PC の絶対パス（利用者の名前を含みうる）を書かない。
//
// この文書で各ライセンスの条件を満たすかは、人（必要なら専門家）が確かめる（docs/license_compliance.md）。

import { compareNatural, isLicenseFileName, isNoticeFileName, normalizeText } from "./third-party.mjs";

/** 文書の名前（licenses/third-party の下）。 */
export const NATIVE_NOTICES_FILE = "native.txt";
/** ビルドのときに文書を置く場所（crates/genzo-app からの相対。.gitignore の /binaries の下）。 */
export const NATIVE_NOTICES_BUILD_PATH = `binaries/licenses/third-party/${NATIVE_NOTICES_FILE}`;
/** 配布物の中の場所（Tauri の resources の行き先）。 */
export const NATIVE_NOTICES_TARGET_PATH = `licenses/third-party/${NATIVE_NOTICES_FILE}`;
/** LibRaw のソースを置くフォルダ（licenses/LibRaw の下）。 */
export const LIBRAW_SOURCE_DIR = "source";

/**
 * IJG のライセンス（libjpeg の README.ijg）が、実行形式だけを配るときに添付の文書に書くよう求める一文
 * （原文のまま）。
 */
export const IJG_STATEMENT = "this software is based in part on the work of the Independent JPEG Group";

/**
 * @typedef {{
 *   name: string, version: string, license: string | null, homepage: string | null, recipe: string | null,
 *   source: string | null, files: string[], texts: Array<{ file: string, text: string }>,
 * }} NativeComponent
 *   `name`・`version` は vcpkg の port・Homebrew の formula の名前と版、`license` はその表示（SPDX の式）、
 *   `recipe` はビルドの手順の URL（コミットを固定）、`source` は上流のソースの URL（分かれば）、`files` は同梱した
 *   ファイルの名前、`texts` はライセンスの文書（`file` はパッケージのフォルダの中の場所）
 * @typedef {{ files: string[], folders: string[] }} VcRuntime
 *   `folders` は再頒布用のフォルダの名前（VC\Redist\MSVC からの相対。vcRedistFolderLabel）
 */

// ---------------------------------------------------------------------------------------------------------------
// vcpkg

/**
 * vcpkg の info の一覧のファイルの名前（`<port>_<版>_<triplet>.list`）を読む。port の名前は `_` を含まない
 * （小文字・数字・`-`）ので、最初の `_` で分ける。違う triplet・形の違うものは null。
 *
 * @param {string} fileName
 * @param {string} triplet
 * @returns {{ port: string, version: string } | null}
 */
export function parseVcpkgListName(fileName, triplet) {
  const suffix = `_${triplet}.list`;
  if (!fileName.endsWith(suffix)) return null;
  const rest = fileName.slice(0, -suffix.length);
  const i = rest.indexOf("_");
  if (i <= 0 || i === rest.length - 1) return null;
  return { port: rest.slice(0, i), version: rest.slice(i + 1) };
}

/**
 * vcpkg の info の一覧から、`<triplet>/bin/` の DLL の持ち主の port の索引を作る。
 *
 * @param {Array<{ name: string, text: string }>} lists info の一覧のファイル（名前と中身）
 * @param {string} triplet
 * @returns {Map<string, { port: string, version: string }>} 小文字の DLL の名前 → 持ち主
 */
export function vcpkgDllOwners(lists, triplet) {
  const owners = new Map();
  const prefix = `${triplet}/bin/`.toLowerCase();
  for (const l of lists) {
    const owner = parseVcpkgListName(l.name, triplet);
    if (!owner) continue;
    for (const line of l.text.split(/\r?\n/)) {
      const p = line.trim().replace(/\\/g, "/").toLowerCase();
      if (p.startsWith(prefix) && p.endsWith(".dll") && !p.slice(prefix.length).includes("/")) {
        owners.set(p.slice(prefix.length), owner);
      }
    }
  }
  return owners;
}

/**
 * vcpkg が port をビルドしたときに記録する ABI の情報（`installed/<triplet>/share/<port>/vcpkg_abi_info.txt`。
 * 1 行に「キー 値」。port のフォルダのファイルは、キーが port のフォルダからの相対のパス、値がその SHA-256）を読む。
 *
 * @param {string} text
 * @returns {Map<string, string>}
 */
export function parseVcpkgAbiInfo(text) {
  const out = new Map();
  for (const line of text.split(/\r?\n/)) {
    const i = line.indexOf(" ");
    if (i > 0) out.set(line.slice(0, i), line.slice(i + 1).trim());
  }
  return out;
}

/**
 * 今の ports/<port> のファイルが、入っている port をビルドしたときのもの（ABI の情報）と同じか。違う・記録のない
 * ファイルの相対のパスを返す（空なら同じ）。vcpkg を更新したのに port を入れ直していないと、ports の vcpkg.json の
 * ライセンスの表示・portfile が、同梱するもののビルドと食い違うため。
 *
 * @param {Map<string, string>} abi parseVcpkgAbiInfo の結果
 * @param {Array<{ file: string, sha256: string }>} portFiles port のフォルダのファイル（相対のパスは "/" 区切り）
 * @returns {string[]}
 */
export function changedVcpkgPortFiles(abi, portFiles) {
  return portFiles
    .filter((f) => {
      const recorded = abi.get(f.file) ?? abi.get(f.file.replaceAll("/", "\\"));
      return recorded === undefined || recorded.toLowerCase() !== f.sha256.toLowerCase();
    })
    .map((f) => f.file)
    .sort(compareNatural);
}

/**
 * vcpkg の port の vcpkg.json から、ライセンスの表示・ホームページ・版（port-version を `#` で付ける）を読む。
 *
 * @param {any} manifest
 * @returns {{ license: string | null, homepage: string | null, version: string | null }}
 */
export function vcpkgPortInfo(manifest) {
  const v = manifest?.version ?? manifest?.["version-semver"] ?? manifest?.["version-date"] ?? manifest?.["version-string"];
  const pv = manifest?.["port-version"];
  return {
    license: typeof manifest?.license === "string" ? manifest.license : null,
    homepage: typeof manifest?.homepage === "string" ? manifest.homepage : null,
    version: v === undefined ? null : pv ? `${v}#${pv}` : String(v),
  };
}

/**
 * vcpkg の portfile.cmake の `vcpkg_from_github(...)` の、取得するリポジトリ・REF・SHA512・パッチを読む。
 * REF の `${VERSION}` は `version` に置き換える（ほかの変数を含む REF は読めないので止める）。
 *
 * @param {string} portfile
 * @param {string} version port の版（port-version を除く）
 * @returns {Array<{ repo: string, ref: string, sha512: string, patches: string[] }>}
 */
export function parseVcpkgFromGithub(portfile, version) {
  const out = [];
  const text = portfile.replace(/#[^\n]*/g, "");
  for (const m of text.matchAll(/vcpkg_from_github\s*\(([^)]*)\)/g)) {
    const words = m[1].match(/"[^"]*"|[^\s"]+/g)?.map((w) => w.replace(/^"|"$/g, "")) ?? [];
    const keys = new Set(["OUT_SOURCE_PATH", "REPO", "REF", "SHA512", "HEAD_REF", "PATCHES", "GITHUB_HOST", "AUTHORIZATION_TOKEN", "FILE_DISAMBIGUATOR"]);
    /** @type {Record<string, string[]>} */
    const args = {};
    let key = null;
    for (const w of words) {
      if (keys.has(w)) {
        key = w;
        args[key] = [];
      } else if (key) {
        args[key].push(w);
      }
    }
    const one = (k) => {
      const v = args[k];
      if (!v || v.length !== 1) throw new Error(`vcpkg_from_github の ${k} が読めない: ${m[0].replace(/\s+/g, " ").slice(0, 200)}`);
      return v[0];
    };
    if (args.GITHUB_HOST || args.FILE_DISAMBIGUATOR) {
      throw new Error(`vcpkg_from_github の GITHUB_HOST・FILE_DISAMBIGUATOR には対応していない: ${one("REPO")}`);
    }
    const ref = one("REF").replace(/\$\{VERSION\}/g, version);
    if (ref.includes("${")) throw new Error(`vcpkg_from_github の REF の変数が読めない: ${ref}`);
    out.push({ repo: one("REPO"), ref, sha512: one("SHA512").toLowerCase(), patches: args.PATCHES ?? [] });
  }
  return out;
}

/**
 * vcpkg_from_github が downloads に置くファイルの名前（`<org>-<repo>-<REF の / を _- に>.tar.gz`）。
 *
 * @param {string} repo "LibRaw/LibRaw" の形
 * @param {string} ref
 */
export function vcpkgArchiveName(repo, ref) {
  const [org, name] = repo.split("/");
  return `${org}-${name}-${ref.replaceAll("/", "_-")}.tar.gz`;
}

/** vcpkg_from_github が取りに行く URL（GitHub の archive）。 */
export function githubArchiveUrl(repo, ref) {
  return `https://github.com/${repo}/archive/${ref}.tar.gz`;
}

/**
 * Visual C++ の再頒布用のフォルダのパスから、文書に書く名前（`VC\Redist\MSVC` より後。なければフォルダの名前）。
 *
 * @param {string} dir
 * @returns {string | null}
 */
export function vcRedistFolderLabel(dir) {
  const parts = dir.split(/[\\/]+/).filter((p) => p !== "");
  const i = parts.findIndex((p, k) => p.toLowerCase() === "msvc" && parts[k - 1]?.toLowerCase() === "redist");
  if (i !== -1 && i + 1 < parts.length) return parts.slice(i + 1).join("\\");
  return parts.length > 0 ? parts[parts.length - 1] : null;
}

// ---------------------------------------------------------------------------------------------------------------
// Homebrew

/**
 * dylib の実体の場所（`<prefix>/Cellar/<formula>/<版>/…`）から formula を読む。Cellar の外なら null。
 *
 * @param {string} realPath
 * @returns {{ formula: string, version: string, keg: string } | null} `keg` は keg のフォルダ
 */
export function parseCellarPath(realPath) {
  const p = realPath.replace(/\\/g, "/");
  const m = /^(.*\/Cellar\/([^/]+)\/([^/]+))\//.exec(p);
  return m ? { formula: m[2], version: m[3], keg: m[1] } : null;
}

/**
 * パッケージのフォルダにある、ライセンスの文書として載せるファイルか（LICENSE・COPYING・COPYRIGHT・NOTICE の
 * 形の名前と、IJG のライセンスの README.ijg）。
 */
export function isNativeLicenseFile(name) {
  return isLicenseFileName(name) || isNoticeFileName(name) || /^readme\.ijg$/i.test(name);
}

/**
 * Homebrew の `brew info --json=v2` の formula から、文書に書く情報を読む。
 *
 * @param {any} f
 * @returns {{ license: string | null, homepage: string | null, recipe: string | null, source: string | null,
 *   sourceSha256: string | null, stable: string | null, installed: string[] }}
 */
export function homebrewFormulaInfo(f) {
  const recipe =
    typeof f?.tap_git_head === "string" && f.tap_git_head !== "" && typeof f?.ruby_source_path === "string"
      ? `https://github.com/Homebrew/homebrew-core/blob/${f.tap_git_head}/${f.ruby_source_path}`
      : null;
  const stable = f?.urls?.stable;
  return {
    license: typeof f?.license === "string" ? f.license : null,
    homepage: typeof f?.homepage === "string" ? f.homepage : null,
    recipe,
    source: typeof stable?.url === "string" ? stable.url : null,
    sourceSha256: typeof stable?.checksum === "string" ? stable.checksum.toLowerCase() : null,
    stable: typeof f?.versions?.stable === "string" ? f.versions.stable : null,
    installed: Array.isArray(f?.installed) ? f.installed.map((i) => i?.version).filter((v) => typeof v === "string") : [],
  };
}

/**
 * keg の版のフォルダの名前（`3.2.0`・`2.19.1_1` など。`_<数>` は Homebrew の revision）から、上流の版。
 *
 * @param {string} kegVersion
 */
export function kegUpstreamVersion(kegVersion) {
  return kegVersion.replace(/_\d+$/, "");
}

// ---------------------------------------------------------------------------------------------------------------
// 文書

/**
 * libjpeg（IJG のコード）を含むか（パッケージが libjpeg・libjpeg-turbo か、ライセンスの表示に IJG があるか。
 * openjpeg（JPEG 2000）などは当たらない）。
 */
export function needsIjgStatement(components) {
  return components.some((c) => /^(lib)?jpeg(-turbo)?$/i.test(c.name) || /\bIJG\b/.test(c.license ?? ""));
}

/**
 * 同梱したライブラリをパッケージごとにまとめる（同じ名前・版のものは 1 つにし、ファイルを足す）。ライセンスの
 * 文書のないパッケージがあれば止める。
 *
 * @param {NativeComponent[]} components
 * @returns {NativeComponent[]} 名前の順
 */
export function mergeComponents(components) {
  /** @type {Map<string, NativeComponent>} */
  const byKey = new Map();
  for (const c of components) {
    const key = `${c.name} ${c.version}`;
    const prev = byKey.get(key);
    if (!prev) {
      byKey.set(key, { ...c, files: [...c.files], texts: [...c.texts] });
      continue;
    }
    prev.files.push(...c.files);
    for (const t of c.texts) if (!prev.texts.some((x) => x.file === t.file)) prev.texts.push(t);
  }
  const out = [...byKey.values()];
  for (const c of out) {
    c.files = [...new Set(c.files)].sort(compareNatural);
    // 同じ中身の文書は 1 つにする（keg の直下と share/doc の両方にある LICENSE など）。
    const seen = new Set();
    c.texts = c.texts
      .map((t) => ({ file: t.file, text: normalizeText(t.text) }))
      .filter((t) => t.text !== "" && !seen.has(t.text) && seen.add(t.text))
      .sort((a, b) => compareNatural(a.file, b.file));
    if (c.texts.length === 0) {
      throw new Error(
        `同梱した ${c.files.join(", ")}（${c.name} ${c.version}）のライセンスの文書が見つからない。` +
          "入手元のパッケージの文書を確かめる（docs/license_compliance.md）",
      );
    }
  }
  return out.sort((a, b) => compareNatural(a.name, b.name) || compareNatural(a.version, b.version));
}

const RULE = "=".repeat(80);
const THIN = "-".repeat(80);

/**
 * 文書の本文（UTF-8・改行は LF）。
 *
 * @param {{ os: "windows" | "macos", triple: string, manager: string, components: NativeComponent[],
 *   vcRuntime: VcRuntime | null }} info
 *   `manager` は入手元の説明（"vcpkg（https://github.com/microsoft/vcpkg/tree/<コミット>）" など）
 * @returns {string}
 */
export function formatNativeNotices({ os, triple, manager, components, vcRuntime }) {
  const list = mergeComponents(components);
  // IJG のライセンスの本文（README.ijg の LEGAL ISSUES）を載せられないなら止める（libjpeg-turbo の LICENSE.md は
  // README.ijg を参照するだけ）。
  for (const c of list.filter((x) => needsIjgStatement([x]))) {
    if (!c.texts.some((t) => /independent jpeg group/i.test(t.text) && /legal issues|permission is hereby granted/i.test(t.text))) {
      throw new Error(`${c.name} ${c.version} の IJG のライセンスの本文（README.ijg）が見つからない`);
    }
  }
  const osName = os === "windows" ? "Windows（x64）" : "macOS（Apple Silicon）";
  const fileRows = [
    ...list.flatMap((c) => c.files.map((f) => ({ f, what: `${c.name} ${c.version}` }))),
    ...(vcRuntime?.files ?? []).map((f) => ({ f, what: "Microsoft Visual C++ のランタイム / Microsoft Visual C++ Runtime" })),
  ].sort((a, b) => compareNatural(a.f.toLowerCase(), b.f.toLowerCase()));
  const lines = [
    "GenzoParis に同梱した C / C++ のライブラリのライセンス",
    "Licenses of the C / C++ libraries bundled with GenzoParis",
    RULE,
    "",
    `GenzoParis の配布物（${osName}）に同梱した共有ライブラリ（RAW を開く専用のプログラム genzo-worker が`,
    "読み込む LibRaw と、LibRaw などが使うライブラリ）の一覧と、そのライセンスの文書です。文書は、配布物を作るときに",
    "ライブラリの入手元のパッケージ（vcpkg・Homebrew）がインストールしたものを、そのまま載せています。この文書は、",
    "scripts/prepare-worker.mjs が作ったものです。",
    "This document lists the shared libraries bundled with GenzoParis (LibRaw and the libraries it uses, loaded by",
    "the genzo-worker helper program) with their license documents, copied unchanged from the packages (vcpkg or",
    "Homebrew) used for the build. Generated by scripts/prepare-worker.mjs.",
    "",
    `  対象 / Target: ${triple}`,
    `  入手元 / Obtained from: ${manager}`,
    `  ライブラリの数 / Number of libraries: ${list.length}`,
    "",
  ];
  if (needsIjgStatement(list)) {
    lines.push(
      "  この配布物に含まれる libjpeg のコードについて / About the libjpeg code included in this distribution:",
      `  ${IJG_STATEMENT[0].toUpperCase()}${IJG_STATEMENT.slice(1)}.`,
      "  （このソフトウェアの一部は、Independent JPEG Group の成果にもとづいています。）",
      "",
    );
  }
  let section = 0;
  lines.push(
    `${++section}. 同梱したファイル / Bundled files`,
    THIN,
    ...fileRows.map((r) => `  ${r.f}: ${r.what}`),
    "",
    `${++section}. ライブラリとライセンスの文書 / Libraries and their license documents`,
  );
  for (const [n, c] of list.entries()) {
    lines.push(
      RULE,
      `[${n + 1}] ${c.name} ${c.version}`,
      `  ライセンス / License: ${c.license ?? "（入手元に表示なし / not declared by the package）"}`,
      ...(c.homepage ? [`  ホームページ / Homepage: ${c.homepage}`] : []),
      ...(c.source ? [`  ソース / Source: ${c.source}`] : []),
      ...(c.recipe ? [`  ビルドの手順 / Build recipe: ${c.recipe}`] : []),
      `  同梱したファイル / Bundled files: ${c.files.join(", ")}`,
      ...(/^libraw$/i.test(c.name)
        ? ["  LibRaw の案内とソースは licenses/LibRaw にあります。 / See licenses/LibRaw for the LibRaw notice and source code."]
        : []),
    );
    for (const t of c.texts) {
      lines.push(THIN, `（${t.file}）`, "", t.text, "");
    }
  }
  if (vcRuntime && vcRuntime.files.length > 0) {
    lines.push(
      RULE,
      `${++section}. Microsoft Visual C++ のランタイム / Microsoft Visual C++ Runtime`,
      THIN,
      `  ファイル / Files: ${[...vcRuntime.files].sort((a, b) => compareNatural(a.toLowerCase(), b.toLowerCase())).join(", ")}`,
      "  著作権者 / Copyright holder: Microsoft Corporation",
      `  入手元 / Obtained from: Microsoft Visual Studio の再頒布用のファイル（${
        vcRuntime.folders.length > 0 ? vcRuntime.folders.map((f) => `VC\\Redist\\MSVC\\${f}`).join("、") : "VC\\Redist\\MSVC"
      }）`,
      "  これらのファイルは Microsoft Corporation のもので、Visual Studio の再頒布用のファイルを変更せずに同梱しています。",
      "  These files are owned by Microsoft Corporation and are included unmodified from the Visual Studio",
      "  redistributable files.",
      "",
    );
  }
  return `${lines.join("\n").replace(/\s+$/, "")}\n`;
}
