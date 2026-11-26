// 配布物に同梱する ONNX Runtime（AI マスクの推論。DEV-20。docs/ai_masks.md の 2 章・docs/packaging.md）を、Microsoft の
// 公式の配布物（NuGet の Microsoft.ML.OnnxRuntime）から取り、SHA-256 を確かめて、同梱する場所に置く処理
// （scripts/lib/worker.mjs の prepareWorker から呼ぶ）。
//
// - 取得元: 2026-11-26 に、GitHub の Releases から NuGet（nuget.org）に替えた（決定事項ログ No.224）。2026-11-17 ごろから
//   github.com/microsoft/onnxruntime が 404 を返し、Releases の配布物を取れなくなったため。NuGet のパッケージ（.nupkg）は
//   zip で、全 OS のライブラリが 1 つに入っている（約 160 MB）。使うのは OS ごとのライブラリ 1 つとライセンスの文書だけ。
// - 版・パッケージの SHA-256・取り出すライブラリの SHA-256 は、下の ONNXRUNTIME_VERSION・ONNXRUNTIME_PACKAGE・
//   ONNXRUNTIME_ARCHIVES に固定する（どれかが違えば止める）。版を上げるときは、新しいパッケージを取って SHA-256 を
//   書き換え（nuget.org の記録の packageHash とも比べる）、genzo-ml の ONNX Runtime のテスト（GENZO_ORT_DYLIB）を通す。
//   ort（pyke。Cargo.toml）の版が対応する ONNX Runtime の版も確かめる。
// - 置く場所: 同梱のモデルのフォルダ（資源のフォルダの models。Windows はインストール先の models、macOS は
//   GenzoParis.app/Contents/Resources/models）。genzo-api の onnxruntime_candidates が、同梱のモデルのフォルダの中を探す。
//   macOS も Contents/Frameworks には置かない（release.yml の確認が、Frameworks の dylib をすべてワーカーが読み込むことを
//   前提にしているため。ワーカーは ONNX Runtime を読み込まない）。
// - ライセンス: 配布物の LICENSE（MIT）と ThirdPartyNotices.txt を licenses/third-party/onnxruntime/ に入れる。
// - テレメトリー: 公式の配布物は既定で有効。アプリ・CLI が止めて使う（genzo-ml の onnx モジュールの doc）。
// - Windows: onnxruntime.dll は Visual C++ のランタイム（msvcp140.dll・msvcp140_1.dll・vcruntime140.dll・
//   vcruntime140_1.dll。1.31.0 で確認）を読み込む。呼び出し側（worker.mjs）が、ワーカーのものと合わせてインストール先に
//   同梱する（実行ファイルのフォルダは DLL の探す場所の最初）。
// - macOS: dylib の最小の版（minos）を返す（呼び出し側が .app の最小の版の確認に含める）。
// - Linux: 同梱しない（確認用の .deb。利用者が GENZO_ONNXRUNTIME などで指定する）。
//
// ダウンロードには curl を使う（Windows 10 以降・macOS に入っている）。展開には tar を使う（Windows・macOS の tar は
// bsdtar で、zip も展開できる）。使うファイルだけを取り出す。取ったファイルは crates/genzo-app/binaries/downloads に残し、
// 次は SHA-256 が合えば使い回す。

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";

/** 同梱する ONNX Runtime の版（2026-10-09 公開。PyPI の onnxruntime 1.31.0 と同じ日）。 */
export const ONNXRUNTIME_VERSION = "1.31.0";

/**
 * 公式の配布物（NuGet の Microsoft.ML.OnnxRuntime。2026-11-26 に取得して SHA-256 を求めた。159,626,085 バイト）。
 *
 * nuget.org の記録の packageHash（SHA-512）と一致することと、パッケージの署名（作成者 Microsoft Corporation・
 * リポジトリ NuGet.org）が `dotnet nuget verify` で通ることを、同じ日に確かめた。
 *
 * - `id`: パッケージの ID（URL に使うので小文字）
 * - `name`: パッケージのファイル名
 */
export const ONNXRUNTIME_PACKAGE = {
  id: "microsoft.ml.onnxruntime",
  name: `microsoft.ml.onnxruntime.${ONNXRUNTIME_VERSION}.nupkg`,
  sha256: "7e35fdb4f5adb62f01e60798e2e5ed05108f5fbbe1cca03fbcc37903cf7c5e3b",
};

