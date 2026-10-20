// third-party.mjs のテスト（node --test）。

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";

import { APP_DIR, REPO_ROOT } from "./proc.mjs";
import {
  NPM_NOTICES_FILE,
  NPM_SUPPLEMENTS,
  RUST_NOTICES_FILE,
  RUST_TARGETS,
  STATIC_LICENSE_RESOURCES,
  THIRD_PARTY_SOURCE_DIR,
  chooseLicenses,
  compareNatural,
  findLicenseTexts,
  formatNotices,
  isLicenseFileName,
  isNoticeFileName,
  isSourceFileName,
  leadingComment,
  lockHash,
  missingFromNpmModel,
  normalizeText,
  npmDistPackages,
  npmModel,
  parseAcceptedList,
  parseSpdx,
  parseTargets,
  pickLicenseFiles,
  readLockHash,
  rustModel,
  textMatchesLicense,
} from "./third-party.mjs";

const MIT = (holder) =>
  `MIT License\n\nCopyright (c) ${holder}\n\nPermission is hereby granted, free of charge, to any person obtaining a copy\n`;
const APACHE = "                                 Apache License\n                           Version 2.0, January 2004\n";
const ACCEPTED = ["CC0-1.0", "MIT", "ISC", "BSD-3-Clause", "Unicode-3.0", "Apache-2.0", "Apache-2.0 WITH LLVM-exception"];
const aboutToml = () => fs.readFileSync(path.join(REPO_ROOT, "about.toml"), "utf8");

test("SPDX の式を読む（OR より AND が強い。括弧・WITH）", () => {
  assert.deepEqual(parseSpdx("MIT"), { license: "MIT" });
  assert.deepEqual(parseSpdx("MIT OR Apache-2.0"), { or: [{ license: "MIT" }, { license: "Apache-2.0" }] });
  assert.deepEqual(parseSpdx("A AND B OR C"), {
    or: [{ and: [{ license: "A" }, { license: "B" }] }, { license: "C" }],
  });
  assert.deepEqual(parseSpdx("(MIT OR Apache-2.0) AND Unicode-3.0"), {
    and: [{ or: [{ license: "MIT" }, { license: "Apache-2.0" }] }, { license: "Unicode-3.0" }],
  });
  assert.deepEqual(parseSpdx("Apache-2.0 WITH LLVM-exception OR MIT"), {
    or: [{ license: "Apache-2.0 WITH LLVM-exception" }, { license: "MIT" }],
  });
  for (const bad of ["", "MIT OR", "(MIT", "MIT)", "OR MIT", "MIT WITH", "MIT Apache-2.0"]) {
    assert.throws(() => parseSpdx(bad), /SPDX の式が読めない/, bad);
  }
});

test("ライセンスを accepted の順で選ぶ（cargo-about と同じ選び方）", () => {
  assert.deepEqual(chooseLicenses("MIT OR Apache-2.0", ACCEPTED), ["MIT"]);
  assert.deepEqual(chooseLicenses("Apache-2.0 OR MIT", ACCEPTED), ["MIT"]);
  assert.deepEqual(chooseLicenses("Unlicense OR MIT", ACCEPTED), ["MIT"]);
  assert.deepEqual(chooseLicenses("CC0-1.0 OR MIT-0 OR Apache-2.0", ACCEPTED), ["CC0-1.0"]);
  assert.deepEqual(chooseLicenses("Apache-2.0 AND MIT", ACCEPTED), ["Apache-2.0", "MIT"]);
  assert.deepEqual(chooseLicenses("(MIT OR Apache-2.0) AND Unicode-3.0", ACCEPTED), ["MIT", "Unicode-3.0"]);
  assert.deepEqual(chooseLicenses("Apache-2.0 WITH LLVM-exception OR Apache-2.0", ACCEPTED), ["Apache-2.0"]);
  // OR の両方とも同じ順位なら、数の少ない方。
  assert.deepEqual(chooseLicenses("(MIT AND ISC) OR ISC", ACCEPTED), ["ISC"]);
  assert.equal(chooseLicenses("GPL-2.0-only", ACCEPTED), null);
  assert.equal(chooseLicenses("MIT AND GPL-2.0-only", ACCEPTED), null);
});

