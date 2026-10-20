// native-notice.mjs のテスト（node --test）。

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";

import { APP_DIR, REPO_ROOT } from "./proc.mjs";

import {
  IJG_STATEMENT,
  VCRUNTIME_TERMS_SOURCE,
  VCRUNTIME_TERMS_TARGET,
  installerLicenseText,
  changedVcpkgPortFiles,
  formatNativeNotices,
  githubArchiveUrl,
  homebrewFormulaInfo,
  isNativeLicenseFile,
  mergeComponents,
  needsIjgStatement,
  parseCellarPath,
  parseVcpkgAbiInfo,
  parseVcpkgFromGithub,
  parseVcpkgListName,
  vcRedistFolderLabel,
  vcpkgArchiveName,
  vcpkgDllOwners,
  vcpkgPortInfo,
} from "./native-notice.mjs";

test("vcpkg: info の一覧から、DLL の持ち主の port を決める", () => {
  assert.deepEqual(parseVcpkgListName("libraw_0.22.2_x64-windows.list", "x64-windows"), { port: "libraw", version: "0.22.2" });
  assert.deepEqual(parseVcpkgListName("lcms_2.19.1#1_x64-windows.list", "x64-windows"), { port: "lcms", version: "2.19.1#1" });
  assert.deepEqual(parseVcpkgListName("vcpkg-cmake_2024-04-23_x64-windows.list", "x64-windows"), {
    port: "vcpkg-cmake",
    version: "2024-04-23",
  });
  assert.equal(parseVcpkgListName("zlib_1.3.2_x64-windows-static.list", "x64-windows"), null);
  assert.equal(parseVcpkgListName("status", "x64-windows"), null);

  const owners = vcpkgDllOwners(
    [
      { name: "libraw_0.22.2_x64-windows.list", text: "x64-windows/\nx64-windows/bin/\nx64-windows/bin/raw.dll\nx64-windows/bin/raw_r.dll\nx64-windows/debug/bin/raw_rd.dll\n" },
      { name: "zlib_1.3.2_x64-windows.list", text: "x64-windows/bin/z.dll\r\nx64-windows/share/zlib/copyright\r\n" },
      { name: "zlib_1.3.2_arm64-windows.list", text: "arm64-windows/bin/z.dll\n" },
    ],
    "x64-windows",
  );
  assert.deepEqual([...owners.entries()].sort(), [
    ["raw.dll", { port: "libraw", version: "0.22.2" }],
    ["raw_r.dll", { port: "libraw", version: "0.22.2" }],
    ["z.dll", { port: "zlib", version: "1.3.2" }],
  ]);
  assert.deepEqual(vcpkgPortInfo({ name: "lcms", version: "2.19.1", "port-version": 1, license: "MIT", homepage: "https://x" }), {
    license: "MIT",
    homepage: "https://x",
    version: "2.19.1#1",
  });
  assert.deepEqual(vcpkgPortInfo({}), { license: null, homepage: null, version: null });
});

test("vcpkg: ABI の情報の port のファイルの SHA-256 と、今の ports のファイルを比べる", () => {
  const abi = parseVcpkgAbiInfo(
    "cmake 3.31.6\nfeatures core\nportfile.cmake AAA111\npatches\\fix.patch bbb222\nvcpkg.json ccc333\nvcpkg_from_github 0123\r\n",
  );
  assert.equal(abi.get("portfile.cmake"), "AAA111");
  assert.equal(abi.get("vcpkg_from_github"), "0123");
  const files = [
    { file: "portfile.cmake", sha256: "aaa111" },
    { file: "patches/fix.patch", sha256: "bbb222" },
    { file: "vcpkg.json", sha256: "ccc333" },
  ];
  assert.deepEqual(changedVcpkgPortFiles(abi, files), []);
  // 中身が変わったファイル・記録のないファイルを返す。
  assert.deepEqual(
    changedVcpkgPortFiles(abi, [...files.slice(0, 2), { file: "vcpkg.json", sha256: "ddd444" }, { file: "usage", sha256: "eee" }]),
    ["usage", "vcpkg.json"],
  );
});