/**
 * OS ごとの、パッケージから取り出す共有ライブラリ。
 *
 * - `name`・`sha256`: 取るパッケージ（両 OS で同じ）
 * - `library`: パッケージの中の共有ライブラリ
 * - `librarySha256`: その SHA-256（取り出した後に確かめる）。Windows のものは、0.2.2 までに GitHub の Releases の
 *   `onnxruntime-win-x64-1.31.0.zip` から同梱したものと同じ。macOS のものは、`onnxruntime-osx-arm64-1.31.0.tgz` から
 *   同梱したもの（SHA-256 `2a50a63e…`）と、コード署名の領域だけが違う（大きさ・それ以外のバイト列・LC_UUID は同じ）
 * - `fileName`: 置くときの名前（genzo-ml の onnxruntime_file_name と同じ）
 */
export const ONNXRUNTIME_ARCHIVES = {
  windows: {
    name: ONNXRUNTIME_PACKAGE.name,
    sha256: ONNXRUNTIME_PACKAGE.sha256,
    library: "runtimes/win-x64/native/onnxruntime.dll",
    librarySha256: "0c42dc4e278f0968fdf9ebf1b1e54a74c657a9942e70803e453c91993451cf4c",
    fileName: "onnxruntime.dll",
  },
  macos: {
    name: ONNXRUNTIME_PACKAGE.name,
    sha256: ONNXRUNTIME_PACKAGE.sha256,
    library: "runtimes/osx-arm64/native/libonnxruntime.dylib",
    librarySha256: "83973572676032b4ba9418afed0b51f45599d35bb4e4628ca8ae1954c8362e31",
    fileName: "libonnxruntime.dylib",
  },
};

/** 取ったものを置くフォルダ（crates/genzo-app からの相対。.gitignore の /binaries の下）。 */
export const ONNXRUNTIME_BUILD_DIR = "binaries/onnxruntime";
/** 取った配布物を残すフォルダ（crates/genzo-app からの相対）。 */
export const DOWNLOADS_DIR = "binaries/downloads";
/** 資源のフォルダの中の、同梱のモデルのフォルダ（genzo-api の MODELS_DIR）。 */
export const MODELS_TARGET_DIR = "models";
/** 資源のフォルダの中の、ONNX Runtime のライセンスの文書のフォルダ。 */
export const ONNXRUNTIME_LICENSES_TARGET_DIR = "licenses/third-party/onnxruntime";
/** 同梱するライセンスの文書（パッケージの最上位にある名前 → 置く名前）。 */
export const ONNXRUNTIME_LICENSE_FILES = [
  ["LICENSE", "LICENSE.txt"],
  ["ThirdPartyNotices.txt", "ThirdPartyNotices.txt"],
];

/**
 * OS の公式の配布物（同梱しない OS なら null）。
 *
 * @param {string} os "windows" | "macos" | "linux" | "other"
 */
export function onnxruntimeArchive(os, archives = ONNXRUNTIME_ARCHIVES) {
  return os === "windows" || os === "macos" ? archives[os] : null;
}

/** パッケージの URL（nuget.org の v3 の flat container。ID とファイル名は小文字）。 */
export function onnxruntimeArchiveUrl(name) {
  return `https://api.nuget.org/v3-flatcontainer/${ONNXRUNTIME_PACKAGE.id}/${ONNXRUNTIME_VERSION}/${name}`;
}

/**
 * Tauri の bundle.resources に足すもの（crates/genzo-app からの相対 → 資源のフォルダの中の場所）。
 *
 * @param {string} os
 * @returns {Record<string, string>}
 */
export function onnxruntimeResources(os, archives = ONNXRUNTIME_ARCHIVES) {
  const archive = onnxruntimeArchive(os, archives);
  if (!archive) return {};
  const out = {
    [`${ONNXRUNTIME_BUILD_DIR}/${archive.fileName}`]: `${MODELS_TARGET_DIR}/${archive.fileName}`,
  };
  for (const [, name] of ONNXRUNTIME_LICENSE_FILES) {
    out[`${ONNXRUNTIME_BUILD_DIR}/${name}`] = `${ONNXRUNTIME_LICENSES_TARGET_DIR}/${name}`;
  }
  return out;
}

