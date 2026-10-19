// 配布用のワーカー（genzo-worker。LibRaw あり）を release でビルドし、Tauri が同梱する場所に置く処理
// （入口は scripts/prepare-worker.mjs。scripts/lib/build.mjs からも使う）。
//
// 1. `cargo build --release -p genzo-worker --features libraw --locked` でワーカーをビルドする
//    （本体 genzo-paris は LibRaw に依存させない。決定事項ログ No.32。Cargo の機能フラグは同じビルドの中で
//    共通になるため、本体とは別の cargo の実行でビルドする）。
// 2. crates/genzo-app/binaries/genzo-worker-<target triple>（Windows は .exe 付き）に置く
//    （Tauri の externalBin の決まり。tauri.bundle.conf.json）。
// 3. LibRaw の共有ライブラリとその依存を crates/genzo-app/binaries/libs に集め、Tauri に渡す追加の設定
//    crates/genzo-app/binaries/tauri.libs.conf.json を作る。
//    - Windows: ワーカーの読み込む DLL を PE のヘッダからたどり、LibRaw のフォルダ（vcpkg なら
//      installed/<triplet>/bin）にある DLL と、Visual C++ のランタイムの DLL（Visual Studio の再頒布用のフォルダ。
//      下の「Visual C++ のランタイム」）を集める。インストール先のフォルダ（ワーカーと同じ場所）に置く。
//    - macOS: `otool -l` で依存をたどり、Homebrew の dylib を集める。install_name_tool で参照を
//      `@executable_path/../Frameworks/...`（ワーカー）・`@loader_path/...`（dylib どうし）に書き換え、
//      codesign で署名し直す（アドホック。書き換えると署名が壊れ、Apple Silicon では動かなくなるため）。
//      .app の Contents/Frameworks に置く。同梱するものの最小の版（minos）が tauri.conf.json の
//      minimumSystemVersion より新しければ、既定ではエラーにする（下の「macOS の最小の版」）。
//    - Linux: 同梱しない（確認用。OS のパッケージの LibRaw（apt の libraw-dev）を使う）。
// 4. 同梱したファイルの一覧（元の場所・SHA-256）を crates/genzo-app/binaries/bundled-libs.txt に書く
//    （LibRaw の LGPL の扱いを人が確かめるときのため。docs/packaging.md）。
// 5. Windows・macOS では、LibRaw の LGPL-2.1 の全文・著作権表示と案内（使っている版・同梱したファイル・ソースの
//    入手先・差し替えの方法）を crates/genzo-app/binaries/licenses/LibRaw に集め、配布物に入れる
//    （scripts/lib/notice.mjs。版は LibRaw のヘッダ libraw_version.h から、genzo-raw の build.rs と同じ場所を探して
//    読む）。LibRaw をビルドした手順（vcpkg の port の定義とパッチ・Homebrew の formula）の写しと、その手順の
//    リポジトリのコミットも記録する（取れなければ案内に「記録できなかった」と書く。止めない）。
//
// LibRaw の場所:
// - Windows: 環境変数 LIBRAW_INCLUDE_DIR・LIBRAW_LIB_DIR（genzo-raw の build.rs）。なければ
//   VCPKG_ROOT（なければ VCPKG_INSTALLATION_ROOT。GitHub Actions の Windows のランナーにある）の
//   installed/<VCPKG_DEFAULT_TRIPLET、既定は x64-windows> を使う。LIBRAW_LIB_NAME がなければ、.lib の
//   ファイル名から選ぶ（vcpkg は raw_r）。DLL は LIBRAW_DLL_DIR、なければ LIBRAW_LIB_DIR の隣の bin と
//   LIBRAW_LIB_DIR で探す。
// - macOS・Linux: pkg-config（Homebrew の libraw、apt の libraw-dev）。
//
// Visual C++ のランタイム（Windows。決定事項ログ No.73）: ワーカーと LibRaw の DLL が読み込むランタイムの DLL
// （vcruntime140.dll・msvcp140.dll など）を、次の順に探したフォルダから同梱する（見つからなければエラー）。
// 1. 環境変数 GENZO_VC_REDIST_DIR（DLL のあるフォルダ。path.delimiter で区切って複数）
// 2. 環境変数 VCToolsRedistDir（Visual Studio の開発者のコマンドプロンプトが設定する。…\VC\Redist\MSVC\<版>\）
// 3. vswhere で見つけた Visual Studio（Build Tools を含む）の VC\Redist\MSVC\<版>
// 2・3 では、<アーキテクチャ>（x64 など）の下の Microsoft.VC*.CRT・Microsoft.VC*.OpenMP などを探す（デバッグ版は除く）。
// 本体（genzo-paris）は Tauri がランタイムを静的にリンクするので、ランタイムを読み込まない。
//
// macOS の最小の版: Homebrew の dylib は、ビルドした Mac（GitHub Actions ならランナー）の macOS の版向けの
// bottle なので、tauri.conf.json の minimumSystemVersion（12.0）より新しい macOS 向けになりうる。そのままでは、
// 古い macOS でアプリは起動するのにワーカーが起動せず RAW を開けない。配る相手の macOS の版は人の判断なので、
// 既定ではエラーにする。環境変数 GENZO_ALLOW_NEWER_MINOS=1 のときだけ通し、そのときは .app の最小の版
// （Info.plist の LSMinimumSystemVersion）を同梱したものに合わせて上げる（tauri.libs.conf.json に書く）。
// GitHub Actions では、最小の版をステップの出力（macos_min_version・macos_min_raised）とジョブの summary に書く。
//
// Windows・macOS の実機では未確認（この開発環境は Linux）。docs/packaging.md の「未確認の事項」。

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";

