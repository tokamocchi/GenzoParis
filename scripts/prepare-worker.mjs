#!/usr/bin/env node
// 配布用のワーカー（genzo-worker。LibRaw あり）を release でビルドし、Tauri が同梱する場所に置く。
//
//   node scripts/prepare-worker.mjs
//
// 1. `cargo build --release -p genzo-worker --features libraw --locked` でワーカーをビルドする
//    （本体 genzo-paris は LibRaw に依存させない。決定事項ログ No.32。Cargo の機能フラグは同じビルドの中で
//    共通になるため、本体とは別の cargo の実行でビルドする）。
// 2. crates/genzo-app/binaries/genzo-worker-<target triple>（Windows は .exe 付き）に置く
//    （Tauri の externalBin の決まり。tauri.bundle.conf.json）。
// 3. LibRaw の共有ライブラリとその依存を crates/genzo-app/binaries/libs に集め、Tauri に渡す追加の設定
//    crates/genzo-app/binaries/tauri.libs.conf.json を作る。
//    - Windows: ワーカーの読み込む DLL を PE のヘッダからたどり、LibRaw のフォルダ（vcpkg なら
//      installed/<triplet>/bin）にある DLL を集める。インストール先のフォルダ（ワーカーと同じ場所）に置く。
//    - macOS: `otool -l` で依存をたどり、Homebrew の dylib を集める。install_name_tool で参照を
//      `@executable_path/../Frameworks/...`（ワーカー）・`@loader_path/...`（dylib どうし）に書き換え、
//      codesign で署名し直す（アドホック。書き換えると署名が壊れ、Apple Silicon では動かなくなるため）。
//      .app の Contents/Frameworks に置く。
//    - Linux: 同梱しない（確認用。OS のパッケージの LibRaw（apt の libraw-dev）を使う）。
// 4. 同梱したファイルの一覧（元の場所・SHA-256）を crates/genzo-app/binaries/bundled-libs.txt に書く
//    （LibRaw の LGPL の扱いを人が確かめるときのため。docs/packaging.md）。
//
// LibRaw の場所:
// - Windows: 環境変数 LIBRAW_INCLUDE_DIR・LIBRAW_LIB_DIR（genzo-raw の build.rs）。なければ
//   VCPKG_ROOT（なければ VCPKG_INSTALLATION_ROOT。GitHub Actions の Windows のランナーにある）の
//   installed/<VCPKG_DEFAULT_TRIPLET、既定は x64-windows> を使う。LIBRAW_LIB_NAME がなければ、.lib の
//   ファイル名から選ぶ（vcpkg は raw_r）。DLL は LIBRAW_DLL_DIR、なければ LIBRAW_LIB_DIR の隣の bin と
//   LIBRAW_LIB_DIR で探す。
// - macOS・Linux: pkg-config（Homebrew の libraw、apt の libraw-dev）。
//
// Windows・macOS の実機では未確認（この開発環境は Linux）。docs/packaging.md の「未確認の事項」。

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { LIBS_DIR, chooseLibrawLibName, libsTauriConfig, planWindowsDlls } from "./lib/bundle.mjs";
import {
  compareMacosVersions,
  installNameToolArgs,
  parseOtoolLoadCommands,
  planMacosBundle,
  unbundledReferences,
} from "./lib/macho.mjs";
import { readPeImports } from "./lib/pe.mjs";
import { APP_DIR, BINARIES_DIR, LIBS_CONFIG, REPO_ROOT, capture, run } from "./lib/proc.mjs";
import { parseHostTriple, sidecarFileName, targetOs } from "./lib/target.mjs";

/** ワーカーの実行ファイルの名前（genzo-worker の WORKER_EXE_NAME。Tauri の externalBin の名前と同じ）。 */
const WORKER_NAME = "genzo-worker";
/** 同梱したファイルの一覧。 */
const MANIFEST_FILE = path.join(BINARIES_DIR, "bundled-libs.txt");

