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
//    同梱した LibRaw をビルドしたソースのアーカイブ（vcpkg は LibRaw と LibRaw-cmake、Homebrew は LibRaw の配布物）を
//    licenses/LibRaw/source に入れる（入手元が取得したものを使い、なければ同じ URL から取る。vcpkg の portfile の
//    SHA512・Homebrew の formula の SHA-256 と同じかを確かめ、違えば止める）。
// 6. Windows・macOS では、同梱した共有ライブラリのライセンスの文書（licenses/third-party/native.txt）を、入手元の
//    パッケージ（vcpkg の port・Homebrew の formula）がインストールした文書から作る（scripts/lib/native-notice.mjs。
//    文書の見つからないライブラリがあれば止める）。
// 7. Windows で Visual C++ のランタイムを同梱するときは、その使用条件（crates/genzo-app/licenses/windows/
//    vcruntime-terms.txt）をインストール先の licenses/third-party に入れ、インストーラー（NSIS）の同意の画面の文書を、
//    使用条件と GPL-3.0 の全文をまとめたもの（binaries/installer-license.txt）に差し替える（bundle.licenseFile。
//    Microsoft の Distributable Code の条件。決定事項ログ No.199）。
//
// LibRaw の場所:
// - Windows: VCPKG_ROOT（なければ VCPKG_INSTALLATION_ROOT。GitHub Actions の Windows のランナーにある）の
//   installed/<VCPKG_DEFAULT_TRIPLET、既定は x64-windows> を使う。環境変数 LIBRAW_INCLUDE_DIR・LIBRAW_LIB_DIR
//   （genzo-raw の build.rs）で指定してもよいが、その vcpkg の中でなければ止める（同梱する DLL のライセンスの文書と
//   LibRaw のソースを vcpkg から集めるため。決定事項ログ No.196）。LIBRAW_LIB_NAME がなければ、.lib の
//   ファイル名から選ぶ（vcpkg は raw_r）。DLL は LIBRAW_DLL_DIR、なければ LIBRAW_LIB_DIR の隣の bin と
//   LIBRAW_LIB_DIR で探す。
// - macOS: pkg-config（Homebrew の libraw。同梱する dylib は Homebrew の Cellar の中のものでなければ止める）。
// - Linux: pkg-config（apt の libraw-dev。同梱しない）。
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
  SOURCE_DIR,
  licenseResources,
  librawNotice,
  librawVersionHeaderCandidates,
  parseLibrawVersion,
} from "./notice.mjs";
import {
  INSTALLER_LICENSE_BUILD_PATH,
  NATIVE_NOTICES_BUILD_PATH,
  NATIVE_NOTICES_TARGET_PATH,
  VCRUNTIME_TERMS_SOURCE,
  VCRUNTIME_TERMS_TARGET,
  formatNativeNotices,
  changedVcpkgPortFiles,
  githubArchiveUrl,
  homebrewFormulaInfo,
  installerLicenseText,
  isNativeLicenseFile,
  parseCellarPath,
  parseVcpkgAbiInfo,
  parseVcpkgFromGithub,
  vcRedistFolderLabel,
  vcpkgArchiveName,
  vcpkgDllOwners,
  vcpkgPortInfo,
} from "./native-notice.mjs";
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
  const nativeNotices = path.join(APP_DIR, NATIVE_NOTICES_BUILD_PATH);
  fs.rmSync(nativeNotices, { force: true });
  const installerLicense = path.join(APP_DIR, INSTALLER_LICENSE_BUILD_PATH);
  fs.rmSync(installerLicense, { force: true });

  const sidecar = path.join(BINARIES_DIR, sidecarFileName(WORKER_NAME, triple));
  fs.rmSync(sidecar, { force: true });
  fs.copyFileSync(built, sidecar);
  fs.chmodSync(sidecar, 0o755);
  console.log(`ワーカー: ${path.relative(REPO_ROOT, sidecar)}`);

  /** @type {Array<{ name: string, source: string }>} */
  let bundled = [];
  /** @type {string[]} 一覧の先頭に足す注記 */
  const notes = [];
  /** @type {Array<{ name: string, source: string }>} Visual C++ のランタイム（Windows。bundled にも含む） */
  let vcRuntime = [];
  /** @type {string | undefined} .app の最小の版を上げるとき、その版 */
  let raisedMinimum;
  if (os === "windows") {
    const r = collectWindowsDlls(sidecar, dllDirs, findVcRedistDirs(env, triple), libsDir);
    bundled = r.bundled;
    vcRuntime = r.vcRuntime;
    notes.push(...r.notes);
  } else if (os === "macos") {
    const r = collectMacosDylibs(built, sidecar, libsDir);
    bundled = r.bundled;
    notes.push(...r.notes);
    raisedMinimum = r.raisedMinimum;
  } else {
    reportLinuxLibraw(sidecar);
  }

  let resources;
  if (os === "windows" || os === "macos") {
    const vcpkgRoot = windowsLibraw?.vcpkgRoot ?? null;
    const vcName = new Set(vcRuntime.map((v) => v.name));
    const libs = bundled.filter((b) => !vcName.has(b.name));
    const components =
      os === "windows"
        ? nativeComponentsWindows(libs, vcpkgRoot, windowsLibraw?.triplet ?? "", env)
        : nativeComponentsMacos(libs, env);
    const sourceFiles = collectLibrawSource(os, path.join(licensesDir, SOURCE_DIR), components, vcpkgRoot, env);
    resources = {
      ...writeLibrawLicenses(os, readLibrawVersion(os, env), bundled.map((b) => b.name), licensesDir, vcpkgRoot, sourceFiles, env),
      ...writeNativeNotices(os, triple, components, vcRuntime, vcpkgRoot, windowsLibraw?.triplet ?? "", env, nativeNotices),
    };
  }
  // Visual C++ のランタイムを同梱するときは、その使用条件をインストール先に入れ、インストーラーの同意の画面で
  // 同意を求める（Microsoft の Distributable Code の条件。決定事項ログ No.199）。
  let licenseFile;
  if (vcRuntime.length > 0) {
    resources = { ...resources, [VCRUNTIME_TERMS_SOURCE]: VCRUNTIME_TERMS_TARGET };
    licenseFile = writeInstallerLicense(installerLicense);
  }
  const config = libsTauriConfig(
    os,
    bundled.map((b) => b.name),
    { minimumSystemVersion: raisedMinimum, resources, licenseFile },
  );
  fs.writeFileSync(LIBS_CONFIG, `${JSON.stringify(config, null, 2)}\n`);
  console.log(`Tauri の追加の設定: ${path.relative(REPO_ROOT, LIBS_CONFIG)}`);
  writeManifest(triple, sidecar, bundled, libsDir, notes);
  return { triple, os, sidecar, libs: bundled.map((b) => b.name) };
}