import {
  LIBS_DIR,
  chooseLibrawLibName,
  isVcRedistFolder,
  libsTauriConfig,
  pickVcRedistVersion,
  planWindowsDlls,
  vcRedistArch,
} from "./bundle.mjs";
import {
  checkMacosMinimum,
  installNameToolArgs,
  parseOtoolLoadCommands,
  planMacosBundle,
  unbundledReferences,
} from "./macho.mjs";
import {
  LIBRAW_LICENSE_FILES,
  LIBRAW_LICENSE_SOURCE_DIR,
  LIBRAW_NOTICE_FILE,
  LICENSES_BUILD_DIR,
  RECIPE_DIR,
  licenseResources,
  librawNotice,
  librawVersionHeaderCandidates,
  parseLibrawVersion,
} from "./notice.mjs";
import { readPeImports } from "./pe.mjs";
import { APP_DIR, BINARIES_DIR, LIBS_CONFIG, REPO_ROOT, capture, run } from "./proc.mjs";
import { parseHostTriple, sidecarFileName, targetOs } from "./target.mjs";

/** ワーカーの実行ファイルの名前（genzo-worker の WORKER_EXE_NAME。Tauri の externalBin の名前と同じ）。 */
const WORKER_NAME = "genzo-worker";
/** 同梱したファイルの一覧。 */
const MANIFEST_FILE = path.join(BINARIES_DIR, "bundled-libs.txt");
/** macOS: 同梱するものの minos が .app の最小の版より新しくても通すときの環境変数（値は "1"）。 */
const ALLOW_NEWER_MINOS_ENV = "GENZO_ALLOW_NEWER_MINOS";

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
  const windowsLibraw = os === "windows" ? setupWindowsLibraw(env) : null;
  const dllDirs = windowsLibraw?.dllDirs ?? [];

  // 前回の結果を先に消す（途中で失敗したときに、前回の tauri.libs.conf.json・一覧を使ってしまわないように）。
  fs.mkdirSync(BINARIES_DIR, { recursive: true });
  fs.rmSync(LIBS_CONFIG, { force: true });
  fs.rmSync(MANIFEST_FILE, { force: true });

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

  const libsDir = path.join(APP_DIR, LIBS_DIR);
  fs.rmSync(libsDir, { recursive: true, force: true });
  fs.mkdirSync(libsDir, { recursive: true });
  const licensesDir = path.join(APP_DIR, LICENSES_BUILD_DIR);
  fs.rmSync(licensesDir, { recursive: true, force: true });

  const sidecar = path.join(BINARIES_DIR, sidecarFileName(WORKER_NAME, triple));
  fs.rmSync(sidecar, { force: true });
  fs.copyFileSync(built, sidecar);
  fs.chmodSync(sidecar, 0o755);
  console.log(`ワーカー: ${path.relative(REPO_ROOT, sidecar)}`);

  /** @type {Array<{ name: string, source: string }>} */
  let bundled = [];
  /** @type {string[]} 一覧の先頭に足す注記 */
  const notes = [];
  /** @type {string | undefined} .app の最小の版を上げるとき、その版 */
  let raisedMinimum;
  if (os === "windows") {
    const r = collectWindowsDlls(sidecar, dllDirs, findVcRedistDirs(env, triple), libsDir);
    bundled = r.bundled;
    notes.push(...r.notes);
  } else if (os === "macos") {
    const r = collectMacosDylibs(built, sidecar, libsDir);
    bundled = r.bundled;
    notes.push(...r.notes);
    raisedMinimum = r.raisedMinimum;
  } else {
    reportLinuxLibraw(sidecar);
  }

  const resources =
    os === "windows" || os === "macos"
      ? writeLibrawLicenses(
          os,
          readLibrawVersion(os, env),
          bundled.map((b) => b.name),
          licensesDir,
          windowsLibraw?.vcpkgRoot ?? null,
          env,
        )
      : undefined;
  const config = libsTauriConfig(
    os,
    bundled.map((b) => b.name),
    { minimumSystemVersion: raisedMinimum, resources },
  );
  fs.writeFileSync(LIBS_CONFIG, `${JSON.stringify(config, null, 2)}\n`);
  console.log(`Tauri の追加の設定: ${path.relative(REPO_ROOT, LIBS_CONFIG)}`);
  writeManifest(triple, sidecar, bundled, libsDir, notes);
  return { triple, os, sidecar, libs: bundled.map((b) => b.name) };
}

