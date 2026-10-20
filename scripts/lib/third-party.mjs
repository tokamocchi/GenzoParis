// 配布物に入れる第三者のライセンスの文書（Rust の crate・画面の npm のパッケージ）を作る純粋な処理（DIST-05）。
// ファイルの読み書きと cargo-about の実行は scripts/lib/third-party-files.mjs、入口は scripts/third-party.mjs。
//
// - Rust: cargo-about（EmbarkStudios）の JSON（`cargo about generate --format json`）から作る。依存の木
//   （配布の対象の OS・機能フラグ・開発用の依存を除く）と、複数のライセンスから選べる crate のライセンスの選択
//   （about.toml の accepted の順）は cargo-about のもの。cargo-about は `LICENSE`・`LICENSE-*`・`LICENSE.*` の形の
//   ファイルしか見ないため、`LICENSE_MIT`（Tauri）・`license-mit`（windows-rs）などを取りこぼして SPDX の標準の文
//   （著作権の行のない雛形）にする。そのときは crate の直下のファイルを名前と中身で選んで補う（findLicenseTexts。
//   rustModel の doc）。補えなければ標準の文のまま載せ、「ライセンスのファイルがない crate」として著作権者の
//   手がかり（Cargo.toml の authors）を書く。
// - npm: ui/package-lock.json の、開発用でない（`dev` の印のない）パッケージ（ui/package.json の dependencies と、
//   その推移的な依存）。ライセンスは package.json の `license`（SPDX の式）を about.toml の accepted の順で選び、
//   本文はパッケージのフォルダのファイルから取る。パッケージに本文がなければ、上流のリポジトリの同じ版のファイルの
//   写し（NPM_SUPPLEMENTS。版と SHA-256 を固定）を使う。それもなければエラーにする（人が写しを足す）。
// - どちらも、NOTICE のファイル（Apache-2.0 の 4 (d) の帰属の表示など）があれば、ライセンスによらず載せる。
//
// どのライセンスに何が要るか（この内容で条件を満たすか、選んだライセンスでよいか）は、人（必要なら専門家）が
// 確かめる（docs/third_party.md の 1.4 節）。

import { createHash } from "node:crypto";

/** 使う cargo-about の版（ci.yml で入れる版と同じ。出力が版で変わりうるため、違う版では作らない）。 */
export const CARGO_ABOUT_VERSION = "0.9.2";
/** 文書を置くフォルダ（crates/genzo-app からの相対。リポジトリに置き、そのまま同梱する）。 */
export const THIRD_PARTY_SOURCE_DIR = "licenses/third-party";
/** Rust の crate の文書の名前。 */
export const RUST_NOTICES_FILE = "rust.txt";
/** 画面の npm のパッケージの文書の名前。 */
export const NPM_NOTICES_FILE = "npm.txt";
/**
 * 配布物の licenses に置く、リポジトリにある文書（ビルドの元（crates/genzo-app からの相対）→ 配布物の中の場所）。
 * tauri.conf.json の bundle.resources と同じ（scripts/lib/third-party.test.mjs で確かめる）。Windows はインストール先、
 * macOS は .app の Contents/Resources の下に入る。LibRaw の文書（licenses/LibRaw）は、ビルドのときに
 * scripts/lib/worker.mjs が足す（tauri.libs.conf.json。Tauri は設定の resources を合わせる）。
 */
export const STATIC_LICENSE_RESOURCES = {
  "licenses/README.txt": "licenses/README.txt",
  "../../LICENSE": "licenses/GenzoParis/LICENSE.txt",
  "licenses/third-party/rust.txt": "licenses/third-party/rust.txt",
  "licenses/third-party/npm.txt": "licenses/third-party/npm.txt",
};
/** 配布の対象（about.toml の targets と同じ。scripts/lib/third-party.test.mjs で確かめる）。 */
export const RUST_TARGETS = ["x86_64-pc-windows-msvc", "aarch64-apple-darwin"];

/**
 * 配布物に入る実行ファイルの crate と、配布のビルドで有効になる機能フラグ（repo からの相対）。
 * 本体は Tauri の CLI が custom-protocol を付けてビルドし、ワーカーは libraw を付けてビルドする
 * （scripts/lib/worker.mjs）。
 */
export const RUST_ROOTS = [
  { name: "genzo-paris", role: "本体", manifest: "crates/genzo-app/Cargo.toml", features: ["custom-protocol"] },
  { name: "genzo-worker", role: "ワーカー", manifest: "crates/genzo-worker/Cargo.toml", features: ["libraw"] },
];