/** `file` が `dir` の中か（`dir` そのものは含めない。パスを解決して比べる。Windows では大文字・小文字を区別しない）。 */
function isInsideDir(dir, file) {
  const rel = path.relative(path.resolve(dir), path.resolve(file));
  return rel !== "" && !rel.startsWith("..") && !path.isAbsolute(rel);
}

/**
 * Windows: LibRaw の場所を環境変数に入れ、DLL を探すフォルダと、vcpkg の場所と triplet を返す。配布物を作るには
 * vcpkg の LibRaw が要る（同梱する DLL のライセンスの文書と LibRaw のソースを vcpkg から集めるため。決定事項ログ
 * No.196）ので、vcpkg の LibRaw でなければ、時間のかかるビルドの前に止める。LIBRAW_INCLUDE_DIR・LIBRAW_LIB_DIR・
 * LIBRAW_DLL_DIR は指定してもよいが、すべて VCPKG_ROOT（なければ VCPKG_INSTALLATION_ROOT）の installed/<triplet> の
 * 中でなければ止める（指定のないものは、その vcpkg の場所で埋める）。
 *
 * @returns {{ dllDirs: string[], vcpkgRoot: string, triplet: string }}
 */
export function setupWindowsLibraw(env) {
  const triplet = env.VCPKG_DEFAULT_TRIPLET || "x64-windows";
  const vcpkgRoot = env.VCPKG_ROOT || env.VCPKG_INSTALLATION_ROOT;
  const vcpkgRequired =
    "配布物を作るには vcpkg の LibRaw が要る（同梱する DLL のライセンスの文書と LibRaw のソースを vcpkg から集めるため。" +
    "決定事項ログ No.196）。VCPKG_ROOT を設定し、LIBRAW_INCLUDE_DIR・LIBRAW_LIB_DIR・LIBRAW_DLL_DIR を消すか、" +
    "すべて vcpkg の中を指す（docs/packaging.md の 2.2）";
  if (!vcpkgRoot) {
    if (env.LIBRAW_INCLUDE_DIR || env.LIBRAW_LIB_DIR || env.LIBRAW_DLL_DIR) throw new Error(`${vcpkgRequired}（VCPKG_ROOT がない）`);
    throw new Error("LibRaw の場所が分からない。環境変数 VCPKG_ROOT を設定する（docs/packaging.md の 2.2）");
  }
  // 指定のないものは vcpkg の場所で埋め、指定したもの（ヘッダ・.lib・DLL）を含めてすべて、その vcpkg の
  // installed/<triplet> の中かを確かめる（片方だけの指定も確かめる）。
  const installed = path.join(vcpkgRoot, "installed", triplet);
  env.LIBRAW_INCLUDE_DIR ||= path.join(installed, "include");
  env.LIBRAW_LIB_DIR ||= path.join(installed, "lib");
  const outside = ["LIBRAW_INCLUDE_DIR", "LIBRAW_LIB_DIR", "LIBRAW_DLL_DIR"].filter(
    (k) => env[k] && !isInsideDir(installed, env[k]),
  );
  if (outside.length > 0) {
    throw new Error(`${vcpkgRequired}（${outside.map((k) => `${k}=${env[k]}`).join("、")} は ${installed} の外）`);
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
  return { dllDirs, vcpkgRoot, triplet };
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
 * @returns {{ bundled: Array<{ name: string, source: string }>, vcRuntime: Array<{ name: string, source: string }>, notes: string[] }}
 *   `bundled` は Visual C++ のランタイムを含むすべて
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
  return { bundled: all, vcRuntime: plan.vcRuntime, notes };
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
 * @param {string[]} sourceFiles `dir` の SOURCE_DIR に置いたソースのアーカイブ
 * @returns {Record<string, string>}
 */
function writeLibrawLicenses(os, version, bundledNames, dir, vcpkgRoot, sourceFiles, env) {
  fs.mkdirSync(path.join(dir, RECIPE_DIR), { recursive: true });
  for (const f of LIBRAW_LICENSE_FILES) {
    fs.copyFileSync(path.join(APP_DIR, LIBRAW_LICENSE_SOURCE_DIR, f), path.join(dir, f));
  }
  const { recipe, files } = collectLibrawRecipe(os, path.join(dir, RECIPE_DIR), vcpkgRoot, env);
  if (files.length === 0) fs.rmSync(path.join(dir, RECIPE_DIR), { recursive: true, force: true });
  console.log(`LibRaw のビルドの手順: ${JSON.stringify(recipe)}`);
  fs.writeFileSync(
    path.join(dir, LIBRAW_NOTICE_FILE),
    librawNotice({ os, version, bundled: bundledNames, recipe, source: sourceFiles }),
  );
  console.log(`LibRaw のライセンスの文書と案内: ${path.relative(REPO_ROOT, dir)}`);
  return licenseResources(files, sourceFiles);
}

/**
 * ライセンスの文書を読む（UTF-8 として読めなければ Latin-1 として読む。古い文書の © などのため）。
 *
 * @param {string} file
 */
function readLicenseText(file) {
  const bytes = fs.readFileSync(file);
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return bytes.toString("latin1");
  }
}

/** vcpkg のフォルダの git のコミット（取れなければ null）。 */
function vcpkgRevision(vcpkgRoot, env) {
  try {
    return capture("git", ["-C", vcpkgRoot, "rev-parse", "HEAD"], { env }).trim() || null;
  } catch {
    return null;
  }
}

/**
 * Windows: 同梱した DLL（Visual C++ のランタイムを除く）を入れた vcpkg の port と、その著作権の文書を調べる。
 *
 * @param {Array<{ name: string, source: string }>} libs
 * @param {string | null} vcpkgRoot
 * @param {string} triplet
 * @returns {import("./native-notice.mjs").NativeComponent[]}
 */
export function nativeComponentsWindows(libs, vcpkgRoot, triplet, env) {
  if (!vcpkgRoot) {
    throw new Error(
      "同梱する DLL の入手元が分からず、ライセンスの文書（native.txt）を作れない。vcpkg の LibRaw を使う" +
        "（環境変数 LIBRAW_INCLUDE_DIR・LIBRAW_LIB_DIR を設定せず、VCPKG_ROOT を設定する。docs/packaging.md の 2.2）",
    );
  }
  const infoDir = path.join(vcpkgRoot, "installed", "vcpkg", "info");
  const lists = fs.existsSync(infoDir)
    ? fs
        .readdirSync(infoDir)
        .filter((n) => n.endsWith(`_${triplet}.list`))
        .map((n) => ({ name: n, text: fs.readFileSync(path.join(infoDir, n), "utf8") }))
    : [];
  const owners = vcpkgDllOwners(lists, triplet);
  const revision = vcpkgRevision(vcpkgRoot, env);
  const bin = path.join(vcpkgRoot, "installed", triplet, "bin");
  return libs.map((lib) => {
    // 持ち主は名前で決めるので、同梱するファイルがその vcpkg の bin のものかを先に確かめる（外の同じ名前の DLL に、
    // vcpkg の文書を付けないため）。
    if (!isInsideDir(bin, lib.source)) {
      throw new Error(`同梱する ${lib.name}（${lib.source}）が vcpkg の ${bin} の中にない。入手元が分からない`);
    }
    const owner = owners.get(lib.name.toLowerCase());
    if (!owner) {
      throw new Error(`同梱する ${lib.name} を入れた vcpkg の port が分からない（${infoDir} の *_${triplet}.list にない）`);
    }
    const share = path.join(vcpkgRoot, "installed", triplet, "share", owner.port);
    const copyright = path.join(share, "copyright");
    const portDir = path.join(vcpkgRoot, "ports", owner.port);
    const manifest = path.join(portDir, "vcpkg.json");
    const info = vcpkgPortInfo(fs.existsSync(manifest) ? JSON.parse(fs.readFileSync(manifest, "utf8")) : null);
    checkVcpkgPortIsInstalledOne(owner, info, portDir, path.join(share, "vcpkg_abi_info.txt"));
    return {
      name: owner.port,
      version: owner.version,
      license: info.license,
      homepage: info.homepage,
      recipe: revision ? `https://github.com/microsoft/vcpkg/tree/${revision}/ports/${owner.port}` : null,
      source: null,
      files: [lib.name],
      texts: fs.existsSync(copyright) ? [{ file: `share/${owner.port}/copyright`, text: readLicenseText(copyright) }] : [],
    };
  });
}

/**
 * Windows: ports/<port>（ライセンスの表示・portfile を読む）が、入っている port をビルドしたときのものと同じかを
 * 確かめる（違えば止める。vcpkg を更新したのに port を入れ直していないと、文書がビルドと食い違うため）。
 * vcpkg の ABI の情報（vcpkg_abi_info.txt）の port のファイルの SHA-256 と比べる。ABI の情報がなければ止める（info の
 * 一覧の名前の版には port-version が入らず、版だけでは port の定義の違いを見分けられないため）。
 *
 * @param {{ port: string, version: string }} owner
 * @param {{ version: string | null }} info ports の vcpkg.json
 * @param {string} portDir
 * @param {string} abiFile
 */
function checkVcpkgPortIsInstalledOne(owner, info, portDir, abiFile) {
  const hint = `vcpkg の ports と入っている ${owner.port} が違う。vcpkg install（または vcpkg upgrade）し直す`;
  const base = (v) => v.replace(/#\d+$/, "");
  if (!info.version || base(info.version) !== base(owner.version)) {
    throw new Error(`${hint}（入っている版 ${owner.version}、ports の版 ${info.version ?? "不明"}）`);
  }
  if (!fs.existsSync(abiFile)) {
    // 版だけでは port-version の違い（同じ上流の版で、port の定義・パッチが違う）を見分けられないので止める。
    throw new Error(`${hint}（${abiFile} がなく、ports のファイルが入っているものと同じかを確かめられない）`);
  }
  const portFiles = listFiles(portDir).map((f) => ({ file: f, sha256: fileHash(path.join(portDir, ...f.split("/")), "sha256") }));
  const changed = changedVcpkgPortFiles(parseVcpkgAbiInfo(fs.readFileSync(abiFile, "utf8")), portFiles);
  if (changed.length > 0) throw new Error(`${hint}（ビルドしたときと違うファイル: ${changed.join(", ")}）`);
}

/**
 * macOS: 同梱した dylib を入れた Homebrew の formula と、keg のライセンスの文書を調べる。
 *
 * @param {Array<{ name: string, source: string }>} libs `source` は実体の場所（Cellar の中）
 * @returns {import("./native-notice.mjs").NativeComponent[]}
 */
export function nativeComponentsMacos(libs, env) {
  const kegs = libs.map((lib) => {
    const keg = parseCellarPath(lib.source);
    if (!keg) throw new Error(`同梱する ${lib.name} が Homebrew の Cellar の中にない（${lib.source}）。入手元が分からない`);
    return { lib, ...keg };
  });
  const formulae = homebrewFormulae([...new Set(kegs.map((k) => k.formula))], env);
  return kegs.map((k) => {
    const info = homebrewFormulaInfo(formulae.get(k.formula));
    // ライセンスの表示・formula の URL は今の formula のもの。同梱する keg がそれと違う版なら止める（文書がビルドと
    // 食い違うため）。
    if (k.version !== info.kegVersion) {
      throw new Error(
        `Homebrew の ${k.formula} は ${k.version} が入っているが、formula の版は ${info.kegVersion ?? "不明"}` +
          `（brew upgrade ${k.formula} してからビルドし直す）`,
      );
    }
    return {
      name: k.formula,
      version: k.version,
      license: info.license,
      homepage: info.homepage,
      recipe: info.recipe,
      source: info.source,
      files: [k.lib.name],
      texts: kegLicenseTexts(k.keg),
    };
  });
}

/** `brew info --json=v2` の formula（名前 → 情報）。 */
function homebrewFormulae(names, env) {
  if (names.length === 0) return new Map();
  const json = JSON.parse(capture("brew", ["info", "--json=v2", ...names], { env }));
  return new Map((json.formulae ?? []).map((f) => [f.name, f]));
}

/** keg の直下と share/doc/<名前> の、ライセンス・NOTICE のファイル（Homebrew の keg の構成）。 */
function kegLicenseTexts(keg) {
  const out = [];
  const add = (dir, rel) => {
    for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
      if (e.isFile() && isNativeLicenseFile(e.name)) {
        out.push({ file: rel ? `${rel}/${e.name}` : e.name, text: readLicenseText(path.join(dir, e.name)) });
      }
    }
  };
  add(keg, "");
  const doc = path.join(keg, "share", "doc");
  if (fs.existsSync(doc)) {
    for (const e of fs.readdirSync(doc, { withFileTypes: true })) {
      if (e.isDirectory()) add(path.join(doc, e.name), `share/doc/${e.name}`);
    }
  }
  return out;
}

/**
 * 同梱した共有ライブラリのライセンスの文書（native.txt）を書き、Tauri の resources の対応を返す。
 *
 * @returns {Record<string, string>}
 */
function writeNativeNotices(os, triple, components, vcRuntime, vcpkgRoot, triplet, env, file) {
  let manager = "Homebrew（homebrew-core）";
  if (os === "windows") {
    const revision = vcpkgRoot ? vcpkgRevision(vcpkgRoot, env) : null;
    manager = `vcpkg（triplet ${triplet}${revision ? `。https://github.com/microsoft/vcpkg/tree/${revision}` : ""}）`;
  }
  const text = formatNativeNotices({
    os,
    triple,
    manager,
    components,
    vcRuntime:
      vcRuntime.length > 0
        ? {
            files: vcRuntime.map((v) => v.name),
            folders: [...new Set(vcRuntime.map((v) => vcRedistFolderLabel(path.dirname(v.source))).filter((f) => f !== null))],
          }
        : null,
  });
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
  console.log(`同梱したライブラリのライセンスの文書: ${path.relative(REPO_ROOT, file)}`);
  return { [NATIVE_NOTICES_BUILD_PATH]: NATIVE_NOTICES_TARGET_PATH };
}

/**
 * インストーラーの同意の画面に出す文書（Visual C++ のランタイムの使用条件と GPL-3.0 の全文）を書き、
 * Tauri の `bundle.licenseFile` に渡す場所（crates/genzo-app からの相対）を返す。
 *
 * @param {string} file 書く場所
 * @returns {string}
 */
function writeInstallerLicense(file) {
  const terms = fs.readFileSync(path.join(APP_DIR, VCRUNTIME_TERMS_SOURCE), "utf8");
  const gpl = fs.readFileSync(path.join(REPO_ROOT, "LICENSE"), "utf8");
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, installerLicenseText(terms, gpl));
  console.log(`インストーラーの同意の画面の文書: ${path.relative(REPO_ROOT, file)}（Visual C++ のランタイムの使用条件と GPL-3.0）`);
  return INSTALLER_LICENSE_BUILD_PATH;
}