test("about.toml: 対象の OS は RUST_TARGETS と同じで、許可するライセンスは deny.toml の allow に含まれる", () => {
  const toml = aboutToml();
  assert.deepEqual(parseTargets(toml), RUST_TARGETS);
  const accepted = parseAcceptedList(toml);
  assert.ok(accepted.indexOf("MIT") < accepted.indexOf("Apache-2.0"), "MIT を Apache-2.0 より先に選ぶ");
  const deny = fs.readFileSync(path.join(REPO_ROOT, "deny.toml"), "utf8");
  const licenses = deny.slice(deny.indexOf("[licenses]"));
  const allow = parseAcceptedList(licenses.replace(/^allow\s*=/m, "accepted ="));
  assert.deepEqual(
    accepted.filter((a) => !allow.includes(a)),
    [],
    "about.toml の accepted に、deny.toml の allow にないライセンスがある",
  );
});

test("ライセンス・NOTICE のファイルらしい名前", () => {
  for (const n of ["LICENSE", "LICENSE-MIT", "LICENSE_APACHE-2.0", "license-mit", "LICENSE.txt", "COPYING", "UNLICENSE", "LICENCE.md"]) {
    assert.ok(isLicenseFileName(n), n);
  }
  for (const n of ["LICENSE.spdx", "license.rs", "README.md", "licenses.json", "src"]) {
    assert.ok(!isLicenseFileName(n), n);
  }
  for (const n of ["NOTICE", "NOTICE.txt", "NOTICES.md", "THIRD-PARTY-NOTICES"]) assert.ok(isNoticeFileName(n), n);
  assert.ok(!isNoticeFileName("notice.rs"));
});

test("名前でライセンスの本文の候補を選び、中身で確かめる", () => {
  const names = ["LICENSE_APACHE-2.0", "LICENSE_MIT", "README.md", "LICENSE.spdx"];
  assert.deepEqual(pickLicenseFiles(names, "MIT"), ["LICENSE_MIT"]);
  assert.deepEqual(pickLicenseFiles(names, "Apache-2.0 WITH LLVM-exception"), ["LICENSE_APACHE-2.0"]);
  assert.deepEqual(pickLicenseFiles(["license-apache-2.0", "license-mit"], "MIT"), ["license-mit"]);
  // 名前にライセンスが書かれていなければ、LICENSE・LICENSE.txt など。
  assert.deepEqual(pickLicenseFiles(["LICENSE.txt", "LICENSE-APACHE"], "MIT"), ["LICENSE.txt"]);
  assert.deepEqual(pickLicenseFiles(["LICENSE-APACHE"], "MIT"), []);

  assert.ok(textMatchesLicense(MIT("2020 A"), "MIT"));
  assert.ok(!textMatchesLicense(APACHE, "MIT"));
  assert.ok(textMatchesLicense(APACHE, "Apache-2.0"));
  // 名前が LICENSE でも、中身が別のライセンスなら選ばない。
  const files = { LICENSE: APACHE, "LICENSE.txt": `dual\n${MIT("2014 K")}` };
  assert.deepEqual(
    findLicenseTexts(Object.keys(files), "MIT", (n) => files[n]).map((f) => f.name),
    ["LICENSE.txt"],
  );
});

test("本文の改行・末尾をそろえ、版は数として並べる", () => {
  assert.equal(normalizeText("﻿a\r\nb\r\n\r\n  "), "a\nb");
  assert.ok(compareNatural("0.9.0", "0.10.0") < 0);
  assert.ok(compareNatural("windows-sys", "windows") > 0);
  assert.equal(compareNatural("1.2.3", "1.2.3"), 0);
});