/**
 * Windows: LibRaw の場所を環境変数に入れ、DLL を探すフォルダと、vcpkg の LibRaw を使うならその vcpkg の場所を返す。
 *
 * @returns {{ dllDirs: string[], vcpkgRoot: string | null }}
 */
function setupWindowsLibraw(env) {
  let usedVcpkg = null;
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
    usedVcpkg = vcpkgRoot;
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
  const dllDirs = env.LIBRAW_DLL_DIR
    ? [env.LIBRAW_DLL_DIR]
    : [path.join(env.LIBRAW_LIB_DIR, "..", "bin"), env.LIBRAW_LIB_DIR];
  return { dllDirs, vcpkgRoot: usedVcpkg };
}

/**
 * Windows: Visual C++ のランタイムの DLL を探すフォルダ（上の「Visual C++ のランタイム」の順）。
 *
 * @returns {string[]}
 */
export function findVcRedistDirs(env, triple) {
  if (env.GENZO_VC_REDIST_DIR) {
    return env.GENZO_VC_REDIST_DIR.split(path.delimiter).filter((d) => d !== "");
  }
  const arch = vcRedistArch(triple);
  if (!arch) throw new Error(`Visual C++ のランタイムのアーキテクチャが分からない: ${triple}`);
  const archDirs = (versionDir) => {
    const dir = path.join(versionDir, arch);
    if (!fs.existsSync(dir)) return [];
    return fs
      .readdirSync(dir, { withFileTypes: true })
      .filter((e) => e.isDirectory() && isVcRedistFolder(e.name))
      .map((e) => path.join(dir, e.name));
  };
  if (env.VCToolsRedistDir) {
    const dirs = archDirs(env.VCToolsRedistDir);
    if (dirs.length > 0) return dirs;
  }
  const vswhere = path.join(
    env["ProgramFiles(x86)"] || "C:\\Program Files (x86)",
    "Microsoft Visual Studio",
    "Installer",
    "vswhere.exe",
  );
  if (!fs.existsSync(vswhere)) {
    throw new Error(
      `vswhere が見つからない（${vswhere}）。Visual Studio（Build Tools）を入れるか、環境変数 GENZO_VC_REDIST_DIR に` +
        "Visual C++ のランタイムの DLL のあるフォルダを指定する（docs/packaging.md）",
    );
  }
  const installs = capture(vswhere, [
    "-latest",
    "-products",
    "*",
    "-requires",
    "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
    "-property",
    "installationPath",
  ])
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => l !== "");
  for (const install of installs) {
    const redistRoot = path.join(install, "VC", "Redist", "MSVC");
    if (!fs.existsSync(redistRoot)) continue;
    const defaultFile = path.join(install, "VC", "Auxiliary", "Build", "Microsoft.VCRedistVersion.default.txt");
    const version = pickVcRedistVersion(
      fs.existsSync(defaultFile) ? fs.readFileSync(defaultFile, "utf8") : null,
      fs.readdirSync(redistRoot),
    );
    if (!version) continue;
    const dirs = archDirs(path.join(redistRoot, version));
    if (dirs.length > 0) return dirs;
  }
  throw new Error(
    `Visual C++ のランタイムの再頒布用のフォルダが見つからない（${arch}。vswhere: ${installs.join(", ") || "なし"}）。` +
      "環境変数 GENZO_VC_REDIST_DIR で指定できる（docs/packaging.md）",
  );
}