/**
 * ワーカーをビルドして置き、LibRaw のライブラリを集める。
 *
 * @returns {{ triple: string, os: string, sidecar: string, libs: string[] }}
 */
export function prepareWorker() {
  const triple = parseHostTriple(capture("rustc", ["-vV"], { cwd: REPO_ROOT }));
  const os = targetOs(triple);
  const exe = os === "windows" ? ".exe" : "";
  console.log(`対象: ${triple}`);

  const env = { ...process.env };
  const dllDirs = os === "windows" ? setupWindowsLibraw(env) : [];

  // ビルドする。何も変わっていなくても実行する: cargo は target/release/genzo-worker を deps の中の
  // ビルド結果から置き直すので、前回の Tauri のビルドが置いたもの（macOS では書き換えたもの）を使わずに済む。
  const cargoCommon = ["--release", "--locked", "-p", "genzo-worker", "--features", "libraw"];
  if (os === "macos") {
    // install_name_tool で参照を書き換える余白を、ヘッダに確保する（参照が長くなっても収まるように）。
    run("cargo", ["rustc", ...cargoCommon, "--bin", WORKER_NAME, "--", "-C", "link-arg=-Wl,-headerpad_max_install_names"], {
      cwd: REPO_ROOT,
      env,
    });
  } else {
    run("cargo", ["build", ...cargoCommon], { cwd: REPO_ROOT, env });
  }

  const metadata = JSON.parse(capture("cargo", ["metadata", "--format-version", "1", "--no-deps"], { cwd: REPO_ROOT, env }));
  const built = path.join(metadata.target_directory, "release", `${WORKER_NAME}${exe}`);
  if (!fs.existsSync(built)) throw new Error(`ワーカーのビルド結果がない: ${built}`);

  fs.mkdirSync(BINARIES_DIR, { recursive: true });
  const libsDir = path.join(APP_DIR, LIBS_DIR);
  fs.rmSync(libsDir, { recursive: true, force: true });
  fs.mkdirSync(libsDir, { recursive: true });

  const sidecar = path.join(BINARIES_DIR, sidecarFileName(WORKER_NAME, triple));
  fs.rmSync(sidecar, { force: true });
  fs.copyFileSync(built, sidecar);
  fs.chmodSync(sidecar, 0o755);
  console.log(`ワーカー: ${path.relative(REPO_ROOT, sidecar)}`);

  /** @type {Array<{ name: string, source: string }>} */
  let bundled = [];
  if (os === "windows") {
    bundled = collectWindowsDlls(sidecar, dllDirs, libsDir);
  } else if (os === "macos") {
    bundled = collectMacosDylibs(built, sidecar, libsDir);
  } else {
    reportLinuxLibraw(sidecar);
  }

  const config = libsTauriConfig(
    os,
    bundled.map((b) => b.name),
  );
  fs.writeFileSync(LIBS_CONFIG, `${JSON.stringify(config, null, 2)}\n`);
  console.log(`Tauri の追加の設定: ${path.relative(REPO_ROOT, LIBS_CONFIG)}`);
  writeManifest(triple, sidecar, bundled, libsDir);
  return { triple, os, sidecar, libs: bundled.map((b) => b.name) };
}