/** cargo-about の JSON の形の、テスト用のもの。 */
function aboutReport(crates, licenses) {
  return {
    crates: crates.map((c) => ({ package: c, license: c.license })),
    licenses: licenses.map((l) => ({
      id: l.id,
      text: l.text,
      source_path: l.source_path,
      used_by: l.users.map((id) => ({ crate: { id } })),
    })),
  };
}

const crate = (name, version, license, extra = {}) => ({
  id: `registry#${name}@${version}`,
  name,
  version,
  license,
  manifest_path: `/reg/${name}-${version}/Cargo.toml`,
  authors: [],
  repository: null,
  ...extra,
});

/** フォルダ → { ファイル名: 中身 } の、テスト用の読み取り。 */
function fakeIo(tree) {
  return {
    listDir: (dir) => Object.keys(tree[dir] ?? {}),
    readText: (file) => {
      const dir = file.slice(0, file.lastIndexOf("/"));
      const name = file.slice(file.lastIndexOf("/") + 1);
      const text = tree[dir]?.[name];
      if (text === undefined) throw new Error(`ない: ${file}`);
      return text;
    },
  };
}

test("Rust: cargo-about の本文の出典を crate ごとに決め、取りこぼしを補い、ファイルのない crate に印を付ける", () => {
  const a = crate("aaa", "1.0.0", "MIT OR Apache-2.0");
  const b = crate("bbb", "2.0.0", "MIT OR Apache-2.0");
  const t = crate("tauri", "2.11.6", "Apache-2.0 OR MIT");
  const o = crate("objc2", "0.6.5", "MIT", { authors: ["Mads Marquart <mads@example.com>"], repository: "https://github.com/madsmtm/objc2" });
  const n = crate("cfg_aliases", "0.2.2", "MIT");
  const shared = MIT("2020 Same Text");
  const canonical = MIT("<year> <copyright holders>");
  const report = aboutReport(
    [a, b, t, o, n],
    [
      // aaa と bbb は同じ本文。cargo-about は aaa のファイルだけを source_path に残す。
      { id: "MIT", text: shared, source_path: "/reg/aaa-1.0.0/LICENSE-MIT", users: [a.id, b.id] },
      // tauri は LICENSE_MIT を cargo-about が見つけられず、objc2 はファイルがない（どちらも標準の文）。
      { id: "MIT", text: canonical, source_path: null, users: [t.id, o.id] },
      // cfg_aliases の NOTICES.md を cargo-about が MIT の本文として拾った。
      { id: "MIT", text: MIT("2021 N"), source_path: "/reg/cfg_aliases-0.2.2/LICENSE", users: [n.id] },
      { id: "MIT", text: `# 3rd Party Notices\n${MIT("tectonic")}`, source_path: "/reg/cfg_aliases-0.2.2/NOTICES.md", users: [n.id] },
    ],
  );
  const io = fakeIo({
    "/reg/aaa-1.0.0": { "LICENSE-MIT": shared, "LICENSE-APACHE": APACHE },
    "/reg/bbb-2.0.0": { "LICENSE-MIT": shared },
    "/reg/tauri-2.11.6": { LICENSE_MIT: MIT("2017 - Present Tauri Apps Contributors"), "LICENSE_APACHE-2.0": APACHE },
    "/reg/objc2-0.6.5": {},
    "/reg/cfg_aliases-0.2.2": { LICENSE: MIT("2021 N"), "NOTICES.md": `# 3rd Party Notices\n${MIT("tectonic")}` },
  });
  // 本体とワーカーの両方に同じ crate があっても、1 つにまとめる。
  const model = rustModel([report, report], io);
  const byName = Object.fromEntries(model.packages.map((p) => [p.name, p]));
  assert.deepEqual(Object.keys(byName), ["aaa", "bbb", "cfg_aliases", "objc2", "tauri"]);
  assert.deepEqual(byName.aaa.files, ["LICENSE-MIT"]);
  assert.deepEqual(byName.bbb.files, ["LICENSE-MIT"], "bbb の本文は bbb 自身のファイルから");
  assert.deepEqual(byName.tauri.files, ["LICENSE_MIT"]);
  assert.deepEqual(byName.tauri.standard, []);
  assert.deepEqual(byName.objc2.standard, ["MIT"]);
  assert.deepEqual(byName.cfg_aliases.files, ["LICENSE", "NOTICES.md"]);
  assert.deepEqual(byName.aaa.chosen, ["MIT"]);

  const sharedEntry = model.texts.find((x) => x.text === normalizeText(shared));
  assert.deepEqual(sharedEntry.users, ["aaa 1.0.0", "bbb 2.0.0"]);
  const standard = model.texts.filter((x) => x.standard);
  assert.equal(standard.length, 1);
  assert.deepEqual(standard[0].users, ["objc2 0.6.5"]);
  // NOTICES.md は、cargo-about が本文として拾ったので NOTICE として重ねない。
  assert.equal(model.texts.filter((x) => x.notice).length, 0);
  assert.ok(!model.texts.some((x) => x.text === normalizeText(canonical) && x.users.includes("tauri 2.11.6")));

  const text = formatNotices(model, { kind: "rust", lockFile: "Cargo.lock", lockHash: "a".repeat(64), facts: ["対象: test"] });
  assert.match(text, /^GenzoParis が使っている Rust のライブラリ（crate）のライセンス\n/);
  assert.match(text, /\n  tauri 2\.11\.6: Apache-2\.0 OR MIT → MIT \[\d+\]（LICENSE_MIT）\n/);
  assert.match(text, /\n  objc2 0\.6\.5: MIT \[\d+\]\n/);
  assert.match(text, /\n3\. ライセンスのファイルがない crate \/ Crates without a license file\n/);
  assert.match(text, /\n  objc2 0\.6\.5: MIT\n    authors: Mads Marquart <mads@example\.com>\n    repository: https:\/\/github\.com\/madsmtm\/objc2\n/);
  assert.match(text, /（SPDX の標準の文。ライセンスのファイルがないため/);
  assert.match(text, /\n  対象: test\n/);
  assert.equal(readLockHash(text, "Cargo.lock"), "a".repeat(64));
  assert.equal(readLockHash(text, "ui/package-lock.json"), null);
  assert.ok(text.endsWith("\n") && !text.endsWith("\n\n"));
});