/** フォルダの中の DLL の索引（小文字の名前 → パス。先に渡したフォルダを優先する）。 */
function indexDlls(dirs) {
  const index = new Map();
  for (const dir of dirs) {
    if (!fs.existsSync(dir)) continue;
    for (const f of fs.readdirSync(dir)) {
      const key = f.toLowerCase();
      if (key.endsWith(".dll") && !index.has(key)) index.set(key, path.join(dir, f));
    }
  }
  return index;
}

/**
 * Windows: ワーカーの読み込む DLL をたどり、LibRaw のフォルダにあるものと、Visual C++ のランタイムを集める。
 *
 * @returns {{ bundled: Array<{ name: string, source: string }>, notes: string[] }}
 */
function collectWindowsDlls(worker, dllDirs, vcRedistDirs, libsDir) {
  const index = indexDlls(dllDirs);
  const vcIndex = indexDlls(vcRedistDirs);
  const readImports = (file) => readPeImports(new Uint8Array(fs.readFileSync(file)));
  const plan = planWindowsDlls(readImports(worker), {
    findDll: (name) => index.get(name.toLowerCase()) ?? null,
    findVcRuntimeDll: (name) => vcIndex.get(name.toLowerCase()) ?? null,
    readImports,
  });
  if (!plan.bundled.some((b) => /raw/i.test(b.name))) {
    throw new Error(
      `ワーカーが LibRaw の DLL を読み込んでいない、または DLL が見つからない（探した場所: ${dllDirs.join(", ")}。` +
        `見つからなかった DLL: ${plan.notFound.join(", ") || "なし"}）`,
    );
  }
  if (plan.vcRuntimeMissing.length > 0) {
    throw new Error(
      `Visual C++ のランタイムの DLL が再頒布用のフォルダにない: ${plan.vcRuntimeMissing.join(", ")}` +
        `（探した場所: ${vcRedistDirs.join(", ")}）`,
    );
  }
  const all = [...plan.bundled, ...plan.vcRuntime];
  for (const b of all) {
    fs.copyFileSync(b.source, path.join(libsDir, b.name));
    console.log(`同梱: ${b.name} ← ${b.source}`);
  }
  if (plan.notFound.length > 0) {
    console.warn(
      `警告: 次の DLL は LibRaw のフォルダになく、OS の DLL の一覧にもない。OS の DLL でなければ同梱が要る: ${plan.notFound.join(", ")}`,
    );
  }
  const notes =
    plan.vcRuntime.length > 0
      ? [
          `# Visual C++ のランタイム（アプリと一緒に置く配置。Windows Update では更新されない。決定事項ログ No.73）: ` +
            `${plan.vcRuntime.map((v) => v.name).join(", ")}（${vcRedistDirs.join(", ")}）`,
        ]
      : [];
  return { bundled: all, notes };
}