/** Windows: LibRaw の場所を環境変数に入れ、DLL を探すフォルダを返す。 */
function setupWindowsLibraw(env) {
  if (!env.LIBRAW_INCLUDE_DIR || !env.LIBRAW_LIB_DIR) {
    const vcpkgRoot = env.VCPKG_ROOT || env.VCPKG_INSTALLATION_ROOT;
    if (!vcpkgRoot) {
      throw new Error(
        "LibRaw の場所が分からない。環境変数 LIBRAW_INCLUDE_DIR・LIBRAW_LIB_DIR か、VCPKG_ROOT を設定する（docs/packaging.md）",
      );
    }
    const installed = path.join(vcpkgRoot, "installed", env.VCPKG_DEFAULT_TRIPLET || "x64-windows");
    env.LIBRAW_INCLUDE_DIR ||= path.join(installed, "include");
    env.LIBRAW_LIB_DIR ||= path.join(installed, "lib");
  }
  if (!fs.existsSync(env.LIBRAW_LIB_DIR)) {
    throw new Error(`LIBRAW_LIB_DIR のフォルダがない: ${env.LIBRAW_LIB_DIR}`);
  }
  if (!env.LIBRAW_LIB_NAME) {
    const name = chooseLibrawLibName(fs.readdirSync(env.LIBRAW_LIB_DIR));
    if (!name) throw new Error(`LibRaw の .lib が見つからない: ${env.LIBRAW_LIB_DIR}`);
    env.LIBRAW_LIB_NAME = name;
  }
  // pkg-config で別の LibRaw を拾わないようにする（Windows は環境変数での指定を使う。genzo-raw の build.rs）。
  env.LIBRAW_R_NO_PKG_CONFIG ||= "1";
  env.LIBRAW_NO_PKG_CONFIG ||= "1";
  console.log(
    `LibRaw: LIBRAW_INCLUDE_DIR=${env.LIBRAW_INCLUDE_DIR} LIBRAW_LIB_DIR=${env.LIBRAW_LIB_DIR} LIBRAW_LIB_NAME=${env.LIBRAW_LIB_NAME}`,
  );
  return env.LIBRAW_DLL_DIR
    ? [env.LIBRAW_DLL_DIR]
    : [path.join(env.LIBRAW_LIB_DIR, "..", "bin"), env.LIBRAW_LIB_DIR];
}

/** Windows: ワーカーの読み込む DLL をたどり、LibRaw のフォルダにあるものを集める。 */
function collectWindowsDlls(worker, dllDirs, libsDir) {
  const index = new Map();
  for (const dir of dllDirs) {
    if (!fs.existsSync(dir)) continue;
    for (const f of fs.readdirSync(dir)) {
      const key = f.toLowerCase();
      if (key.endsWith(".dll") && !index.has(key)) index.set(key, path.join(dir, f));
    }
  }
  const readImports = (file) => readPeImports(new Uint8Array(fs.readFileSync(file)));
  const plan = planWindowsDlls(readImports(worker), {
    findDll: (name) => index.get(name.toLowerCase()) ?? null,
    readImports,
  });
  if (!plan.bundled.some((b) => /raw/i.test(b.name))) {
    throw new Error(
      `ワーカーが LibRaw の DLL を読み込んでいない、または DLL が見つからない（探した場所: ${dllDirs.join(", ")}。` +
        `見つからなかった DLL: ${plan.notFound.join(", ") || "なし"}）`,
    );
  }
  for (const b of plan.bundled) {
    fs.copyFileSync(b.source, path.join(libsDir, b.name));
    console.log(`同梱: ${b.name} ← ${b.source}`);
  }
  if (plan.vcRuntime.length > 0) {
    console.log(
      `注意: Visual C++ のランタイム（${plan.vcRuntime.join(", ")}）は同梱しない。利用者の PC に` +
        "「Microsoft Visual C++ 再頒布可能パッケージ（x64）」が要る（docs/packaging.md）",
    );
  }
  if (plan.notFound.length > 0) {
    console.warn(
      `警告: 次の DLL は LibRaw のフォルダになく、OS の DLL の一覧にもない。OS の DLL でなければ同梱が要る: ${plan.notFound.join(", ")}`,
    );
  }
  return plan.bundled;
}

