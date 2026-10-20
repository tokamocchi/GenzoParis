// 第三者のライセンスの文書（crates/genzo-app/licenses/third-party の rust.txt・npm.txt）を作る・確かめる処理
// （入口は scripts/third-party.mjs。scripts/lib/build.mjs からも使う）。文書の中身を作る純粋な処理は
// scripts/lib/third-party.mjs。
//
// - Rust: `cargo about generate --format json` を、配布物に入る実行ファイルの crate ごと（本体 genzo-app に
//   custom-protocol、ワーカー genzo-worker に libraw。RUST_ROOTS）に実行し、和を取る。設定はリポジトリの最上位の
//   about.toml（対象の OS・許可するライセンスとその優先の順・開発用の依存を除く）。cargo-about は CARGO_ABOUT_VERSION
//   の版だけを使う（違う版では止める。`cargo install cargo-about --version <版> --locked --features cli`）。
// - npm: ui/package-lock.json と ui/node_modules（`npm ci` の後）から作る。
// - 確かめる（check）: 作り直した内容が、リポジトリの文書と同じか。違えば作り直してコミットするよう案内して止める。
//   npm は、Vite の build.license の出力（ui/dist/.vite/license.json。画面のバンドルに実際に入ったパッケージ）が、
//   すべて一覧にあるかも確かめられる（devDependencies のパッケージを画面から import すると、バンドルに入るのに
//   一覧から漏れるため）。
// - 配布物を作るとき（scripts/lib/build.mjs）: Rust は cargo-about がなくても確かめられるよう、文書に書いた
//   Cargo.lock・about.toml の SHA-256 と今のファイルを比べる。npm は作り直して比べる。

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { APP_DIR, REPO_ROOT, UI_DIR, capture } from "./proc.mjs";
import {
  CARGO_ABOUT_VERSION,
  NPM_NOTICES_FILE,
  NPM_SUPPLEMENTS,
  RUST_NOTICES_FILE,
  RUST_ROOTS,
  RUST_SUPPLEMENTS,
  THIRD_PARTY_SOURCE_DIR,
  formatNotices,
  lockHash,
  missingFromNpmModel,
  npmDistPackages,
  npmModel,
  parseAcceptedList,
  parseTargets,
  readLockHash,
  rustModel,
  unusedSupplements,
} from "./third-party.mjs";

/** cargo-about の設定。 */
export const ABOUT_TOML = path.join(REPO_ROOT, "about.toml");
/** 文書を置くフォルダ。 */
export const THIRD_PARTY_DIR = path.join(APP_DIR, THIRD_PARTY_SOURCE_DIR);
/** Vite の build.license の出力（ui/vite.config.ts）。 */
export const VITE_LICENSE_JSON = path.join(UI_DIR, "dist", ".vite", "license.json");

const CARGO_LOCK = path.join(REPO_ROOT, "Cargo.lock");
const PACKAGE_LOCK = path.join(UI_DIR, "package-lock.json");
/** 文書ごとの、元にしたファイル（文書に SHA-256 を書き、変わっていないかを確かめる）。文書に書く名前 → パス。 */
const SOURCES = {
  rust: [
    ["Cargo.lock", CARGO_LOCK],
    ["about.toml", ABOUT_TOML],
  ],
  npm: [
    ["ui/package-lock.json", PACKAGE_LOCK],
    ["about.toml", ABOUT_TOML],
  ],
};

/** 元にしたファイルの今の SHA-256。 */
const sourceHashes = (kind) => SOURCES[kind].map(([file, p]) => ({ file, hash: lockHash(fs.readFileSync(p, "utf8")) }));

const REGENERATE = "node scripts/third-party.mjs を実行し、作り直した文書をコミットする（docs/third_party.md の 1.4 節）";

/** ファイルを読む処理（rustModel・npmModel に渡す）。 */
const fileIo = {
  listDir: (dir) => {
    try {
      return fs
        .readdirSync(dir, { withFileTypes: true })
        .filter((e) => e.isFile())
        .map((e) => e.name);
    } catch {
      return [];
    }
  },
  readText: (file) => fs.readFileSync(file, "utf8"),
  readSupplement: (file) => fs.readFileSync(path.join(THIRD_PARTY_DIR, file), "utf8"),
};

/** 一覧のどの crate・パッケージにも使われなかった上流の写しを警告する（版を上げたら足し直すか消す）。 */
function warnUnusedSupplements(supplements, model, where) {
  const unused = unusedSupplements(supplements, model);
  if (unused.length > 0) {
    console.warn(
      `警告: 使われなかった上流の写し（${where}。版を上げたなら、新しい版のファイルを確かめて足し直すか消す）: ${unused.join(", ")}`,
    );
  }
}