test("ソースのファイルの先頭の注釈（ライセンスの表示）だけを取り出す", () => {
  const rs = [
    "#![allow(clippy::all)]",
    "// Copied from regex_syntax crate",
    "//",
    "// Copyright (c) 2014 The Rust Project Developers",
    "//",
    "//    indented line",
    "",
    "pub fn escape(text: &str) -> String {",
    "    // コードの中の注釈は含めない",
    "}",
  ].join("\n");
  assert.equal(
    leadingComment(rs),
    "Copied from regex_syntax crate\n\nCopyright (c) 2014 The Rust Project Developers\n\n   indented line",
  );
  const c = ["/*", " * // Copyright (c) Radzivon Bartoshyk 6/2025.", " *", " * // Redistribution and use", " */", "mod a;"].join("\r\n");
  assert.equal(leadingComment(c), "Copyright (c) Radzivon Bartoshyk 6/2025.\n\nRedistribution and use");
  assert.equal(leadingComment("/* Copyright 2015 Google Inc. */\nint x;"), "Copyright 2015 Google Inc.");
  assert.equal(leadingComment("fn main() {}\n// late"), "");
  assert.ok(isSourceFileName("src/io.rs") && isSourceFileName("crypto/x.h") && !isSourceFileName("LICENSE.md"));
});