/** ファイルのハッシュ（16 進）。 */
function fileHash(file, algorithm) {
  return createHash(algorithm).update(fs.readFileSync(file)).digest("hex");
}

/**
 * `cached`（入手元が取得したもの）があれば写し、なければ `url` から取り、ハッシュを確かめる（違えば止める）。
 */
function placeArchive(dest, cached, url, algorithm, expected) {
  if (cached && fs.existsSync(cached)) {
    fs.copyFileSync(cached, dest);
  } else {
    console.log(`取得: ${url}`);
    run("curl", ["-fsSL", "--retry", "3", "-o", dest, url]);
  }
  const actual = fileHash(dest, algorithm);
  if (actual !== expected) {
    fs.rmSync(dest, { force: true });
    throw new Error(`${path.basename(dest)} の ${algorithm.toUpperCase()} が違う（${actual}。期待: ${expected}）`);
  }
  console.log(`LibRaw のソース: ${path.basename(dest)}（${algorithm.toUpperCase()} を確かめた）`);
}

/**
 * 同梱した LibRaw をビルドしたソースのアーカイブを `dir` に置き、置いたファイルの名前を返す。
 *
 * @param {import("./native-notice.mjs").NativeComponent[]} components 同梱したライブラリ（LibRaw の版を使う）
 * @returns {string[]}
 */
