// macOS の実行ファイル・dylib（Mach-O）の依存をたどり、.app に同梱するための書き換えの計画を作る（純粋な処理）。
//
// 配布用のビルドで、ワーカー（genzo-worker）が読み込む Homebrew の LibRaw（libraw_r.*.dylib）と、その依存
// （libomp・little-cms2・jpeg-turbo など。Homebrew の libraw の formula の依存）を集め、.app の
// Contents/Frameworks に置くために使う（scripts/prepare-worker.mjs）。
//
// - 依存は `otool -l` の出力（Load command の一覧）から読む。otool・install_name_tool・codesign は
//   Xcode の Command Line Tools に含まれる。
// - .app の中の配置: ワーカーは Contents/MacOS/genzo-worker（Tauri の externalBin）、dylib は
//   Contents/Frameworks/<名前>（Tauri の bundle.macOS.frameworks）。ワーカーからは
//   `@executable_path/../Frameworks/<名前>`、dylib どうしは `@loader_path/<名前>` で参照するように書き換える。
//
// **macOS の実機で未確認**（この開発環境は Linux。otool の出力の形は Apple の cctools の形に合わせて書き、
// テストは出力の例で確かめる）。

import path from "node:path";

/** 依存の dylib を表す Load command（LC_LOAD_DYLIB など）。 */
const DYLIB_LOAD_COMMANDS = new Set([
  "LC_LOAD_DYLIB",
  "LC_LOAD_WEAK_DYLIB",
  "LC_REEXPORT_DYLIB",
  "LC_LAZY_LOAD_DYLIB",
  "LC_LOAD_UPWARD_DYLIB",
]);

/** ワーカーから dylib を参照するときの前置き（Contents/MacOS から見た Contents/Frameworks）。 */
export const WORKER_DYLIB_PREFIX = "@executable_path/../Frameworks/";
/** dylib どうしの参照の前置き（同じ Contents/Frameworks にある）。 */
export const SIBLING_DYLIB_PREFIX = "@loader_path/";

/**
 * `otool -l <file>` の出力を読む。
 *
 * @param {string} text otool の標準出力
 * @returns {{ id: string | null, deps: string[], rpaths: string[], minos: string | null }}
 *   `deps` は依存の dylib の参照（書かれたとおりの文字列。順番どおり）。`minos` は動く macOS の最小の版
 *   （LC_BUILD_VERSION の minos、なければ LC_VERSION_MIN_MACOSX の version）。
 */
export function parseOtoolLoadCommands(text) {
  const result = { id: null, deps: [], rpaths: [], minos: null };
  /** @type {{ cmd?: string, name?: string, path?: string, minos?: string, version?: string } | null} */
  let block = null;
  const flush = () => {
    if (!block || !block.cmd) return;
    // 複数のアーキテクチャを含むファイル（fat）では同じ参照が繰り返し出るので、1 つにまとめる
    // （install_name_tool に同じ参照の -change を 2 回渡さないため）。
    if (DYLIB_LOAD_COMMANDS.has(block.cmd) && block.name) {
      if (!result.deps.includes(block.name)) result.deps.push(block.name);
    } else if (block.cmd === "LC_ID_DYLIB" && block.name) {
      result.id = block.name;
    } else if (block.cmd === "LC_RPATH" && block.path) {
      if (!result.rpaths.includes(block.path)) result.rpaths.push(block.path);
    } else if (block.cmd === "LC_BUILD_VERSION" && block.minos) {
      result.minos = block.minos;
    } else if (block.cmd === "LC_VERSION_MIN_MACOSX" && block.version && result.minos === null) {
      result.minos = block.version;
    }
  };
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (/^Load command \d+$/.test(line)) {
      flush();
      block = {};
      continue;
    }
    if (!block) continue;
    let m;
    if ((m = /^cmd (\S+)$/.exec(line))) {
      block.cmd = m[1];
    } else if ((m = /^name (.+) \(offset \d+\)$/.exec(line))) {
      block.name = m[1];
    } else if ((m = /^path (.+) \(offset \d+\)$/.exec(line))) {
      block.path = m[1];
    } else if ((m = /^minos (\S+)$/.exec(line))) {
      block.minos = m[1];
    } else if ((m = /^version (\S+)$/.exec(line)) && block.cmd === "LC_VERSION_MIN_MACOSX") {
      block.version = m[1];
    }
  }
  flush();
  return result;
}

/** macOS の OS が持っているライブラリか（同梱しない）。 */
export function isSystemLibrary(ref) {
  return ref.startsWith("/usr/lib/") || ref.startsWith("/System/");
}