/**
 * macOS: Homebrew の dylib を集め、参照を書き換えて署名し直す。
 *
 * @returns {{ bundled: Array<{ name: string, source: string }>, notes: string[], raisedMinimum: string | undefined }}
 *   `raisedMinimum` は .app の最小の版を上げるときの版（GENZO_ALLOW_NEWER_MINOS=1 のときだけ）
 */
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

  // 最小の版を先に確かめる（書き換え・署名の前に止めるため）。
  const minimum = readMinimumMacosVersion();
  const check = checkMacosMinimum(
    [{ name: WORKER_NAME, minos: plan.worker.minos }, ...plan.libs.map((l) => ({ name: l.name, minos: l.minos }))],
    minimum,
    process.env[ALLOW_NEWER_MINOS_ENV] === "1",
  );
  reportMacosMinimumToActions(check, minimum);
  if (check.error) throw new Error(check.error);
  if (check.unknown.length > 0) {
    console.warn(`警告: 最小の版（minos）が読み取れなかった: ${check.unknown.join(", ")}`);
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

  const raised = check.minimumSystemVersion !== minimum;
  if (raised) {
    const message =
      `${ALLOW_NEWER_MINOS_ENV}=1 のため、.app の最小の版を ${minimum} から ${check.minimumSystemVersion} に上げる` +
      `（同梱したもの: ${check.newer.map((n) => `${n.name} ${n.minos}`).join(", ")}）。macOS ${check.minimumSystemVersion} より` +
      "古い Mac では起動できない（docs/packaging.md の 2.3）";
    console.warn(`警告: ${message}`);
    if (process.env.GITHUB_ACTIONS === "true") console.log(`::warning::${message}`);
  }
  const notes = [
    `# macOS の最小の版: .app は ${check.minimumSystemVersion}（tauri.conf.json の minimumSystemVersion は ${minimum}` +
      `${raised ? `。${ALLOW_NEWER_MINOS_ENV}=1 で上げた` : ""}）。同梱したものの minos の最大は ${check.newest ?? "不明"}`,
  ];
  return {
    bundled: plan.libs.map((l) => ({ name: l.name, source: l.source })),
    notes,
    raisedMinimum: raised ? check.minimumSystemVersion : undefined,
  };
}