/**
 * npm のパッケージに選んだライセンスの本文がないときに使う、上流のリポジトリの同じ版のファイルの写し
 * （THIRD_PARTY_SOURCE_DIR からの相対。改行を変えずに置く。.gitattributes）。`version` のパッケージにだけ使う
 * （版を上げたら、その版のタグのファイルを確かめて写しを足し直す）。
 *
 * @type {Record<string, Array<{ version: string, license: string, file: string, sha256: string, source: string }>>}
 */
export const NPM_SUPPLEMENTS = {
  // npm のパッケージには LICENSE.spdx しかない（package.json の files の "LICENSE" に当たるファイルがない）。
  // 上流の plugins/dialog/LICENSE_MIT（タグ dialog-js-v2.7.3 のコミット。LICENSE.spdx は npm のものと同じ内容）。
  "@tauri-apps/plugin-dialog": [
    {
      version: "2.7.3",
      license: "MIT",
      file: "supplements/npm/@tauri-apps/plugin-dialog/LICENSE_MIT",
      sha256: "89ff9689dcf9dd53968785d05a26f7898bb169dbfcada8d032b3e68cf0d55607",
      source:
        "https://github.com/tauri-apps/plugins-workspace/blob/6aa2854f314481a459be1189b02c65a2450789ab/plugins/dialog/LICENSE_MIT",
    },
  ],
};

// ---------------------------------------------------------------------------------------------------------------
// SPDX の式

/**
 * @typedef {{ license: string }
 *   | { and: SpdxNode[] }
 *   | { or: SpdxNode[] }} SpdxNode
 *   `license` は識別子（例外付きなら "Apache-2.0 WITH LLVM-exception" の形）
 */

/**
 * SPDX のライセンスの式を読む（AND・OR・WITH・括弧。大文字の演算子だけ）。読めなければ例外を投げる。
 *
 * @param {string} expr
 * @returns {SpdxNode}
 */
export function parseSpdx(expr) {
  const tokens = expr.match(/\(|\)|[^\s()]+/g) ?? [];
  let i = 0;
  const fail = (why) => {
    throw new Error(`SPDX の式が読めない（${why}）: ${expr}`);
  };
  const isOp = (t) => t === "AND" || t === "OR" || t === "WITH";
  const parseAtom = () => {
    const t = tokens[i++];
    if (t === undefined) fail("途中で終わっている");
    if (t === "(") {
      const node = parseOr();
      if (tokens[i++] !== ")") fail("括弧が閉じていない");
      return node;
    }
    if (t === ")" || isOp(t)) fail(`「${t}」の位置`);
    if (tokens[i] === "WITH") {
      i++;
      const exception = tokens[i++];
      if (exception === undefined || exception === "(" || exception === ")" || isOp(exception)) fail("WITH の後");
      return { license: `${t} WITH ${exception}` };
    }
    return { license: t };
  };
  const parseList = (op, parseItem) => {
    const items = [parseItem()];
    while (tokens[i] === op) {
      i++;
      items.push(parseItem());
    }
    return items.length === 1 ? items[0] : op === "AND" ? { and: items } : { or: items };
  };
  const parseAnd = () => parseList("AND", parseAtom);
  const parseOr = () => parseList("OR", parseAnd);
  const node = parseOr();
  if (i !== tokens.length) fail(`余分な「${tokens[i]}」`);
  return node;
}

/**
 * 式を満たすライセンスを、accepted の順（先のものを優先）で選ぶ。OR は、選ぶライセンスのうち最も後の順位が
 * 最も前になる方（同じなら数の少ない方、それも同じなら先に書かれた方）を選ぶ。満たせなければ null。
 *
 * @param {string} expr
 * @param {string[]} accepted 許可するライセンス（優先の順）
 * @returns {string[] | null} 選んだライセンス（式に出てくる順。重複なし）
 */
export function chooseLicenses(expr, accepted) {
  const rank = new Map(accepted.map((id, n) => [id, n]));
  /** @returns {{ ids: string[], worst: number } | null} */
  const visit = (node) => {
    if ("license" in node) {
      const r = rank.get(node.license);
      return r === undefined ? null : { ids: [node.license], worst: r };
    }
    const parts = ("and" in node ? node.and : node.or).map(visit);
    if ("and" in node) {
      if (parts.some((p) => p === null)) return null;
      return {
        ids: [...new Set(parts.flatMap((p) => p.ids))],
        worst: Math.max(...parts.map((p) => p.worst)),
      };
    }
    let best = null;
    for (const p of parts) {
      if (p === null) continue;
      if (best === null || p.worst < best.worst || (p.worst === best.worst && p.ids.length < best.ids.length)) best = p;
    }
    return best;
  };
  return visit(parseSpdx(expr))?.ids ?? null;
}