test("vcpkg: portfile の vcpkg_from_github を読み、downloads のファイルの名前と URL を作る", () => {
  const portfile = [
    "vcpkg_from_github(",
    "    OUT_SOURCE_PATH SOURCE_PATH",
    "    REPO LibRaw/LibRaw",
    '    REF "${VERSION}"',
    "    SHA512 24313FBD",
    "    HEAD_REF master",
    ")",
    "",
    "vcpkg_from_github(",
    "    OUT_SOURCE_PATH LIBRAW_CMAKE_SOURCE_PATH",
    "    REPO LibRaw/LibRaw-cmake",
    "    REF eb98e4325aef2ce85d2eb031c2ff18640ca616d3",
    "    SHA512 63e68a4d",
    "    HEAD_REF master",
    "    PATCHES",
    "        dependencies.patch",
    "        # Move the non-thread-safe library to manual-link. (注釈の中の括弧)",
    "        fix-install.patch",
    ")",
  ].join("\n");
  assert.deepEqual(parseVcpkgFromGithub(portfile, "0.22.2"), [
    { repo: "LibRaw/LibRaw", ref: "0.22.2", sha512: "24313fbd", patches: [] },
    {
      repo: "LibRaw/LibRaw-cmake",
      ref: "eb98e4325aef2ce85d2eb031c2ff18640ca616d3",
      sha512: "63e68a4d",
      patches: ["dependencies.patch", "fix-install.patch"],
    },
  ]);
  assert.throws(() => parseVcpkgFromGithub("vcpkg_from_github(REPO a/b REF ${OTHER} SHA512 1)", "1"), /REF の変数/);
  assert.throws(() => parseVcpkgFromGithub("vcpkg_from_github(REPO a/b SHA512 1)", "1"), /REF が読めない/);
  assert.equal(vcpkgArchiveName("LibRaw/LibRaw", "0.22.2"), "LibRaw-LibRaw-0.22.2.tar.gz");
  assert.equal(vcpkgArchiveName("a/b", "release/1.0"), "a-b-release_-1.0.tar.gz");
  assert.equal(githubArchiveUrl("LibRaw/LibRaw", "0.22.2"), "https://github.com/LibRaw/LibRaw/archive/0.22.2.tar.gz");
});

test("Visual C++ の再頒布用のフォルダは、VC\\Redist\\MSVC より後だけを書く（絶対パスを書かない）", () => {
  assert.equal(
    vcRedistFolderLabel("C:\\Program Files\\Microsoft Visual Studio\\18\\Enterprise\\VC\\Redist\\MSVC\\14.51.36231\\x64\\Microsoft.VC145.CRT"),
    "14.51.36231\\x64\\Microsoft.VC145.CRT",
  );
  assert.equal(vcRedistFolderLabel("D:\\Users\\someone\\redist"), "redist");
  assert.equal(vcRedistFolderLabel(""), null);
});

test("Homebrew: Cellar の場所から formula を読み、keg のライセンスの文書を選ぶ", () => {
  assert.deepEqual(parseCellarPath("/opt/homebrew/Cellar/jpeg-turbo/3.2.0/lib/libjpeg.8.3.2.dylib"), {
    formula: "jpeg-turbo",
    version: "3.2.0",
    keg: "/opt/homebrew/Cellar/jpeg-turbo/3.2.0",
  });
  assert.equal(parseCellarPath("/usr/lib/libz.1.dylib"), null);
  for (const n of ["LICENSE.md", "COPYING", "COPYRIGHT", "LICENSE.TXT", "LICENSE.LGPL", "LICENSE.CDDL", "NOTICE", "README.ijg"]) {
    assert.ok(isNativeLicenseFile(n), n);
  }
  for (const n of ["README.md", "ChangeLog", "INSTALL_RECEIPT.json", "libjpeg.txt", "example.c"]) {
    assert.ok(!isNativeLicenseFile(n), n);
  }
  const info = homebrewFormulaInfo({
    license: "IJG AND Zlib AND BSD-3-Clause",
    homepage: "https://www.libjpeg-turbo.org/",
    tap_git_head: "abc123",
    ruby_source_path: "Formula/j/jpeg-turbo.rb",
    versions: { stable: "3.2.0" },
    urls: { stable: { url: "https://example.com/libjpeg-turbo-3.2.0.tar.gz", checksum: "6F30" } },
    installed: [{ version: "3.2.0" }],
  });
  assert.deepEqual(info, {
    license: "IJG AND Zlib AND BSD-3-Clause",
    homepage: "https://www.libjpeg-turbo.org/",
    recipe: "https://github.com/Homebrew/homebrew-core/blob/abc123/Formula/j/jpeg-turbo.rb",
    source: "https://example.com/libjpeg-turbo-3.2.0.tar.gz",
    sourceSha256: "6f30",
    stable: "3.2.0",
    kegVersion: "3.2.0",
    installed: ["3.2.0"],
  });
  assert.equal(homebrewFormulaInfo({ versions: { stable: "2.19.1" }, revision: 1 }).kegVersion, "2.19.1_1");
  assert.equal(homebrewFormulaInfo({ versions: { stable: "2.19.1" }, revision: 0 }).kegVersion, "2.19.1");
  assert.equal(homebrewFormulaInfo({}).recipe, null);
});