test("Rust: ソースのファイルから拾った本文は先頭の注釈だけ。同じ本文がほかの crate にあるときは、自分のファイルを出典にする", () => {
  const bsd = (holder) => `Copyright (c) ${holder}\n\nRedistribution and use in source and binary forms, with or without modification`;
  const m = crate("moxcms", "0.8.1", "BSD-3-Clause OR Apache-2.0");
  const p = crate("pxfm", "0.1.30", "BSD-3-Clause OR Apache-2.0");
  const shared = bsd("Radzivon Bartoshyk");
  const header = `/*\n * // ${bsd("Radzivon Bartoshyk 7/2025")}\n */\nmod erf;\nfn secret() {}\n`;
  const report = aboutReport(
    [m, p],
    [
      // 同じ作者の LICENSE.md は同じ本文。cargo-about は moxcms のファイルの場所だけを返す。
      { id: "BSD-3-Clause", text: shared, source_path: "/reg/moxcms-0.8.1/LICENSE.md", users: [m.id, p.id] },
      // pxfm のソースのファイル（ファイル全体が本文として返る）。
      { id: "BSD-3-Clause", text: header, source_path: "/reg/pxfm-0.1.30/src/err/mod.rs", users: [p.id] },
    ],
  );
  const io = fakeIo({
    "/reg/moxcms-0.8.1": { "LICENSE.md": shared, "LICENSE-APACHE.md": APACHE },
    "/reg/pxfm-0.1.30": { "LICENSE.md": shared, "LICENSE-APACHE.md": APACHE },
  });
  const model = rustModel([report], io);
  const px = model.packages.find((x) => x.name === "pxfm");
  assert.deepEqual(px.files, ["LICENSE.md", "src/err/mod.rs の先頭の注釈"]);
  assert.deepEqual(px.standard, []);
  const texts = model.texts.filter((t) => t.users.includes("pxfm 0.1.30")).map((t) => t.text);
  assert.ok(texts.includes(normalizeText(shared)));
  assert.ok(texts.includes(bsd("Radzivon Bartoshyk 7/2025")));
  assert.ok(!texts.some((t) => /mod erf|secret/.test(t)), "コードは載せない");

  // 先頭の注釈にライセンスの文がなければ止める（about.toml の clarify で本文のファイルを指定する）。
  const bad = aboutReport([p], [{ id: "BSD-3-Clause", text: "//! docs\nfn f() {}\n", source_path: "/reg/pxfm-0.1.30/src/lib.rs", users: [p.id] }]);
  assert.throws(() => rustModel([bad], io), /pxfm 0\.1\.30 のソースのファイル src\/lib\.rs を BSD-3-Clause の本文として拾った/);
});

test("npm: 配布物に入りうるパッケージは、開発用の印のないもの", () => {
  const lock = {
    lockfileVersion: 3,
    packages: {
      "": { name: "genzo-ui", dependencies: { react: "19.3.0" } },
      "node_modules/react": { version: "19.3.0", license: "MIT" },
      "node_modules/@tauri-apps/api": { version: "2.11.1", license: "Apache-2.0 OR MIT" },
      "node_modules/vite": { version: "8.3.1", license: "MIT", dev: true },
      "node_modules/fsevents": { version: "2.3.3", license: "MIT", dev: true, optional: true },
      "node_modules/x": { version: "1.0.0", devOptional: true },
      "node_modules/a/node_modules/b": { version: "0.1.0", license: "ISC" },
    },
  };
  assert.deepEqual(
    npmDistPackages(lock).map((p) => `${p.path} ${p.name} ${p.version} ${p.license}`),
    [
      "node_modules/@tauri-apps/api @tauri-apps/api 2.11.1 Apache-2.0 OR MIT",
      "node_modules/a/node_modules/b b 0.1.0 ISC",
      "node_modules/react react 19.3.0 MIT",
    ],
  );
  assert.throws(() => npmDistPackages({ lockfileVersion: 1, dependencies: {} }), /lockfileVersion 2 以降/);
});