/** GitHub Actions で実行しているとき、macOS の最小の版をステップの出力とジョブの summary に書く（release.yml）。 */
function reportMacosMinimumToActions(check, minimum) {
  const output = process.env.GITHUB_OUTPUT;
  if (output) {
    const raised = !check.error && check.minimumSystemVersion !== minimum;
    fs.appendFileSync(output, `macos_min_version=${check.minimumSystemVersion}\nmacos_min_raised=${raised ? "1" : "0"}\n`);
  }
  const summary = process.env.GITHUB_STEP_SUMMARY;
  if (summary) {
    const rows = check.items.map((i) => `| ${i.name} | ${i.minos ?? "不明"} |`);
    const lines = [
      "### macOS の最小の版（scripts/lib/worker.mjs）",
      "",
      `- tauri.conf.json の minimumSystemVersion: ${minimum}`,
      `- 同梱するものの minos の最大: ${check.newest ?? "不明"}`,
      check.error
        ? `- **エラー**: ${check.error}`
        : `- .app の最小の版: ${check.minimumSystemVersion}${check.minimumSystemVersion !== minimum ? `（${ALLOW_NEWER_MINOS_ENV}=1 で上げた）` : ""}`,
      "",
      "| ファイル | minos |",
      "|---|---|",
      ...rows,
      "",
    ];
    fs.appendFileSync(summary, `${lines.join("\n")}\n`);
  }
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

/**
 * 同梱する LibRaw の版を、ヘッダ（libraw_version.h）から読む。genzo-raw の build.rs と同じ場所を探す: Windows は
 * LIBRAW_INCLUDE_DIR（setupWindowsLibraw が設定する。libraw/libraw.h・libraw.h のどちらのあるフォルダでもよい）、
 * macOS は pkg-config の libraw_r、なければ libraw の includedir。読めなければエラーにする（案内に版を書けないため）。
 */
export function readLibrawVersion(os, env) {
  const includeDirs = [];
  if (os === "windows") {
    includeDirs.push(env.LIBRAW_INCLUDE_DIR ?? "");
  } else {
    for (const name of ["libraw_r", "libraw"]) {
      try {
        includeDirs.push(capture("pkg-config", ["--variable=includedir", name], { env }).trim());
      } catch {
        // その名前の .pc がない。次を探す。
      }
    }
  }
  const candidates = librawVersionHeaderCandidates(includeDirs).map((p) => path.normalize(p));
  for (const header of candidates) {
    if (!fs.existsSync(header)) continue;
    const version = parseLibrawVersion(fs.readFileSync(header, "utf8"));
    if (version) {
      console.log(`LibRaw の版: ${version}（${header}）`);
      return version;
    }
  }
  throw new Error(`LibRaw の版が読めない（探したヘッダ: ${candidates.join(", ") || "なし"}）`);
}

/**
 * LibRaw をビルドした手順（vcpkg の port・Homebrew の formula）の写しを `dir` に置き、手順の情報を返す。
 * 取れないもの（コミット・写し）は省く（案内に「記録できなかった」と書く）。
 *
 * @returns {{ recipe: import("./notice.mjs").LibrawRecipe, files: string[] }} `files` は写しのファイル（`dir` からの相対）
 */
export function collectLibrawRecipe(os, dir, vcpkgRoot, env) {
  const tryCapture = (cmd, args) => {
    try {
      return capture(cmd, args, { env });
    } catch (e) {
      console.warn(`警告: ${e instanceof Error ? e.message.split("\n")[0] : e}`);
      return null;
    }
  };
  if (os === "windows") {
    const port = vcpkgRoot ? path.join(vcpkgRoot, "ports", "libraw") : null;
    if (!port || !fs.existsSync(port)) return { recipe: { kind: "unknown" }, files: [] };
    const revision = tryCapture("git", ["-C", vcpkgRoot, "rev-parse", "HEAD"])?.trim() || undefined;
    let version;
    try {
      const manifest = JSON.parse(fs.readFileSync(path.join(port, "vcpkg.json"), "utf8"));
      const v = manifest.version ?? manifest["version-string"] ?? manifest["version-semver"] ?? manifest["version-date"];
      if (v) version = manifest["port-version"] ? `${v}#${manifest["port-version"]}` : String(v);
    } catch {
      // 版が読めなくても、写しとコミットは残す。
    }
    const dest = path.join(dir, "vcpkg-ports-libraw");
    fs.cpSync(port, dest, { recursive: true });
    const files = listFiles(dest).map((f) => `vcpkg-ports-libraw/${f}`);
    return { recipe: { kind: "vcpkg", version, revision, copied: files.length > 0 }, files };
  }
  // macOS: Homebrew（brew がなければ手順は分からない）。
  const info = tryCapture("brew", ["info", "--json=v2", "libraw"]);
  if (info === null) return { recipe: { kind: "unknown" }, files: [] };
  let version;
  let revision;
  try {
    const f = JSON.parse(info).formulae?.[0] ?? {};
    version = f.installed?.[0]?.version ?? f.versions?.stable ?? undefined;
    revision = f.tap_git_head || undefined;
  } catch {
    // 情報が読めなくても、formula の写しは試す。
  }
  const formula = tryCapture("brew", ["cat", "libraw"]);
  const files = [];
  if (formula) {
    fs.writeFileSync(path.join(dir, "homebrew-libraw.rb"), formula);
    files.push("homebrew-libraw.rb");
  }
  return { recipe: { kind: "homebrew", version, revision, copied: files.length > 0 }, files };
}

/** フォルダの中のファイル（相対のパス。区切りは "/"）。 */
function listFiles(dir) {
  return fs
    .readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter((e) => e.isFile())
    .map((e) => path.relative(dir, path.join(e.parentPath ?? e.path, e.name)).split(path.sep).join("/"))
    .sort();
}

/**
 * LibRaw のライセンスの文書（上流のファイルをそのまま）・ビルドの手順の写し・案内を `dir` に置き、Tauri の
 * resources の対応を返す。
 *
 * @returns {Record<string, string>}
 */
function writeLibrawLicenses(os, version, bundledNames, dir, vcpkgRoot, env) {
  fs.mkdirSync(path.join(dir, RECIPE_DIR), { recursive: true });
  for (const f of LIBRAW_LICENSE_FILES) {
    fs.copyFileSync(path.join(APP_DIR, LIBRAW_LICENSE_SOURCE_DIR, f), path.join(dir, f));
  }
  const { recipe, files } = collectLibrawRecipe(os, path.join(dir, RECIPE_DIR), vcpkgRoot, env);
  if (files.length === 0) fs.rmSync(path.join(dir, RECIPE_DIR), { recursive: true, force: true });
  console.log(`LibRaw のビルドの手順: ${JSON.stringify(recipe)}`);
  fs.writeFileSync(path.join(dir, LIBRAW_NOTICE_FILE), librawNotice({ os, version, bundled: bundledNames, recipe }));
  console.log(`LibRaw のライセンスの文書と案内: ${path.relative(REPO_ROOT, dir)}`);
  return licenseResources(files);
}

/** tauri.conf.json の bundle.macOS.minimumSystemVersion を読む（ないときはエラー。最小の版を確かめられないため）。 */
function readMinimumMacosVersion() {
  const file = path.join(APP_DIR, "tauri.conf.json");
  const conf = JSON.parse(fs.readFileSync(file, "utf8"));
  const version = conf?.bundle?.macOS?.minimumSystemVersion;
  if (typeof version !== "string" || version === "") {
    throw new Error(`${path.relative(REPO_ROOT, file)} に bundle.macOS.minimumSystemVersion がない`);
  }
  return version;
}

/** 同梱したファイルの一覧を書く。 */
function writeManifest(triple, sidecar, bundled, libsDir, notes) {
  const sha256 = (file) => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
  const lines = [
    "# GenzoParis の配布物に同梱したワーカーとライブラリ（scripts/prepare-worker.mjs が作る）",
    `# 対象: ${triple}`,
    ...notes,
    "# 名前\tSHA-256（同梱したもの）\t元の場所",
    `${path.basename(sidecar)}\t${sha256(sidecar)}\t(cargo build -p genzo-worker --features libraw)`,
    ...bundled.map((b) => `${b.name}\t${sha256(path.join(libsDir, b.name))}\t${b.source}`),
  ];
  fs.writeFileSync(MANIFEST_FILE, `${lines.join("\n")}\n`);
  console.log(`同梱の一覧: ${path.relative(REPO_ROOT, MANIFEST_FILE)}`);
}