/**
 * Rust の crate の文書を作る（cargo-about を実行する）。
 *
 * @returns {string}
 */
export function generateRustNotices() {
  const version = capture("cargo", ["about", "--version"], { cwd: REPO_ROOT }).trim();
  if (version !== `cargo-about ${CARGO_ABOUT_VERSION}`) {
    throw new Error(
      `cargo-about の版が違う（${version}。期待: ${CARGO_ABOUT_VERSION}）。` +
        `cargo install cargo-about --version ${CARGO_ABOUT_VERSION} --locked --features cli で入れる`,
    );
  }
  const toml = fs.readFileSync(ABOUT_TOML, "utf8");
  const reports = RUST_ROOTS.map((root) => {
    console.log(`cargo-about: ${root.manifest}（機能フラグ ${root.features.join(" ")}）`);
    return JSON.parse(
      runCargoAbout([
        "generate",
        "--format",
        "json",
        "--locked",
        "--fail",
        "-c",
        ABOUT_TOML,
        "-m",
        path.join(REPO_ROOT, root.manifest),
        "--features",
        root.features.join(" "),
      ]),
    );
  });
  const model = rustModel(reports, fileIo);
  warnUnusedSupplements(RUST_SUPPLEMENTS, model, "RUST_SUPPLEMENTS");
  return formatNotices(model, {
    kind: "rust",
    sources: sourceHashes("rust"),
    facts: [
      `対象の OS / Targets: ${parseTargets(toml).join(", ")}`,
      `対象の実行ファイル / Programs: ${RUST_ROOTS.map((r) => `${r.name}（${r.role}。機能フラグ ${r.features.join(" ")}）`).join("、")}`,
      `依存とライセンスの調査 / Dependency and license data: cargo-about ${CARGO_ABOUT_VERSION}（about.toml。開発用の依存を除く / excluding dev-dependencies）`,
    ],
  });
}

/**
 * cargo-about を実行して標準出力を返す。about.toml の clarify が効かなかったとき（指定したファイルの SHA-256 が
 * 合わないなど。cargo-about は警告を出すだけで、ファイルを走査した結果に戻る）は止める（crate の版が上がって
 * ファイルが変わったら、中身を確かめて clarify を直す）。
 *
 * @param {string[]} args `cargo about` の後の引数
 * @returns {string}
 */
function runCargoAbout(args) {
  const r = spawnSync("cargo", ["about", ...args], {
    cwd: REPO_ROOT,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 256 * 1024 * 1024,
  });
  if (r.error) throw new Error(`cargo about を実行できない: ${r.error.message}`);
  if (r.status !== 0) throw new Error(`cargo about ${args[0]} が失敗した（終了コード ${r.status ?? r.signal}）\n${r.stderr}`);
  const failed = r.stderr
    .split("\n")
    .filter((l) => /failed to validate all files specified in clarification/.test(l));
  if (failed.length > 0) {
    throw new Error(`about.toml の clarify が効かなかった（ファイルの中身を確かめて直す）:\n${failed.join("\n")}`);
  }
  return r.stdout;
}

/**
 * 画面の npm のパッケージの一覧のもとを作る（ui/node_modules を読む）。
 *
 * @returns {import("./third-party.mjs").NoticeModel}
 */
function buildNpmModel() {
  const accepted = parseAcceptedList(fs.readFileSync(ABOUT_TOML, "utf8"));
  const lock = JSON.parse(fs.readFileSync(PACKAGE_LOCK, "utf8"));
  return npmModel(npmDistPackages(lock), accepted, {
    listDir: (p) => fileIo.listDir(path.join(UI_DIR, p)),
    readText: (p) => fileIo.readText(path.join(UI_DIR, p)),
    readPackageJson: (p) => {
      const f = path.join(UI_DIR, p, "package.json");
      return fs.existsSync(f) ? JSON.parse(fs.readFileSync(f, "utf8")) : null;
    },
    readSupplement: fileIo.readSupplement,
  });
}

/**
 * 画面の npm のパッケージの文書を作る。
 *
 * @returns {{ text: string, model: import("./third-party.mjs").NoticeModel }}
 */
export function generateNpmNotices() {
  const model = buildNpmModel();
  warnUnusedSupplements(NPM_SUPPLEMENTS, model, "NPM_SUPPLEMENTS");
  const text = formatNotices(model, {
    kind: "npm",
    sources: sourceHashes("npm"),
    facts: [
      "対象 / Scope: ui/package.json の dependencies と、その推移的な依存（package-lock.json の dev の印のないもの）",
      "  dependencies of ui/package.json and their transitive dependencies (packages not marked dev in package-lock.json)",
      `ライセンスの選択 / License choice: about.toml の accepted の順（Rust の crate と同じ / same as the Rust crates）`,
    ],
  });
  return { text, model };
}