/**
 * 参照（`@rpath/...` など）を、ファイルの場所に直す候補を返す（先頭から順に、あるものを使う）。
 *
 * @param {string} ref 依存の参照
 * @param {{ loaderPath: string, executablePath: string, rpaths: string[] }} ctx
 *   `loaderPath` は参照を持つファイルの場所、`executablePath` はワーカーの場所、`rpaths` は参照を持つ
 *   ファイルとワーカーの LC_RPATH（dyld は読み込んだ側の連鎖の rpath を順に探す）。
 * @returns {string[]}
 */
export function candidatePaths(ref, ctx) {
  const loaderDir = path.posix.dirname(ctx.loaderPath);
  const exeDir = path.posix.dirname(ctx.executablePath);
  const expand = (p) =>
    p.startsWith("@loader_path/")
      ? path.posix.join(loaderDir, p.slice("@loader_path/".length))
      : p.startsWith("@executable_path/")
        ? path.posix.join(exeDir, p.slice("@executable_path/".length))
        : p;
  if (ref.startsWith("@rpath/")) {
    const rest = ref.slice("@rpath/".length);
    return ctx.rpaths.map((r) => path.posix.join(expand(r), rest));
  }
  return [expand(ref)];
}

/**
 * 同梱する dylib の名前の衝突を確かめつつ、実体（シンボリックリンクをたどった場所）ごとに名前を決める。
 *
 * @param {Map<string, string>} names 実体の場所 → 同梱の名前（呼び出し側が持つ。この関数が足す）
 * @param {string} realPath 実体の場所
 * @param {string} ref 参照（名前は参照の最後の部分。例: libraw_r.23.dylib）
 * @returns {string} 同梱の名前
 */
export function assignBundleName(names, realPath, ref) {
  const existing = names.get(realPath);
  if (existing) return existing;
  const name = path.posix.basename(ref);
  for (const [other, n] of names) {
    if (n === name && other !== realPath) {
      throw new Error(`同梱する dylib の名前が重なる: ${name}（${other} と ${realPath}）`);
    }
  }
  names.set(realPath, name);
  return name;
}

/**
 * install_name_tool の引数を作る。
 *
 * @param {{ id?: string | null, changes: Array<[string, string]> }} plan
 *   `id` は dylib の新しい識別名（実行ファイルなら省く）、`changes` は [古い参照, 新しい参照]
 * @param {string} file 書き換えるファイル
 * @returns {string[] | null} 書き換えがなければ null
 */
export function installNameToolArgs(plan, file) {
  const args = [];
  if (plan.id) args.push("-id", plan.id);
  for (const [from, to] of plan.changes) {
    if (from !== to) args.push("-change", from, to);
  }
  if (args.length === 0) return null;
  return [...args, file];
}

/**
 * 書き換えた後のファイルに、同梱の外（Homebrew の場所など）への参照が残っていないかを確かめる。
 *
 * @param {string[]} deps 書き換えた後の依存（parseOtoolLoadCommands の deps）
 * @returns {string[]} 残っている参照
 */
export function unbundledReferences(deps) {
  return deps.filter(
    (d) =>
      !isSystemLibrary(d) &&
      !d.startsWith(WORKER_DYLIB_PREFIX) &&
      !d.startsWith(SIBLING_DYLIB_PREFIX),
  );
}

/**
 * ワーカーから依存をたどり、同梱する dylib と書き換えの計画を作る。
 *
 * @param {string} workerPath ワーカーの実行ファイル（ビルドした場所）
 * @param {{
 *   loadCommands: (file: string) => ReturnType<typeof parseOtoolLoadCommands>,
 *   realpath: (file: string) => string,
 *   exists: (file: string) => boolean,
 * }} io ファイルを読む処理（テストでは差し替える）
 * @returns {{
 *   worker: { changes: Array<[string, string]>, minos: string | null },
 *   libs: Array<{ name: string, source: string, id: string, changes: Array<[string, string]>, minos: string | null }>,
 * }} `source` は実体の場所（シンボリックリンクをたどった場所）
 */