/**
 * about.toml の最上位の `accepted`（許可するライセンス。優先の順）を読む。
 *
 * @param {string} toml
 * @returns {string[]}
 */
export function parseAcceptedList(toml) {
  const m = /^accepted\s*=\s*\[([^\]]*)\]/m.exec(toml);
  if (!m) throw new Error("about.toml に accepted がない");
  const body = m[1]
    .split("\n")
    .map((l) => l.replace(/#.*$/, ""))
    .join("\n");
  return [...body.matchAll(/"([^"]+)"/g)].map((x) => x[1]);
}

/**
 * about.toml の最上位の `targets` を読む。
 *
 * @param {string} toml
 * @returns {string[]}
 */
export function parseTargets(toml) {
  const m = /^targets\s*=\s*\[([^\]]*)\]/m.exec(toml);
  return m ? [...m[1].replace(/#.*$/gm, "").matchAll(/"([^"]+)"/g)].map((x) => x[1]) : [];
}

// ---------------------------------------------------------------------------------------------------------------
// ライセンス・NOTICE のファイル

/** コード・データの拡張子（ライセンスの文書として扱わない）。LICENSE.spdx は SPDX の記述でライセンスの本文ではない。 */
const NOT_TEXT_EXT = /\.(rs|[cm]?js|ts|json|spdx|toml|ya?ml|html?|xml|sh|py|c|h|cc|cpp)$/i;

/** ライセンスの本文のファイルらしい名前か（LICENSE・LICENSE-MIT・LICENSE_APACHE-2.0・license-mit・COPYING など）。 */
export function isLicenseFileName(name) {
  return /^(licen[cs]e|copying|copyright|unlicense)([-_.].*)?$/i.test(name) && !NOT_TEXT_EXT.test(name);
}

/** NOTICE のファイルらしい名前か（NOTICE・NOTICE.txt・NOTICES.md など）。 */
export function isNoticeFileName(name) {
  return /^(notices?|third[-_]party[-_]notices?)([-_.].*)?$/i.test(name) && !NOT_TEXT_EXT.test(name);
}

/** ライセンスの識別子から、ファイル名に書かれる語（同じ系統のライセンスは同じ語）。 */
function licenseFamily(id) {
  const s = id.toLowerCase();
  const families = [
    ["mit", /^mit/],
    ["apache", /^apache/],
    ["bsd", /bsd/],
    ["zlib", /^zlib/],
    ["isc", /^isc$/],
    ["cc0", /^cc0/],
    ["unicode", /^unicode/],
    ["mpl", /^mpl/],
    ["bsl", /^bsl/],
    ["unlicense", /^unlicense/],
    ["lgpl", /^lgpl/],
    ["gpl", /^gpl/],
  ];
  return families.find(([, re]) => re.test(s))?.[0] ?? null;
}

/** ファイル名に書かれたライセンスの系統（LICENSE_APACHE-2.0 → apache。書かれていなければ空）。 */
function fileFamilies(name) {
  const words = name.toLowerCase().split(/[^a-z0-9]+/);
  const found = new Set();
  for (const w of words) {
    if (w === "mit") found.add("mit");
    else if (w === "apache") found.add("apache");
    else if (w === "bsd" || /^[0-4]?bsd$/.test(w)) found.add("bsd");
    else if (w === "zlib") found.add("zlib");
    else if (w === "isc") found.add("isc");
    else if (w === "cc0") found.add("cc0");
    else if (w === "unicode") found.add("unicode");
    else if (w === "mpl") found.add("mpl");
    else if (w === "bsl" || w === "boost") found.add("bsl");
    else if (w === "unlicense") found.add("unlicense");
    else if (w === "lgpl") found.add("lgpl");
    else if (w === "gpl") found.add("gpl");
  }
  return found;
}

/**
 * フォルダのファイルから、ライセンス `id` の本文のファイルの候補を選ぶ。名前にライセンスが書かれたもの
 * （LICENSE-MIT など）があればそれ、なければ名前にライセンスの書かれていないもの（LICENSE・COPYING など）。
 * 中身がそのライセンスの本文かは textMatchesLicense で確かめる。
 *
 * @param {string[]} names フォルダの直下のファイルの名前
 * @param {string} id
 * @returns {string[]} 名前の順
 */
export function pickLicenseFiles(names, id) {
  const family = licenseFamily(id);
  const candidates = names.filter(isLicenseFileName).sort(compareNatural);
  const named = candidates.filter((n) => family !== null && fileFamilies(n).has(family));
  if (named.length > 0) return named;
  return candidates.filter((n) => fileFamilies(n).size === 0);
}

/** ライセンスの系統ごとの、本文に必ずある文言（ファイル名だけで本文を選ばないため）。 */
const LICENSE_TEXT_MARKERS = {
  mit: /permission is hereby granted, free of charge/i,
  apache: /apache license/i,
  bsd: /redistribution and use in source and binary forms/i,
  zlib: /altered source versions must be plainly marked/i,
  isc: /permission to use, copy, modify, and(\/or)? distribute this software for any purpose/i,
  cc0: /cc0 1\.0 universal|creative commons legal code/i,
  unicode: /unicode/i,
  mpl: /mozilla public license/i,
  bsl: /boost software license/i,
  unlicense: /this is free and unencumbered software/i,
};

/**
 * 本文が、ライセンス `id` の本文らしいか（その系統の決まった文言を含むか。文言を決めていない系統は true）。
 *
 * @param {string} text
 * @param {string} id
 */
export function textMatchesLicense(text, id) {
  const family = licenseFamily(id);
  const marker = family === null ? undefined : LICENSE_TEXT_MARKERS[family];
  return marker === undefined || marker.test(text.replace(/\s+/g, " "));
}

/**
 * フォルダの直下のファイルから、ライセンス `id` の本文のファイルを選んで読む（名前で選び、中身で確かめる）。
 *
 * @param {string[]} names
 * @param {string} id
 * @param {(name: string) => string} read
 * @returns {Array<{ name: string, text: string }>}
 */
export function findLicenseTexts(names, id, read) {
  return pickLicenseFiles(names, id)
    .map((name) => ({ name, text: read(name) }))
    .filter((f) => textMatchesLicense(f.text, id));
}

// ---------------------------------------------------------------------------------------------------------------
// 一覧のもと（Rust・npm で共通の形）

/**
 * @typedef {{
 *   name: string, version: string, declared: string, chosen: string[],
 *   files: string[], standard: string[], supplemented: string[], authors: string[], repository: string | null,
 * }} NoticePackage
 *   `declared` は Cargo.toml・package.json のライセンスの表示、`chosen` は選んだライセンス、`files` は本文を
 *   取ったファイル（パッケージのフォルダからの相対）、`standard` は本文のファイルがなく標準の文を載せた
 *   ライセンス、`supplemented` は上流のファイルの写しで補ったライセンス
 * @typedef {{ license: string, notice: boolean, standard: boolean, text: string, users: string[] }} NoticeText
 *   `users` は使っているパッケージ（"名前 版"）
 * @typedef {{ packages: NoticePackage[], texts: NoticeText[], supplements: string[] }} NoticeModel
 *   `supplements` は補った写しの出典（文書に書く）
 */

/** 本文の改行を LF にそろえ、先頭の BOM と末尾の空白を除く（ほかは変えない）。 */
export function normalizeText(text) {
  return text.replace(/^﻿/, "").replace(/\r\n?/g, "\n").replace(/\s+$/, "");
}

/** 数字の部分を数として比べる（版の並び。ロケールによらない）。 */
export function compareNatural(a, b) {
  const pa = a.split(/(\d+)/);
  const pb = b.split(/(\d+)/);
  for (let k = 0; k < Math.max(pa.length, pb.length); k++) {
    const x = pa[k] ?? "";
    const y = pb[k] ?? "";
    if (x === y) continue;
    if (k % 2 === 1 && x !== "" && y !== "") {
      const d = Number(x) - Number(y);
      if (d !== 0) return d;
    }
    return x < y ? -1 : 1;
  }
  return 0;
}

const packageKey = (p) => `${p.name} ${p.version}`;
const comparePackages = (a, b) => compareNatural(a.name, b.name) || compareNatural(a.version, b.version);

/**
 * パッケージごとの本文から、同じ本文をまとめた一覧のもとを作る。
 *
 * @param {Array<{ pkg: NoticePackage, parts: Array<{ license: string, notice?: boolean, standard?: boolean, text: string }> }>} items
 * @param {string[]} [supplements]
 * @returns {NoticeModel}
 */
export function buildModel(items, supplements = []) {
  /** @type {Map<string, NoticeText>} */
  const byText = new Map();
  for (const { pkg, parts } of items) {
    for (const part of parts) {
      const text = normalizeText(part.text);
      const notice = part.notice === true;
      const standard = part.standard === true;
      // 標準の文は、同じ本文のファイルを持つパッケージとは分ける（見出しに標準の文と書くため）。
      const k = `${notice ? "N" : "L"}\u0000${standard ? "S" : "F"}\u0000${part.license}\u0000${text}`;
      let entry = byText.get(k);
      if (!entry) {
        entry = { license: part.license, notice, standard, text, users: [] };
        byText.set(k, entry);
      }
      const key = packageKey(pkg);
      if (!entry.users.includes(key)) entry.users.push(key);
    }
  }
  const texts = [...byText.values()];
  for (const t of texts) t.users.sort(compareNatural);
  texts.sort(
    (a, b) =>
      Number(a.notice) - Number(b.notice) ||
      compareNatural(a.license, b.license) ||
      Number(a.standard) - Number(b.standard) ||
      compareNatural(a.users[0], b.users[0]) ||
      (a.text < b.text ? -1 : a.text > b.text ? 1 : 0),
  );
  const packages = items.map((i) => i.pkg).sort(comparePackages);
  return { packages, texts, supplements: [...new Set(supplements)].sort() };
}

/**
 * @typedef {{ listDir: (dir: string) => string[], readText: (file: string) => string }} NoticeIo
 *   `listDir` はフォルダの直下のファイルの名前（フォルダは除く。なければ空）
 */

/** パスのフォルダ部分（区切りは / と \ のどちらでも）。 */
const dirOf = (p) => p.replace(/[\\/][^\\/]*$/, "");
const baseOf = (p) => p.replace(/^.*[\\/]/, "");
const joinPath = (dir, name) => `${dir}/${name}`;

/**
 * cargo-about の JSON（本体・ワーカーの分）から、Rust の crate の一覧のもとを作る。
 *
 * cargo-about は、ライセンスと本文が同じものを 1 つにまとめ、`source_path` には最初に入れた crate のファイル
 * （または、ファイルがなかったときの null）だけを残す。そのため crate ごとの本文の出典は次の順で決める。
 * 1. `source_path` がその crate のフォルダの中なら、そのファイル。
 * 2. でなければ、crate の直下のファイルから名前と中身で選んだもの（findLicenseTexts。cargo-about が取りこぼした
 *    `LICENSE_MIT` などもここで見つかる）。
 * 3. それもなければ、cargo-about の本文（SPDX の標準の文）をそのまま載せ、ファイルがない crate として印を付ける。
 *
 * @param {Array<{ licenses: Array<{ id: string, text: string, source_path: string | null, used_by: Array<{ crate: any }> }>, crates: Array<{ package: any, license: string }> }>} reports
 * @param {NoticeIo} io crate のフォルダのファイルを読む
 * @returns {NoticeModel}
 */
export function rustModel(reports, io) {
  /** @type {Map<string, { pkg: NoticePackage, dir: string, parts: Map<string, Array<{ text: string, file: string | null }>> }>} */
  const crates = new Map();
  for (const report of reports) {
    for (const c of report.crates) {
      if (crates.has(c.package.id)) continue;
      crates.set(c.package.id, {
        pkg: {
          name: c.package.name,
          version: c.package.version,
          declared: c.package.license ?? c.license ?? "",
          chosen: [],
          files: [],
          standard: [],
          supplemented: [],
          authors: c.package.authors ?? [],
          repository: c.package.repository ?? null,
        },
        dir: dirOf(c.package.manifest_path),
        parts: new Map(),
      });
    }
  }
  for (const report of reports) {
    for (const l of report.licenses) {
      for (const u of l.used_by) {
        const c = crates.get(u.crate.id);
        if (!c) throw new Error(`cargo-about の crates にない crate: ${u.crate.id}`);
        const list = c.parts.get(l.id) ?? [];
        const file = l.source_path ? relativeInside(c.dir, l.source_path) : null;
        const text = normalizeText(l.text);
        if (!list.some((x) => x.text === text && x.file === file)) list.push({ text, file });
        c.parts.set(l.id, list);
      }
    }
  }
  const items = [];
  for (const c of crates.values()) {
    if (c.parts.size === 0) throw new Error(`ライセンスの本文がない crate: ${packageKey(c.pkg)}`);
    const names = io.listDir(c.dir);
    const used = new Set();
    for (const list of c.parts.values()) for (const x of list) if (x.file) used.add(x.file);
    const parts = [];
    for (const [license, list] of [...c.parts.entries()].sort(([a], [b]) => compareNatural(a, b))) {
      c.pkg.chosen.push(license);
      const own = list.filter((x) => x.file !== null);
      if (own.length > 0) {
        for (const x of own) {
          parts.push({ license, text: x.text });
          c.pkg.files.push(x.file);
        }
        continue;
      }
      const found = findLicenseTexts(
        names.filter((n) => !used.has(n)),
        license,
        (n) => io.readText(joinPath(c.dir, n)),
      );
      if (found.length > 0) {
        for (const f of found) {
          parts.push({ license, text: f.text });
          c.pkg.files.push(f.name);
          used.add(f.name);
        }
      } else {
        for (const x of list) parts.push({ license, standard: true, text: x.text });
        c.pkg.standard.push(license);
      }
    }
    for (const n of names.filter((n) => isNoticeFileName(n) && !used.has(n)).sort(compareNatural)) {
      parts.push({ license: "NOTICE", notice: true, text: io.readText(joinPath(c.dir, n)) });
      c.pkg.files.push(n);
    }
    c.pkg.files = [...new Set(c.pkg.files)].sort(compareNatural);
    items.push({ pkg: c.pkg, parts });
  }
  return buildModel(items);
}

/** `file` の `dir` からの相対のパス（区切りは /）。`dir` の外なら null。 */
function relativeInside(dir, file) {
  const d = dir.replace(/\\/g, "/").replace(/\/$/, "");
  const f = file.replace(/\\/g, "/");
  return f.startsWith(`${d}/`) ? f.slice(d.length + 1) : null;
}

/**
 * ui/package-lock.json の、配布物に入りうるパッケージ（開発用の印のないもの）。
 *
 * @param {any} lock package-lock.json（lockfileVersion 2・3）
 * @returns {Array<{ path: string, name: string, version: string, license: string | null, optional: boolean }>}
 *   `path` は ui/ からの相対（node_modules/…）
 */
export function npmDistPackages(lock) {
  if (!lock || typeof lock.packages !== "object") {
    throw new Error("package-lock.json に packages がない（lockfileVersion 2 以降が要る）");
  }
  return Object.entries(lock.packages)
    .filter(([k, v]) => k !== "" && !v.dev && !v.devOptional && !v.link && !v.extraneous)
    .map(([k, v]) => ({
      path: k,
      name: v.name ?? k.slice(k.lastIndexOf("node_modules/") + "node_modules/".length),
      version: v.version,
      license: typeof v.license === "string" ? v.license : null,
      optional: v.optional === true,
    }))
    .sort(comparePackages);
}

/**
 * 画面の npm のパッケージの一覧のもとを作る。
 *
 * @param {ReturnType<typeof npmDistPackages>} packages
 * @param {string[]} accepted 許可するライセンス（優先の順。about.toml）
 * @param {NoticeIo & { readPackageJson: (dir: string) => any | null, readSupplement: (file: string) => string }} io
 *   `readPackageJson` はパッケージのフォルダ（`path`）の package.json（なければ null）、`readSupplement` は
 *   THIRD_PARTY_SOURCE_DIR からの相対のファイル
 * @param {typeof NPM_SUPPLEMENTS} [supplements]
 * @returns {NoticeModel}
 */
export function npmModel(packages, accepted, io, supplements = NPM_SUPPLEMENTS) {
  const items = [];
  const used = [];
  for (const p of packages) {
    const json = io.readPackageJson(p.path);
    if (json === null) {
      // 別の OS 向けの任意の依存（入っていない）。配布物を作る OS では入る可能性があるので、止める。
      throw new Error(`${p.path} がない（npm ci をしてから実行する。別の OS 向けの任意の依存なら、扱いを決める）`);
    }
    const declared = p.license ?? (typeof json.license === "string" ? json.license : null);
    if (!declared) throw new Error(`${p.name} ${p.version} のライセンスの表示（package.json の license）がない`);
    const chosen = chooseLicenses(declared, accepted);
    if (!chosen) throw new Error(`${p.name} ${p.version} のライセンス（${declared}）が about.toml の accepted で満たせない`);
    /** @type {NoticePackage} */
    const pkg = {
      name: p.name,
      version: p.version,
      declared,
      chosen,
      files: [],
      standard: [],
      supplemented: [],
      authors: authorsOf(json),
      repository: repositoryOf(json),
    };
    const names = io.listDir(p.path);
    const parts = [];
    for (const license of chosen) {
      const found = findLicenseTexts(names, license, (n) => io.readText(joinPath(p.path, n)));
      if (found.length > 0) {
        for (const f of found) {
          parts.push({ license, text: f.text });
          pkg.files.push(f.name);
        }
        continue;
      }
      const sup = (supplements[p.name] ?? []).filter((s) => s.license === license && s.version === p.version);
      if (sup.length === 0) {
        throw new Error(
          `${p.name} ${p.version} に ${license} の本文のファイルがない（${names.join(", ") || "ファイルなし"}）。` +
            "上流のリポジトリのこの版のファイルの写しを NPM_SUPPLEMENTS（scripts/lib/third-party.mjs）に足す",
        );
      }
      for (const s of sup) {
        const text = io.readSupplement(s.file);
        const actual = sha256Hex(text);
        if (actual !== s.sha256) throw new Error(`${s.file} の SHA-256 が違う（${actual}。期待: ${s.sha256}）`);
        parts.push({ license, text });
        pkg.supplemented.push(license);
        pkg.files.push(`${baseOf(s.file)}（上流 / upstream）`);
        used.push(`${packageKey(pkg)}（${license}）: ${s.source}`);
      }
    }
    for (const n of names.filter(isNoticeFileName).sort(compareNatural)) {
      parts.push({ license: "NOTICE", notice: true, text: io.readText(joinPath(p.path, n)) });
      pkg.files.push(n);
    }
    pkg.files = [...new Set(pkg.files)].sort(compareNatural);
    items.push({ pkg, parts });
  }
  return buildModel(items, used);
}

/** package.json の author（文字列か { name }）。 */
function authorsOf(json) {
  const a = json.author;
  const name = typeof a === "string" ? a : a && typeof a.name === "string" ? a.name : null;
  return name ? [name] : [];
}

function repositoryOf(json) {
  const r = json.repository;
  if (typeof r === "string") return r;
  if (r && typeof r.url === "string") return r.url;
  return null;
}

/**
 * Vite の build.license の JSON（バンドルに入ったパッケージ）のうち、一覧にないもの。
 *
 * @param {Array<{ name: string, version: string }>} bundled
 * @param {NoticeModel} model
 * @returns {string[]} "名前 版"
 */
export function missingFromNpmModel(bundled, model) {
  const listed = new Set(model.packages.map(packageKey));
  return [...new Set(bundled.map(packageKey))].filter((k) => !listed.has(k)).sort(compareNatural);
}

// ---------------------------------------------------------------------------------------------------------------
// 文書

/** 改行を LF にそろえた内容の SHA-256（Windows で改行が CRLF に変わっても同じ値にする）。 */
export function lockHash(text) {
  return sha256Hex(text.replace(/\r\n?/g, "\n"));
}

function sha256Hex(text) {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

/** 文書に書く、元にした lock のファイルの行。 */
export function lockLine(file, hash) {
  return `元にしたファイル / Source: ${file}（SHA-256 ${hash}）`;
}

/**
 * 文書から、元にした lock のファイルの SHA-256 を読む。なければ null。
 *
 * @param {string} notices
 * @param {string} file "Cargo.lock" など
 * @returns {string | null}
 */
export function readLockHash(notices, file) {
  const escaped = file.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const m = new RegExp(`^ *元にしたファイル / Source: ${escaped}（SHA-256 ([0-9a-f]{64})）$`, "m").exec(notices);
  return m ? m[1] : null;
}

const RULE = "=".repeat(80);
const THIN = "-".repeat(80);

/**
 * 文書の本文（UTF-8・改行は LF）。
 *
 * @param {NoticeModel} model
 * @param {{ kind: "rust" | "npm", lockFile: string, lockHash: string, facts: string[] }} meta
 *   `facts` は見出しの下に書く事実（対象・生成した道具など）
 * @returns {string}
 */
export function formatNotices(model, meta) {
  const rust = meta.kind === "rust";
  const what = rust ? "crate" : "パッケージ";
  // 日本語の文の中の英字の語（crate）の前後は空ける。
  const sp = rust ? " " : "";
  const whatEn = rust ? "crates" : "packages";
  const index = new Map(model.texts.map((t, n) => [t, n + 1]));
  const refsOf = (pkg) =>
    model.texts.filter((t) => t.users.includes(packageKey(pkg))).map((t) => `[${index.get(t)}]`);
  const lines = [
    rust
      ? "GenzoParis が使っている Rust のライブラリ（crate）のライセンス"
      : "GenzoParis の画面が使っている JavaScript のライブラリ（npm のパッケージ）のライセンス",
    rust
      ? "Licenses of the Rust libraries (crates) used in GenzoParis"
      : "Licenses of the JavaScript libraries (npm packages) used in the GenzoParis user interface",
    RULE,
    "",
    ...(rust
      ? [
          "GenzoParis の配布物（Windows・macOS）の本体（genzo-paris）とワーカー（genzo-worker）に組み込んだ Rust の",
          "ライブラリ（crate）の一覧と、そのライセンスの本文です。",
        ]
      : [
          "GenzoParis の画面（アプリに埋め込んだ HTML・JavaScript）に組み込んだ npm のパッケージの一覧と、その",
          "ライセンスの本文です。",
        ]),
    "複数のライセンスから選べるものは、GenzoParis が選んだライセンスの本文を載せています。NOTICE のファイルが",
    "あるものは、その内容も載せています。この文書は、scripts/third-party.mjs が自動で作ったものです。",
    rust
      ? "This document lists the Rust crates compiled into GenzoParis (genzo-paris and genzo-worker, for Windows and"
      : "This document lists the npm packages bundled into the GenzoParis user interface, with the full text of their",
    rust ? "macOS), with the full text of their licenses. Where a crate offers a choice of licenses, the license chosen" : "licenses. Where a package offers a choice of licenses, the license chosen for GenzoParis is included. NOTICE",
    rust ? "for GenzoParis is included. NOTICE files are included as well. Generated by scripts/third-party.mjs." : "files are included as well. Generated by scripts/third-party.mjs.",
    "",
    `  ${lockLine(meta.lockFile, meta.lockHash)}`,
    ...meta.facts.map((f) => `  ${f}`),
    `  ${what}${sp}の数 / Number of ${whatEn}: ${model.packages.length}`,
    `  本文の数 / Number of texts: ${model.texts.length}`,
    "",
    `1. ${what}${sp}の一覧 / List of ${whatEn}`,
    THIN,
    `  名前 版: ライセンスの表示 → 選んだライセンス 本文の番号（本文を取ったファイル）`,
    `  name version: declared license → chosen license, text number (license files)`,
    "",
    ...model.packages.map((p) => {
      const chosen = p.chosen.join(" AND ");
      const sameAsDeclared = chosen === p.declared;
      const files = p.files.length > 0 ? `（${p.files.join(", ")}）` : "";
      return `  ${p.name} ${p.version}: ${sameAsDeclared ? chosen : `${p.declared} → ${chosen}`} ${refsOf(p).join("")}${files}`;
    }),
    "",
    `2. ライセンスの本文 / License texts`,
    ...model.texts.flatMap((t) => {
      const n = index.get(t);
      const title = t.notice ? "NOTICE" : t.license;
      return [
        RULE,
        `[${n}] ${title}${t.standard ? "（SPDX の標準の文。ライセンスのファイルがないため / standard SPDX text; no license file）" : ""}`,
        `使っている${sp}${what} / Used by: ${t.users.join(", ")}`,
        THIN,
        t.text,
        "",
      ];
    }),
  ];
  const standard = model.packages.filter((p) => p.standard.length > 0);
  if (standard.length > 0) {
    lines.push(
      RULE,
      `3. ライセンスのファイルがない${sp}${what} / ${rust ? "Crates" : "Packages"} without a license file`,
      THIN,
      `次の${sp}${what}${sp}は、公開された${sp}${what}${sp}にライセンスの本文のファイルがありません。2 章には SPDX の標準の文を載せて`,
      "います。著作権者の手がかりとして、作者（authors）とリポジトリを書きます。",
      `The following ${whatEn} do not include a license file in their published package. The standard SPDX text is`,
      "included in section 2. The authors and the repository are listed for reference.",
      "",
      ...standard.flatMap((p) => [
        `  ${p.name} ${p.version}: ${p.standard.join(", ")}`,
        `    authors: ${p.authors.length > 0 ? p.authors.join(", ") : "（記載なし / not specified）"}`,
        ...(p.repository ? [`    repository: ${p.repository}`] : []),
      ]),
      "",
    );
  }
  if (model.supplements.length > 0) {
    lines.push(
      RULE,
      `${standard.length > 0 ? 4 : 3}. 上流のリポジトリから補った本文 / License texts taken from the upstream repositories`,
      THIN,
      `次の${sp}${what}${sp}は、配布物にライセンスの本文のファイルがないため、上流のリポジトリの同じ版のファイルを載せています。`,
      `The following ${whatEn} do not include the license text in their published package; the file from the upstream`,
      "repository at the corresponding version is included instead.",
      "",
      ...model.supplements.map((s) => `  ${s}`),
      "",
    );
  }
  return `${lines.join("\n").replace(/\s+$/, "")}\n`;
}