/**
 * Vite の build.license の出力にあるパッケージが、すべて一覧にあるかを確かめる。なければ例外を投げる。
 *
 * @param {import("./third-party.mjs").NoticeModel} model
 */
export function checkViteBundle(model) {
  if (!fs.existsSync(VITE_LICENSE_JSON)) {
    throw new Error(`${path.relative(REPO_ROOT, VITE_LICENSE_JSON)} がない（先に ui/ で npm run build を実行する）`);
  }
  const bundled = JSON.parse(fs.readFileSync(VITE_LICENSE_JSON, "utf8"));
  const missing = missingFromNpmModel(bundled, model);
  if (missing.length > 0) {
    throw new Error(
      `画面のバンドルに入ったのに、npm の一覧にないパッケージがある: ${missing.join(", ")}。` +
        "ui/package.json の devDependencies にあるなら dependencies に移し、" +
        REGENERATE,
    );
  }
  console.log(`画面のバンドルに入ったパッケージ（${bundled.length} 件）は、すべて npm の一覧にある`);
}

/** 文書のファイルのパス。 */
const noticesPath = (file) => path.join(THIRD_PARTY_DIR, file);

/**
 * 文書を作って書く、または今の文書と比べる。
 *
 * @param {{ check: boolean, only: "rust" | "npm" | null, vite: boolean }} options
 *   `vite` は Vite の build.license の出力とも比べる（npm。出力がなければエラー）
 */
export function runThirdParty({ check, only, vite }) {
  fs.mkdirSync(THIRD_PARTY_DIR, { recursive: true });
  const stale = [];
  const handle = (file, text) => {
    const target = noticesPath(file);
    const rel = path.relative(REPO_ROOT, target);
    if (check) {
      const current = fs.existsSync(target) ? fs.readFileSync(target, "utf8") : null;
      if (current !== text) {
        stale.push(`${rel}（${describeDifference(current, text)}）`);
      } else {
        console.log(`${rel}: 最新`);
      }
      return;
    }
    fs.writeFileSync(target, text);
    console.log(`${rel} を書いた（${Buffer.byteLength(text)} バイト）`);
  };
  if (only !== "npm") handle(RUST_NOTICES_FILE, generateRustNotices());
  if (only !== "rust") {
    const { text, model } = generateNpmNotices();
    handle(NPM_NOTICES_FILE, text);
    if (vite) checkViteBundle(model);
  }
  if (stale.length > 0) {
    throw new Error(`第三者のライセンスの文書が古い: ${stale.join("、")}。${REGENERATE}`);
  }
}

/** 今の文書と作り直した文書の、最初に違う行（案内用）。 */
function describeDifference(current, next) {
  if (current === null) return "ファイルがない";
  const a = current.split("\n");
  const b = next.split("\n");
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    if (a[i] !== b[i]) return `${i + 1} 行目から違う: 今「${(a[i] ?? "").slice(0, 60)}」→ 新「${(b[i] ?? "").slice(0, 60)}」`;
  }
  return "改行などが違う";
}

/**
 * 配布物を作る前に、文書が今の lock のファイル・about.toml から作ったものかを確かめる（scripts/lib/build.mjs）。
 * Rust は文書に書いた Cargo.lock・about.toml の SHA-256 を比べ（cargo-about を使わない）、npm は作り直して比べる。
 * 古ければ例外を投げる。
 */
export function checkNoticesForBuild() {
  const rustFile = noticesPath(RUST_NOTICES_FILE);
  if (!fs.existsSync(rustFile)) throw new Error(`${path.relative(REPO_ROOT, rustFile)} がない。${REGENERATE}`);
  const notices = fs.readFileSync(rustFile, "utf8");
  for (const { file, hash } of sourceHashes("rust")) {
    const recorded = readLockHash(notices, file);
    if (recorded !== hash) {
      throw new Error(
        `${path.relative(REPO_ROOT, rustFile)} は今の ${file} から作ったものではない（文書: ${recorded ?? "記録なし"}、` +
          `${file}: ${hash}）。${REGENERATE}`,
      );
    }
  }
  runThirdParty({ check: true, only: "npm", vite: false });
  console.log("第三者のライセンスの文書は、今の Cargo.lock・ui/package-lock.json・about.toml から作ったもの");
}