export function collectLibrawSource(os, dir, components, vcpkgRoot, env) {
  const libraw = components.find((c) => c.name === "libraw");
  if (!libraw) throw new Error(`同梱したライブラリに LibRaw（vcpkg の port・Homebrew の formula「libraw」）がない`);
  fs.mkdirSync(dir, { recursive: true });
  const placed = [];
  if (os === "windows") {
    if (!vcpkgRoot) throw new Error("vcpkg の場所が分からず、LibRaw のソースを集められない");
    const port = path.join(vcpkgRoot, "ports", "libraw");
    const manifest = vcpkgPortInfo(JSON.parse(fs.readFileSync(path.join(port, "vcpkg.json"), "utf8")));
    if (manifest.version !== libraw.version && manifest.version?.replace(/#\d+$/, "") !== libraw.version) {
      throw new Error(`vcpkg の libraw は ${libraw.version} が入っているが、ports の版は ${manifest.version}（ソースが合わない）`);
    }
    const version = (manifest.version ?? "").replace(/#\d+$/, "");
    for (const a of parseVcpkgFromGithub(fs.readFileSync(path.join(port, "portfile.cmake"), "utf8"), version)) {
      const name = vcpkgArchiveName(a.repo, a.ref);
      const downloads = env.VCPKG_DOWNLOADS || path.join(vcpkgRoot, "downloads");
      placeArchive(path.join(dir, name), path.join(downloads, name), githubArchiveUrl(a.repo, a.ref), "sha512", a.sha512);
      placed.push(name);
    }
  } else {
    const info = homebrewFormulaInfo(homebrewFormulae(["libraw"], env).get("libraw"));
    if (!info.source || !info.sourceSha256) throw new Error("Homebrew の libraw の formula に、ソースの URL・SHA-256 がない");
    if (libraw.version !== info.kegVersion) {
      throw new Error(`Homebrew の libraw は ${libraw.version} が入っているが、formula の版は ${info.kegVersion}（ソースが合わない。brew upgrade libraw）`);
    }
    const name = decodeURIComponent(new URL(info.source).pathname.split("/").pop() ?? "");
    if (!/^[\w.+-]+$/.test(name)) throw new Error(`ソースのファイルの名前が読めない: ${info.source}`);
    let cached = null;
    try {
      cached = capture("brew", ["--cache", "--build-from-source", "libraw"], { env }).trim() || null;
    } catch {
      // キャッシュの場所が分からなければ取りに行く。
    }
    placeArchive(path.join(dir, name), cached, info.source, "sha256", info.sourceSha256);
    placed.push(name);
  }
  if (placed.length === 0) throw new Error("LibRaw のソースのアーカイブが 1 つもない（vcpkg の portfile を確かめる）");
  return placed.sort();
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