test("文書: パッケージごとにまとめ、libjpeg を含むときは IJG の一文を書き、Visual C++ のランタイムを書く", () => {
  const base = { license: null, homepage: null, recipe: null, source: null };
  const text = formatNativeNotices({
    os: "macos",
    triple: "aarch64-apple-darwin",
    manager: "Homebrew",
    components: [
      { ...base, name: "libraw", version: "0.22.2", license: "LGPL-2.1-only OR CDDL-1.0", files: ["libraw_r.25.dylib"], texts: [{ file: "COPYRIGHT", text: "(C) LibRaw LLC\r\n" }] },
      {
        ...base,
        name: "jpeg-turbo",
        version: "3.2.0",
        recipe: "https://github.com/Homebrew/homebrew-core/blob/abc/Formula/j/jpeg-turbo.rb",
        files: ["libjpeg.8.dylib"],
        texts: [
          { file: "LICENSE.md", text: "libjpeg-turbo Licenses" },
          { file: "share/doc/libjpeg-turbo/LICENSE.md", text: "libjpeg-turbo Licenses\n" },
          { file: "share/doc/libjpeg-turbo/README.ijg", text: "The Independent JPEG Group's JPEG software\n\nLEGAL ISSUES\n============" },
        ],
      },
    ],
    vcRuntime: null,
  });
  assert.match(text, /^GenzoParis に同梱した C \/ C\+\+ のライブラリのライセンス\n/);
  assert.ok(text.includes(`This software is based in part on the work of the Independent JPEG Group.`));
  assert.equal(IJG_STATEMENT, "this software is based in part on the work of the Independent JPEG Group");
  assert.match(text, /\n  libjpeg\.8\.dylib: jpeg-turbo 3\.2\.0\n  libraw_r\.25\.dylib: libraw 0\.22\.2\n/);
  assert.match(text, /\n\[1\] jpeg-turbo 3\.2\.0\n/);
  assert.match(text, /\n\[2\] libraw 0\.22\.2\n  ライセンス \/ License: LGPL-2\.1-only OR CDDL-1\.0\n/);
  assert.match(text, /LibRaw の案内とソースは licenses\/LibRaw にあります/);
  // 同じ中身の LICENSE.md（keg の直下と share/doc）は 1 つだけ載せる。
  assert.equal(text.split("libjpeg-turbo Licenses").length - 1, 1);
  assert.match(text, /（share\/doc\/libjpeg-turbo\/README\.ijg）\n\nThe Independent JPEG Group's JPEG software\n/);
  // IJG のライセンスの本文（README.ijg）がなければ止める。
  assert.throws(
    () =>
      formatNativeNotices({
        os: "macos",
        triple: "aarch64-apple-darwin",
        manager: "Homebrew",
        components: [{ ...base, name: "jpeg-turbo", version: "3.2.0", files: ["libjpeg.8.dylib"], texts: [{ file: "LICENSE.md", text: "see README.ijg" }] }],
        vcRuntime: null,
      }),
    /jpeg-turbo 3\.2\.0 の IJG のライセンスの本文（README\.ijg）が見つからない/,
  );
  assert.ok(!text.includes("\r"));
  assert.ok(!text.includes("Microsoft"));
  assert.ok(text.endsWith("\n") && !text.endsWith("\n\n"));

  const win = formatNativeNotices({
    os: "windows",
    triple: "x86_64-pc-windows-msvc",
    manager: "vcpkg",
    components: [
      { ...base, name: "zlib", version: "1.3.2", license: "Zlib", files: ["z.dll"], texts: [{ file: "share/zlib/copyright", text: "zlib license" }] },
      { ...base, name: "libraw", version: "0.22.2", files: ["raw_r.dll"], texts: [{ file: "share/libraw/copyright", text: "LibRaw" }] },
      { ...base, name: "libraw", version: "0.22.2", files: ["raw.dll"], texts: [{ file: "share/libraw/copyright", text: "LibRaw" }] },
    ],
    vcRuntime: { files: ["VCRUNTIME140.dll", "MSVCP140.dll"], folders: ["14.51.36231\\x64\\Microsoft.VC145.CRT"] },
  });
  assert.ok(!win.includes("Independent JPEG Group"));
  assert.match(win, /\n  同梱したファイル \/ Bundled files: raw\.dll, raw_r\.dll\n/);
  assert.match(win, /\n  MSVCP140\.dll: Microsoft Visual C\+\+ のランタイム/);
  assert.match(win, /\n3\. Microsoft Visual C\+\+ のランタイム \/ Microsoft Visual C\+\+ Runtime\n/);
  assert.match(win, /VC\\Redist\\MSVC\\14\.51\.36231\\x64\\Microsoft\.VC145\.CRT/);
  assert.ok(win.includes(`使用条件 / Terms of use: ${VCRUNTIME_TERMS_TARGET}`));
  assert.match(win, /ライセンス \/ License: （入手元に表示なし/);
});

test("IJG の一文は libjpeg・libjpeg-turbo（またはライセンスの表示が IJG）のときだけ", () => {
  const c = (name, license = null) => ({ name, version: "1", license, homepage: null, recipe: null, source: null, files: [], texts: [] });
  assert.ok(needsIjgStatement([c("jpeg-turbo")]));
  assert.ok(needsIjgStatement([c("libjpeg-turbo")]));
  assert.ok(needsIjgStatement([c("jpeg")]));
  assert.ok(needsIjgStatement([c("something", "IJG AND Zlib")]));
  assert.ok(!needsIjgStatement([c("openjpeg", "BSD-2-Clause")]));
  assert.ok(!needsIjgStatement([c("jasper", "JasPer-2.0")]));
});

test("文書: ライセンスの文書が見つからないパッケージがあれば止める", () => {
  assert.throws(
    () =>
      mergeComponents([
        { name: "libomp", version: "23.1.3", license: "MIT", homepage: null, recipe: null, source: null, files: ["libomp.dylib"], texts: [] },
      ]),
    /libomp\.dylib（libomp 23\.1\.3）のライセンスの文書が見つからない/,
  );
});


test("Visual C++ のランタイムの使用条件と、インストーラーの同意の画面の文書", () => {
  const terms = fs.readFileSync(path.join(APP_DIR, VCRUNTIME_TERMS_SOURCE), "utf8");
  // Microsoft の Distributable Code の条件に対応する条項（docs/license_compliance.md の 4 章の 1）。
  for (const phrase of [
    "GenzoParis を動かすためにだけ使う",
    "リバースエンジニアリング",
    "表示を、取り除く",
    "GenzoParis から切り離して単独で配布",
    "この条件と同じ以上に Microsoft のファイルを保護する条件",
    "輸出・制裁に関する法令",
    "あなたに認める権利を制限するものではありません",
    "5.00 米ドル",
    "text prevails",
  ]) {
    assert.ok(terms.includes(phrase), phrase);
  }
  assert.ok(!terms.includes("\r"), "リポジトリの文書は LF");

  const text = installerLicenseText(terms, "GNU GENERAL PUBLIC LICENSE\nVersion 3\n");
  assert.ok(text.startsWith("GenzoParis のインストール / Installing GenzoParis\r\n"));
  assert.ok(!/[^\r]\n/.test(text), "改行はすべて CRLF");
  assert.ok(text.endsWith("Version 3\r\n"));
  assert.ok(text.indexOf("Microsoft Visual C++ のランタイムのファイルの使用条件") < text.indexOf("GNU GENERAL PUBLIC LICENSE"));
  assert.match(text, /「同意する」は、1 の条件への同意です。/);
  assert.match(text, /GPL-3\.0 の 9 条/);
  // リポジトリの GPL-3.0 の全文を、そのまま後ろに付けられる。
  const gpl = fs.readFileSync(path.join(REPO_ROOT, "LICENSE"), "utf8");
  assert.match(installerLicenseText(terms, gpl), /END OF TERMS AND CONDITIONS/);
});