/** ファイルの SHA-256（小文字の 16 進数）。 */
export function fileSha256(file) {
  return createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

/**
 * ONNX Runtime を取って確かめ、同梱する場所に置く（Linux などでは何もしない）。
 *
 * @param {string} os
 * @param {{
 *   appDir: string,
 *   run: (cmd: string, args: string[]) => void,
 *   readMinos?: (file: string) => string | null,
 *   download?: (url: string, file: string) => void,
 *   archives?: typeof ONNXRUNTIME_ARCHIVES,
 * }} io `run` はコマンドの実行（失敗したら例外）、`readMinos` は macOS の dylib の最小の版を読む処理、`download` は
 *   取る処理（既定は curl）、`archives` は配布物の表（テスト用。既定は ONNXRUNTIME_ARCHIVES）
 * @returns {{
 *   resources: Record<string, string>,
 *   minos: string | null,
 *   fileName: string,
 *   libraryPath: string,
 *   notes: string[],
 * } | null} `libraryPath` は置いた共有ライブラリ（Windows では、呼び出し側がその読み込む Visual C++ のランタイムも同梱する）
 */
export function prepareOnnxRuntime(os, io) {
  const archives = io.archives ?? ONNXRUNTIME_ARCHIVES;
  const archive = onnxruntimeArchive(os, archives);
  if (!archive) {
    console.log("ONNX Runtime: この OS では同梱しない（Linux の .deb は確認用）");
    return null;
  }
  const downloads = path.join(io.appDir, DOWNLOADS_DIR);
  fs.mkdirSync(downloads, { recursive: true });
  const file = path.join(downloads, archive.name);
  const url = onnxruntimeArchiveUrl(archive.name);
  if (fs.existsSync(file) && fileSha256(file) === archive.sha256) {
    console.log(`ONNX Runtime: 前に取った ${archive.name} を使う`);
  } else {
    fs.rmSync(file, { force: true });
    if (io.download) io.download(url, file);
    else io.run("curl", ["-fsSL", "--retry", "3", "--retry-delay", "5", "-o", file, url]);
  }
  const actual = fileSha256(file);
  if (actual !== archive.sha256) {
    fs.rmSync(file, { force: true });
    throw new Error(
      `ONNX Runtime の配布物 ${archive.name} の SHA-256 が違う（期待 ${archive.sha256}、実際 ${actual}）。` +
        `${url} の中身が変わった可能性がある。中身を確かめるまで同梱しない（scripts/lib/onnxruntime.mjs）`,
    );
  }

  // 使うファイルだけを取り出す（パッケージには全 OS のライブラリが入っている）。
  const extractDir = path.join(downloads, "extract");
  fs.rmSync(extractDir, { recursive: true, force: true });
  fs.mkdirSync(extractDir, { recursive: true });
  io.run("tar", ["-xf", file, "-C", extractDir, archive.library, ...ONNXRUNTIME_LICENSE_FILES.map(([from]) => from)]);

  const library = path.join(extractDir, ...archive.library.split("/"));
  if (!fs.existsSync(library)) throw new Error(`ONNX Runtime の配布物に ${archive.library} がない`);
  const sha = fileSha256(library);
  if (sha !== archive.librarySha256) {
    fs.rmSync(extractDir, { recursive: true, force: true });
    throw new Error(
      `ONNX Runtime の配布物 ${archive.name} の ${archive.library} の SHA-256 が違う` +
        `（期待 ${archive.librarySha256}、実際 ${sha}）。中身を確かめるまで同梱しない（scripts/lib/onnxruntime.mjs）`,
    );
  }

  const out = path.join(io.appDir, ONNXRUNTIME_BUILD_DIR);
  fs.rmSync(out, { recursive: true, force: true });
  fs.mkdirSync(out, { recursive: true });
  const dest = path.join(out, archive.fileName);
  fs.copyFileSync(library, dest);
  for (const [from, to] of ONNXRUNTIME_LICENSE_FILES) {
    const src = path.join(extractDir, from);
    if (!fs.existsSync(src)) throw new Error(`ONNX Runtime の配布物に ${from} がない`);
    fs.copyFileSync(src, path.join(out, to));
  }
  fs.rmSync(extractDir, { recursive: true, force: true });

  const minos = os === "macos" && io.readMinos ? io.readMinos(dest) : null;
  console.log(
    `ONNX Runtime ${ONNXRUNTIME_VERSION}: ${archive.fileName}（SHA-256 ${sha}${minos ? `、minos ${minos}` : ""}）← ${url}`,
  );
  return {
    resources: onnxruntimeResources(os, archives),
    minos,
    fileName: archive.fileName,
    libraryPath: dest,
    notes: [
      `# ONNX Runtime ${ONNXRUNTIME_VERSION}（AI マスク）: ${MODELS_TARGET_DIR}/${archive.fileName}（SHA-256 ${sha}）` +
        ` ← ${url}（SHA-256 ${archive.sha256}）の ${archive.library}`,
    ],
  };
}