/** macOS: Homebrew の dylib を集め、参照を書き換えて署名し直す。 */
function collectMacosDylibs(built, sidecar, libsDir) {
  const otool = (file) => parseOtoolLoadCommands(capture("otool", ["-l", file]));
  const plan = planMacosBundle(built, {
    loadCommands: otool,
    realpath: (f) => fs.realpathSync(f),
    exists: (f) => fs.existsSync(f),
  });
  if (!plan.libs.some((l) => /raw/i.test(l.name))) {
    throw new Error("ワーカーが LibRaw の dylib を読み込んでいない（libraw の機能フラグでビルドされているか確かめる）");
  }
  const outputs = [];
  for (const lib of plan.libs) {
    const dest = path.join(libsDir, lib.name);
    fs.copyFileSync(lib.source, dest);
    // Homebrew のファイルは書き込みできない（0444）ことがあるので、書き換えられるようにする。
    fs.chmodSync(dest, 0o644);
    const args = installNameToolArgs({ id: lib.id, changes: lib.changes }, dest);
    if (args) run("install_name_tool", args);
    outputs.push(dest);
    console.log(`同梱: ${lib.name} ← ${lib.source}（minos ${lib.minos ?? "不明"}）`);
  }
  const workerArgs = installNameToolArgs({ changes: plan.worker.changes }, sidecar);
  if (workerArgs) run("install_name_tool", workerArgs);
  outputs.push(sidecar);

  // 書き換えで壊れた署名を、アドホック署名で付け直す（Tauri も bundle.macOS.signingIdentity "-" で .app 全体を
  // 署名し直すが、.app に入れる前のファイルも動くようにする）。
  for (const file of outputs) run("codesign", ["--force", "--sign", "-", file]);

  for (const file of outputs) {
    const left = unbundledReferences(otool(file).deps);
    if (left.length > 0) {
      throw new Error(`同梱の外への参照が残っている: ${file}: ${left.join(", ")}`);
    }
  }

  const minimum = readMinimumMacosVersion();
  for (const lib of plan.libs) {
    if (minimum && lib.minos && compareMacosVersions(lib.minos, minimum) > 0) {
      console.warn(
        `警告: ${lib.name} は macOS ${lib.minos} 以降向けにビルドされている（tauri.conf.json の` +
          ` minimumSystemVersion は ${minimum}）。それより古い macOS では RAW を開けない（docs/packaging.md）`,
      );
    }
  }
  return plan.libs.map((l) => ({ name: l.name, source: l.source }));
}

/** Linux: 同梱しない。ワーカーが LibRaw に依存していることだけ表示する（確認用）。 */
function reportLinuxLibraw(sidecar) {
  try {
    const lines = capture("ldd", [sidecar])
      .split("\n")
      .filter((l) => /raw/i.test(l))
      .map((l) => l.trim());
    console.log(`LibRaw は同梱しない（Linux は確認用。OS のパッケージを使う）: ${lines.join("; ") || "LibRaw への依存がない"}`);
  } catch {
    console.log("LibRaw は同梱しない（Linux は確認用。OS のパッケージを使う）");
  }
}

/** tauri.conf.json の bundle.macOS.minimumSystemVersion を読む。 */
function readMinimumMacosVersion() {
  try {
    const conf = JSON.parse(fs.readFileSync(path.join(APP_DIR, "tauri.conf.json"), "utf8"));
    return conf?.bundle?.macOS?.minimumSystemVersion ?? null;
  } catch {
    return null;
  }
}

/** 同梱したファイルの一覧を書く。 */
function writeManifest(triple, sidecar, bundled, libsDir) {
  const sha256 = (file) => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
  const lines = [
    "# GenzoParis の配布物に同梱したワーカーとライブラリ（scripts/prepare-worker.mjs が作る）",
    `# 対象: ${triple}`,
    "# 名前\tSHA-256（同梱したもの）\t元の場所",
    `${path.basename(sidecar)}\t${sha256(sidecar)}\t(cargo build -p genzo-worker --features libraw)`,
    ...bundled.map((b) => `${b.name}\t${sha256(path.join(libsDir, b.name))}\t${b.source}`),
  ];
  fs.writeFileSync(MANIFEST_FILE, `${lines.join("\n")}\n`);
  console.log(`同梱の一覧: ${path.relative(REPO_ROOT, MANIFEST_FILE)}`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    prepareWorker();
  } catch (e) {
    console.error(`エラー: ${e instanceof Error ? e.message : e}`);
    process.exit(1);
  }
}