export function planMacosBundle(workerPath, io) {
  const workerInfo = io.loadCommands(workerPath);
  /** 実体の場所 → 同梱の名前 */
  const names = new Map();
  /** 同梱の名前 → 計画 */
  const libs = new Map();
  const queue = [];

  const resolve = (ref, loaderPaths, loaderRpaths) => {
    const tried = [];
    for (const loaderPath of loaderPaths) {
      const cands = candidatePaths(ref, {
        loaderPath,
        executablePath: workerPath,
        rpaths: [...loaderRpaths, ...workerInfo.rpaths],
      });
      for (const c of cands) {
        if (io.exists(c)) return c;
        tried.push(c);
      }
    }
    throw new Error(
      `依存の dylib が見つからない: ${ref}（${loaderPaths[0]} から。探した場所: ${tried.join(", ") || "なし"}）`,
    );
  };

  const visit = (ref, loaderPaths, loaderRpaths) => {
    const found = resolve(ref, loaderPaths, loaderRpaths);
    const real = io.realpath(found);
    const isNew = !names.has(real);
    const name = assignBundleName(names, real, ref);
    if (isNew) {
      libs.set(name, { name, source: real, loadPath: found, id: `@rpath/${name}`, changes: [], minos: null });
      queue.push(name);
    }
    return name;
  };

  const workerChanges = [];
  for (const ref of workerInfo.deps) {
    if (isSystemLibrary(ref)) continue;
    const name = visit(ref, [workerPath], workerInfo.rpaths);
    workerChanges.push([ref, WORKER_DYLIB_PREFIX + name]);
  }
  while (queue.length > 0) {
    const lib = libs.get(queue.shift());
    const info = io.loadCommands(lib.source);
    lib.minos = info.minos;
    // @loader_path は、読み込んだときの場所（シンボリックリンクの場所）と実体の場所の両方で探す。
    const loaderPaths = lib.loadPath === lib.source ? [lib.source] : [lib.loadPath, lib.source];
    for (const ref of info.deps) {
      if (isSystemLibrary(ref)) continue;
      const depName = visit(ref, loaderPaths, info.rpaths);
      lib.changes.push([ref, SIBLING_DYLIB_PREFIX + depName]);
    }
  }
  return {
    worker: { changes: workerChanges, minos: workerInfo.minos },
    libs: [...libs.values()].map(({ loadPath: _loadPath, ...rest }) => rest),
  };
}

/**
 * macOS の版（"12.0"・"14" など）を比べる。a が b より新しければ正、同じなら 0、古ければ負。
 */
export function compareMacosVersions(a, b) {
  const pa = a.split(".").map((x) => Number.parseInt(x, 10) || 0);
  const pb = b.split(".").map((x) => Number.parseInt(x, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

/**
 * 同梱する Mach-O（ワーカーと dylib）の最小の版（minos）を、.app の最小の版（tauri.conf.json の
 * bundle.macOS.minimumSystemVersion）と比べる。
 *
 * Homebrew の dylib は、ビルドした Mac の macOS の版向けの bottle なので、minos が .app の最小の版より
 * 新しくなりうる。その場合、古い macOS ではアプリは起動するのにワーカーが起動せず、RAW を開けない。
 * 配る相手の macOS の版は人の判断なので、既定ではエラーにする。明示的に許すとき（allowNewer）は通し、
 * .app の最小の版を同梱したものの minos の最大に上げる（古い macOS では、起動できないことを OS が知らせる）。
 *
 * @param {Array<{ name: string, minos: string | null }>} items 同梱するもの（minos が読み取れなければ null）
 * @param {string} minimum .app の最小の版（tauri.conf.json）
 * @param {boolean} allowNewer 新しいものがあっても通すか
 * @returns {{
 *   items: Array<{ name: string, minos: string | null }>,
 *   newest: string | null,
 *   newer: Array<{ name: string, minos: string }>,
 *   unknown: string[],
 *   error: string | null,
 *   minimumSystemVersion: string,
 * }} `newest` は minos の最大（読み取れないものを除く）、`newer` は minimum より新しいもの、`unknown` は
 *   minos が読み取れなかったものの名前、`minimumSystemVersion` は .app に書く最小の版
 */
export function checkMacosMinimum(items, minimum, allowNewer) {
  let newest = null;
  for (const { minos } of items) {
    if (minos && (newest === null || compareMacosVersions(minos, newest) > 0)) newest = minos;
  }
  const newer = items
    .filter((i) => i.minos && compareMacosVersions(i.minos, minimum) > 0)
    .map((i) => ({ name: i.name, minos: /** @type {string} */ (i.minos) }));
  const unknown = items.filter((i) => !i.minos).map((i) => i.name);
  const base = { items, newest, newer, unknown };
  if (newer.length === 0 || newest === null) {
    return { ...base, error: null, minimumSystemVersion: minimum };
  }
  if (!allowNewer) {
    return {
      ...base,
      error:
        `同梱するものが、.app の最小の版（tauri.conf.json の minimumSystemVersion: ${minimum}）より新しい macOS 向け` +
        `にビルドされている: ${newer.map((n) => `${n.name}（minos ${n.minos}）`).join(", ")}。このままでは macOS ` +
        `${newest} より古い Mac で RAW を開けない。配る相手の Mac がすべて macOS ${newest} 以降なら、環境変数 ` +
        "GENZO_ALLOW_NEWER_MINOS=1 を付けて実行する（.app の最小の版を上げる）。そうでなければ、LibRaw と依存を" +
        ` MACOSX_DEPLOYMENT_TARGET=${minimum} でビルドしたものを使う（手順は未整備。docs/packaging.md の 2.3）`,
      minimumSystemVersion: minimum,
    };
  }
  return { ...base, error: null, minimumSystemVersion: newest };
}