test("npm: 選んだライセンスの本文をパッケージから取り、なければ上流の写し（SHA-256 を確かめる）を使う", () => {
  const upstream = MIT("2017 - Present Tauri Apps Contributors");
  const sha = createHash("sha256").update(upstream).digest("hex");
  const supplements = {
    "@tauri-apps/plugin-dialog": [
      { version: "2.7.3", license: "MIT", file: "supplements/dialog/LICENSE_MIT", sha256: sha, source: "https://example.com/LICENSE_MIT" },
    ],
  };
  const tree = {
    "node_modules/@tauri-apps/api": { LICENSE_MIT: upstream, "LICENSE_APACHE-2.0": APACHE },
    "node_modules/@tauri-apps/plugin-dialog": { "LICENSE.spdx": "SPDXVersion: SPDX-2.1", "README.md": "" },
    "node_modules/react": { LICENSE: MIT("Meta Platforms, Inc. and affiliates.") },
  };
  const io = {
    ...fakeIo(tree),
    readPackageJson: (dir) => (tree[dir] ? { author: { name: "someone" }, repository: { url: "git+https://x" } } : null),
    readSupplement: (f) => {
      assert.equal(f, "supplements/dialog/LICENSE_MIT");
      return upstream;
    },
  };
  const packages = [
    { path: "node_modules/@tauri-apps/api", name: "@tauri-apps/api", version: "2.11.1", license: "Apache-2.0 OR MIT", optional: false },
    { path: "node_modules/@tauri-apps/plugin-dialog", name: "@tauri-apps/plugin-dialog", version: "2.7.3", license: "MIT OR Apache-2.0", optional: false },
    { path: "node_modules/react", name: "react", version: "19.3.0", license: "MIT", optional: false },
  ];
  const model = npmModel(packages, ACCEPTED, io, supplements);
  const byName = Object.fromEntries(model.packages.map((p) => [p.name, p]));
  assert.deepEqual(byName["@tauri-apps/api"].files, ["LICENSE_MIT"]);
  assert.deepEqual(byName["@tauri-apps/plugin-dialog"].supplemented, ["MIT"]);
  assert.deepEqual(byName["@tauri-apps/plugin-dialog"].files, ["LICENSE_MIT（上流 / upstream）"]);
  assert.deepEqual(model.supplements, ["@tauri-apps/plugin-dialog 2.7.3（MIT）: https://example.com/LICENSE_MIT"]);
  // 同じ本文（api の LICENSE_MIT と上流の写し）は 1 つにまとめる。
  assert.equal(model.texts.length, 2);

  const text = formatNotices(model, { kind: "npm", lockFile: "ui/package-lock.json", lockHash: "b".repeat(64), facts: [] });
  assert.match(text, /\n  @tauri-apps\/api 2\.11\.1: Apache-2\.0 OR MIT → MIT \[1\]（LICENSE_MIT）\n/);
  assert.match(text, /\n3\. 上流のリポジトリから補った本文/);
  assert.equal(readLockHash(text, "ui/package-lock.json"), "b".repeat(64));

  // 写しの中身が変わっていたら止める。
  assert.throws(
    () => npmModel(packages, ACCEPTED, { ...io, readSupplement: () => `${upstream}x` }, supplements),
    /SHA-256 が違う/,
  );
  // 本文も写しもなければ止める（人が写しを足す）。写しは記録した版にだけ使う。
  assert.throws(() => npmModel(packages, ACCEPTED, io, {}), /plugin-dialog 2\.7\.3 に MIT の本文のファイルがない/);
  const newer = packages.map((p) => (p.name === "@tauri-apps/plugin-dialog" ? { ...p, version: "2.8.0" } : p));
  assert.throws(() => npmModel(newer, ACCEPTED, io, supplements), /plugin-dialog 2\.8\.0 に MIT の本文のファイルがない/);
  // 許可していないライセンスなら止める。
  assert.throws(
    () => npmModel([{ ...packages[2], license: "GPL-2.0-only" }], ACCEPTED, io, supplements),
    /accepted で満たせない/,
  );
  // パッケージのフォルダがなければ止める。
  assert.throws(() => npmModel([{ ...packages[2], path: "node_modules/none" }], ACCEPTED, io, supplements), /npm ci/);

  assert.deepEqual(
    missingFromNpmModel(
      [
        { name: "react", version: "19.3.0" },
        { name: "leaflet", version: "1.9.4" },
        { name: "leaflet", version: "1.9.4" },
      ],
      model,
    ),
    ["leaflet 1.9.4"],
  );
});

test("tauri.conf.json の resources に、ライセンスの文書がすべてあり、元のファイルもある", () => {
  const conf = JSON.parse(fs.readFileSync(path.join(APP_DIR, "tauri.conf.json"), "utf8"));
  assert.deepEqual(conf.bundle.resources, STATIC_LICENSE_RESOURCES);
  for (const src of Object.keys(STATIC_LICENSE_RESOURCES)) {
    assert.ok(fs.existsSync(path.join(APP_DIR, src)), `${src} がない`);
  }
  assert.deepEqual(
    [RUST_NOTICES_FILE, NPM_NOTICES_FILE].map((f) => `${THIRD_PARTY_SOURCE_DIR}/${f}`).filter((f) => !(f in STATIC_LICENSE_RESOURCES)),
    [],
  );
  // アプリ自体のライセンスの全文（GPL-3.0）。
  assert.match(fs.readFileSync(path.join(REPO_ROOT, "LICENSE"), "utf8"), /GNU GENERAL PUBLIC LICENSE\s+Version 3, 29 June 2007/);
});

test("lock のファイルの SHA-256 は、改行が CRLF でも同じ", () => {
  assert.equal(lockHash("a\r\nb\n"), lockHash("a\nb\n"));
  assert.notEqual(lockHash("a\nb\n"), lockHash("a\nc\n"));
});

test("上流の写しは、記録した SHA-256 のとおり（改行を変えずに置いてある）", () => {
  for (const [name, list] of Object.entries(NPM_SUPPLEMENTS)) {
    for (const s of list) {
      assert.match(s.version, /^\d+\.\d+\.\d+/, `${name}: 版を固定する`);
      const file = path.join(APP_DIR, THIRD_PARTY_SOURCE_DIR, s.file);
      const sha = createHash("sha256").update(fs.readFileSync(file)).digest("hex");
      assert.equal(sha, s.sha256, `${name}: ${s.file}`);
      assert.match(s.source, /^https:\/\/github\.com\/.+\/blob\/[0-9a-f]{40}\//, "出典はコミットを固定した URL");
    }
  }
});

// リポジトリの文書が、今の lock のファイルから作ったものか（cargo-about がなくても確かめられる部分）。中身まで
// 同じかは、CI の third-party ジョブ（node scripts/third-party.mjs --check）で確かめる。
test("第三者のライセンスの文書は、今の Cargo.lock・ui/package-lock.json から作ったもの", () => {
  const dir = path.join(APP_DIR, THIRD_PARTY_SOURCE_DIR);
  const hint = "Cargo.lock・ui/package-lock.json を変えたら node scripts/third-party.mjs を実行して、作り直した文書をコミットする";
  const rust = fs.readFileSync(path.join(dir, RUST_NOTICES_FILE), "utf8");
  assert.equal(readLockHash(rust, "Cargo.lock"), lockHash(fs.readFileSync(path.join(REPO_ROOT, "Cargo.lock"), "utf8")), hint);
  const npm = fs.readFileSync(path.join(dir, NPM_NOTICES_FILE), "utf8");
  assert.equal(
    readLockHash(npm, "ui/package-lock.json"),
    lockHash(fs.readFileSync(path.join(REPO_ROOT, "ui", "package-lock.json"), "utf8")),
    hint,
  );
});
