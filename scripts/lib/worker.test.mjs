// worker.mjs のテスト（node --test）。Windows の Visual Studio の再頒布用のフォルダの探し方を、偽のフォルダで確かめる。

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import {
  collectLibrawRecipe,
  collectLibrawSource,
  findVcRedistDirs,
  nativeComponentsMacos,
  nativeComponentsWindows,
  readLibrawVersion,
  setupWindowsLibraw,
} from "./worker.mjs";

test("Visual C++ のランタイムを探すフォルダ（環境変数の指定と VCToolsRedistDir）", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "genzo-vcredist-"));
  try {
    // 環境変数 GENZO_VC_REDIST_DIR はそのまま使う（複数は path.delimiter で区切る）。
    assert.deepEqual(findVcRedistDirs({ GENZO_VC_REDIST_DIR: ["a", "b"].join(path.delimiter) }, "x86_64-pc-windows-msvc"), [
      "a",
      "b",
    ]);

    // VCToolsRedistDir の <アーキテクチャ> の下の Microsoft.VC*.* を使う（デバッグ版・ほかのアーキテクチャは除く）。
    const version = path.join(root, "VC", "Redist", "MSVC", "14.44.35112");
    for (const d of ["x64/Microsoft.VC143.CRT", "x64/Microsoft.VC143.OpenMP", "x64/Microsoft.VC143.DebugCRT", "arm64/Microsoft.VC143.CRT"]) {
      fs.mkdirSync(path.join(version, d), { recursive: true });
    }
    fs.mkdirSync(path.join(version, "x64", "onecore"), { recursive: true });
    const dirs = findVcRedistDirs({ VCToolsRedistDir: version }, "x86_64-pc-windows-msvc").sort();
    assert.deepEqual(dirs, [path.join(version, "x64", "Microsoft.VC143.CRT"), path.join(version, "x64", "Microsoft.VC143.OpenMP")]);
    assert.deepEqual(findVcRedistDirs({ VCToolsRedistDir: version }, "aarch64-pc-windows-msvc"), [
      path.join(version, "arm64", "Microsoft.VC143.CRT"),
    ]);

    // どこにもなければ（vswhere もなければ）止まる。
    assert.throws(
      () => findVcRedistDirs({ VCToolsRedistDir: path.join(root, "none"), "ProgramFiles(x86)": root }, "x86_64-pc-windows-msvc"),
      /vswhere が見つからない/,
    );
    assert.throws(() => findVcRedistDirs({}, "riscv64gc-unknown-linux-gnu"), /アーキテクチャが分からない/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("LibRaw の版のヘッダを、build.rs と同じ include の置き方で探す（Windows の LIBRAW_INCLUDE_DIR）", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "genzo-libraw-"));
  try {
    const header = "#define LIBRAW_MAJOR_VERSION 0\n#define LIBRAW_MINOR_VERSION 22\n#define LIBRAW_PATCH_VERSION 1\n";
    // libraw/libraw.h のあるフォルダ（vcpkg の include）。
    fs.mkdirSync(path.join(root, "a", "libraw"), { recursive: true });
    fs.writeFileSync(path.join(root, "a", "libraw", "libraw_version.h"), header);
    assert.equal(readLibrawVersion("windows", { LIBRAW_INCLUDE_DIR: path.join(root, "a") }), "0.22.1");
    // libraw.h を直接含むフォルダ。
    fs.mkdirSync(path.join(root, "b"));
    fs.writeFileSync(path.join(root, "b", "libraw_version.h"), header);
    assert.equal(readLibrawVersion("windows", { LIBRAW_INCLUDE_DIR: path.join(root, "b") }), "0.22.1");
    assert.throws(() => readLibrawVersion("windows", { LIBRAW_INCLUDE_DIR: path.join(root, "none") }), /LibRaw の版が読めない/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("LibRaw をビルドした手順: vcpkg の port の写しと版（git のないところではコミットなし）", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "genzo-recipe-"));
  try {
    const vcpkg = path.join(root, "vcpkg");
    const port = path.join(vcpkg, "ports", "libraw");
    fs.mkdirSync(path.join(port, "sub"), { recursive: true });
    fs.writeFileSync(path.join(port, "vcpkg.json"), JSON.stringify({ name: "libraw", version: "0.22.0", "port-version": 1 }));
    fs.writeFileSync(path.join(port, "portfile.cmake"), "# portfile");
    fs.writeFileSync(path.join(port, "sub", "fix.patch"), "--- a\n+++ b\n");
    const out = path.join(root, "out");
    fs.mkdirSync(out);
    const r = collectLibrawRecipe("windows", out, vcpkg, { PATH: process.env.PATH, GIT_CEILING_DIRECTORIES: root });
    assert.equal(r.recipe.kind, "vcpkg");
    assert.equal(r.recipe.version, "0.22.0#1");
    assert.equal(r.recipe.copied, true);
    assert.deepEqual(r.files, ["vcpkg-ports-libraw/portfile.cmake", "vcpkg-ports-libraw/sub/fix.patch", "vcpkg-ports-libraw/vcpkg.json"]);
    assert.equal(fs.readFileSync(path.join(out, "vcpkg-ports-libraw", "sub", "fix.patch"), "utf8"), "--- a\n+++ b\n");
    // vcpkg を使っていない（LIBRAW_INCLUDE_DIR などを自分で指定した）なら、手順は分からない。
    assert.deepEqual(collectLibrawRecipe("windows", out, null, {}), { recipe: { kind: "unknown" }, files: [] });
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("Windows: 同梱した DLL を入れた vcpkg の port と著作権の文書を調べ、LibRaw のソースを downloads から集める", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "genzo-vcpkg-"));
  const out = fs.mkdtempSync(path.join(os.tmpdir(), "genzo-source-"));
  try {
    const write = (rel, text) => {
      const f = path.join(root, ...rel.split("/"));
      fs.mkdirSync(path.dirname(f), { recursive: true });
      fs.writeFileSync(f, text);
    };
    write("installed/vcpkg/info/libraw_0.22.2_x64-windows.list", "x64-windows/bin/raw_r.dll\nx64-windows/share/libraw/copyright\n");
    write("installed/vcpkg/info/zlib_1.3.2_x64-windows.list", "x64-windows/bin/z.dll\n");
    write("installed/x64-windows/share/libraw/copyright", "Copyright (C) 2008-2024 LibRaw LLC\n");
    write("installed/x64-windows/share/zlib/copyright", "Copyright (C) 1995-2024 Jean-loup Gailly and Mark Adler\n");
    write("ports/libraw/vcpkg.json", JSON.stringify({ name: "libraw", version: "0.22.2", license: "(LGPL-2.1-only OR CDDL-1.0) AND BSD-3-Clause", homepage: "https://www.libraw.org" }));
    write("ports/zlib/vcpkg.json", JSON.stringify({ name: "zlib", version: "1.3.2", "port-version": 2, license: "Zlib" }));
    const archive = "LibRaw source archive";
    const cmake = "LibRaw-cmake archive";
    const sha512 = (t) => createHash("sha512").update(t).digest("hex");
    write("downloads/LibRaw-LibRaw-0.22.2.tar.gz", archive);
    write("downloads/LibRaw-LibRaw-cmake-eb98e43.tar.gz", cmake);
    write(
      "ports/libraw/portfile.cmake",
      `vcpkg_from_github(\n OUT_SOURCE_PATH SOURCE_PATH\n REPO LibRaw/LibRaw\n REF "\${VERSION}"\n SHA512 ${sha512(archive)}\n HEAD_REF master\n)\n` +
        `vcpkg_from_github(\n OUT_SOURCE_PATH C\n REPO LibRaw/LibRaw-cmake\n REF eb98e43\n SHA512 ${sha512(cmake)}\n PATCHES a.patch\n)\n`,
    );

    // vcpkg がビルドしたときの port のファイルの SHA-256（vcpkg_abi_info.txt）。
    const sha256 = (rel) => createHash("sha256").update(fs.readFileSync(path.join(root, ...rel.split("/")))).digest("hex");
    write(
      "installed/x64-windows/share/libraw/vcpkg_abi_info.txt",
      `cmake 3.31\nportfile.cmake ${sha256("ports/libraw/portfile.cmake")}\nvcpkg.json ${sha256("ports/libraw/vcpkg.json")}\n`,
    );
    write("installed/x64-windows/share/zlib/vcpkg_abi_info.txt", `vcpkg.json ${sha256("ports/zlib/vcpkg.json")}\n`);

    const libs = [
      { name: "raw_r.dll", source: path.join(root, "installed", "x64-windows", "bin", "raw_r.dll") },
      { name: "Z.dll", source: path.join(root, "installed", "x64-windows", "bin", "z.dll") },
    ];
    const components = nativeComponentsWindows(libs, root, "x64-windows", {});
    assert.deepEqual(
      components.map((c) => [c.name, c.version, c.license, c.files, c.texts.map((t) => t.file)]),
      [
        ["libraw", "0.22.2", "(LGPL-2.1-only OR CDDL-1.0) AND BSD-3-Clause", ["raw_r.dll"], ["share/libraw/copyright"]],
        ["zlib", "1.3.2", "Zlib", ["Z.dll"], ["share/zlib/copyright"]],
      ],
    );
    assert.match(components[1].texts[0].text, /Jean-loup Gailly/);
    // 持ち主の分からない DLL・vcpkg でない LibRaw では止める。
    assert.throws(
      () => nativeComponentsWindows([{ name: "other.dll", source: path.join(root, "installed", "x64-windows", "bin", "other.dll") }], root, "x64-windows", {}),
      /other\.dll を入れた vcpkg の port が分からない/,
    );
    // 同じ名前でも、vcpkg の bin の外の DLL には vcpkg の文書を付けない。
    assert.throws(
      () => nativeComponentsWindows([{ name: "raw_r.dll", source: path.join(root, "elsewhere", "raw_r.dll") }], root, "x64-windows", {}),
      /raw_r\.dll（.*）が vcpkg の .* の中にない/,
    );
    assert.throws(() => nativeComponentsWindows(libs, null, "x64-windows", {}), /vcpkg の LibRaw を使う/);
    // ports のファイルが、入っているものをビルドしたときと違えば止める（vcpkg を更新して入れ直していない）。
    const zlibManifest = fs.readFileSync(path.join(root, "ports/zlib/vcpkg.json"), "utf8");
    write("ports/zlib/vcpkg.json", JSON.stringify({ name: "zlib", version: "1.3.2", "port-version": 3, license: "Zlib" }));
    assert.throws(() => nativeComponentsWindows(libs, root, "x64-windows", {}), /入っている zlib が違う.*ビルドしたときと違うファイル: vcpkg\.json/);
    write("ports/zlib/vcpkg.json", JSON.stringify({ name: "zlib", version: "1.3.3", license: "Zlib" }));
    assert.throws(() => nativeComponentsWindows(libs, root, "x64-windows", {}), /入っている版 1\.3\.2、ports の版 1\.3\.3/);
    write("ports/zlib/vcpkg.json", zlibManifest);
    assert.equal(nativeComponentsWindows(libs, root, "x64-windows", {}).length, 2);

    assert.deepEqual(collectLibrawSource("windows", out, components, root, {}), [
      "LibRaw-LibRaw-0.22.2.tar.gz",
      "LibRaw-LibRaw-cmake-eb98e43.tar.gz",
    ]);
    assert.equal(fs.readFileSync(path.join(out, "LibRaw-LibRaw-0.22.2.tar.gz"), "utf8"), archive);

    // downloads のファイルが portfile の SHA512 と違えば止める（置いたファイルは消す）。
    write("downloads/LibRaw-LibRaw-0.22.2.tar.gz", "changed");
    assert.throws(() => collectLibrawSource("windows", out, components, root, {}), /LibRaw-LibRaw-0\.22\.2\.tar\.gz の SHA512 が違う/);
    assert.ok(!fs.existsSync(path.join(out, "LibRaw-LibRaw-0.22.2.tar.gz")));
    // 入っている版と ports の版が違えば止める。
    const older = components.map((c) => (c.name === "libraw" ? { ...c, version: "0.22.1" } : c));
    assert.throws(() => collectLibrawSource("windows", out, older, root, {}), /0\.22\.1 が入っているが、ports の版は 0\.22\.2/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
    fs.rmSync(out, { recursive: true, force: true });
  }
});

test("macOS: 同梱した dylib を入れた Homebrew の formula と keg の文書を調べ、LibRaw のソースを集める（偽の brew）", { skip: process.platform === "win32" }, () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "genzo-brew-"));
  const out = path.join(root, "out");
  try {
    const write = (rel, text) => {
      const f = path.join(root, ...rel.split("/"));
      fs.mkdirSync(path.dirname(f), { recursive: true });
      fs.writeFileSync(f, text);
    };
    write("Cellar/jpeg-turbo/3.2.0/LICENSE.md", "libjpeg-turbo Licenses");
    write("Cellar/jpeg-turbo/3.2.0/README.md", "readme");
    write("Cellar/jpeg-turbo/3.2.0/share/doc/libjpeg-turbo/README.ijg", "The Independent JPEG Group's JPEG software");
    write("Cellar/jpeg-turbo/3.2.0/share/doc/libjpeg-turbo/example.c", "int main");
    write("Cellar/libraw/0.22.2_1/COPYRIGHT", "LibRaw LLC");
    write("Cellar/libraw/0.22.2_1/LICENSE.LGPL", "GNU LESSER");
    fs.writeFileSync(path.join(root, "Cellar/libraw/0.22.2_1/LICENSE.CDDL"), Buffer.from([0x43, 0x44, 0x44, 0x4c, 0x20, 0xa9]));
    const archive = "LibRaw-0.22.2 archive";
    write("cache/LibRaw-0.22.2.tar.gz", archive);
    const formula = (name, version, extra = {}) => ({
      name,
      license: name === "libraw" ? "LGPL-2.1-only OR CDDL-1.0" : "IJG AND Zlib AND BSD-3-Clause",
      homepage: `https://example.com/${name}`,
      tap_git_head: "abc123",
      ruby_source_path: `Formula/${name[0]}/${name}.rb`,
      versions: { stable: version },
      urls: { stable: { url: `https://example.com/${name}-${version}.tar.gz`, checksum: "0" } },
      ...extra,
    });
    const info = {
      formulae: [
        formula("jpeg-turbo", "3.2.0"),
        formula("libraw", "0.22.2", {
          revision: 1,
          urls: {
            stable: {
              url: "https://www.libraw.org/data/LibRaw-0.22.2.tar.gz",
              checksum: createHash("sha256").update(archive).digest("hex"),
            },
          },
        }),
      ],
    };
    write("info.json", JSON.stringify(info));
    const bin = path.join(root, "bin");
    write(
      "bin/brew",
      `#!/bin/sh\nif [ "$1" = "info" ]; then cat "${path.join(root, "info.json")}"; exit 0; fi\n` +
        `if [ "$1" = "--cache" ]; then echo "${path.join(root, "cache", "LibRaw-0.22.2.tar.gz")}"; exit 0; fi\nexit 1\n`,
    );
    fs.chmodSync(path.join(bin, "brew"), 0o755);
    const env = { ...process.env, PATH: `${bin}${path.delimiter}${process.env.PATH}` };

    const libs = [
      { name: "libraw_r.25.dylib", source: path.join(root, "Cellar/libraw/0.22.2_1/lib/libraw_r.25.dylib") },
      { name: "libjpeg.8.dylib", source: path.join(root, "Cellar/jpeg-turbo/3.2.0/lib/libjpeg.8.3.2.dylib") },
    ];
    const components = nativeComponentsMacos(libs, env);
    assert.deepEqual(
      components.map((c) => [c.name, c.version, c.recipe, c.texts.map((t) => t.file).sort()]),
      [
        ["libraw", "0.22.2_1", "https://github.com/Homebrew/homebrew-core/blob/abc123/Formula/l/libraw.rb", ["COPYRIGHT", "LICENSE.CDDL", "LICENSE.LGPL"]],
        ["jpeg-turbo", "3.2.0", "https://github.com/Homebrew/homebrew-core/blob/abc123/Formula/j/jpeg-turbo.rb", ["LICENSE.md", "share/doc/libjpeg-turbo/README.ijg"]],
      ],
    );
    // UTF-8 でない文書は Latin-1 として読む。
    assert.equal(components[0].texts.find((t) => t.file === "LICENSE.CDDL").text, "CDDL ©");
    assert.equal(components[1].source, "https://example.com/jpeg-turbo-3.2.0.tar.gz");
    assert.throws(() => nativeComponentsMacos([{ name: "x.dylib", source: "/usr/local/lib/x.dylib" }], env), /Cellar の中にない/);
    // 同梱する keg が今の formula と違う版なら止める（ライセンスの表示・formula の URL が食い違うため）。
    write("Cellar/jpeg-turbo/3.1.0/LICENSE.md", "old");
    assert.throws(
      () => nativeComponentsMacos([{ name: "libjpeg.8.dylib", source: path.join(root, "Cellar/jpeg-turbo/3.1.0/lib/libjpeg.8.dylib") }], env),
      /jpeg-turbo は 3\.1\.0 が入っているが、formula の版は 3\.2\.0/,
    );

    assert.deepEqual(collectLibrawSource("macos", out, components, null, env), ["LibRaw-0.22.2.tar.gz"]);
    assert.equal(fs.readFileSync(path.join(out, "LibRaw-0.22.2.tar.gz"), "utf8"), archive);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("Windows: 配布物を作るには vcpkg の LibRaw が要る（vcpkg の外の LIBRAW_LIB_DIR はビルドの前に止める）", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "genzo-setup-"));
  try {
    const vcpkg = path.join(root, "vcpkg");
    const lib = path.join(vcpkg, "installed", "x64-windows", "lib");
    fs.mkdirSync(lib, { recursive: true });
    fs.writeFileSync(path.join(lib, "raw_r.lib"), "");
    const other = path.join(root, "other", "lib");
    fs.mkdirSync(other, { recursive: true });
    fs.writeFileSync(path.join(other, "raw_r.lib"), "");

    // VCPKG_ROOT だけ: その installed/<triplet> を使う。
    const a = setupWindowsLibraw({ VCPKG_ROOT: vcpkg });
    assert.equal(a.vcpkgRoot, vcpkg);
    assert.equal(a.triplet, "x64-windows");
    // LIBRAW_* が vcpkg の中を指すなら vcpkg の LibRaw として扱う。
    const b = setupWindowsLibraw({
      VCPKG_ROOT: vcpkg,
      LIBRAW_INCLUDE_DIR: path.join(vcpkg, "installed", "x64-windows", "include"),
      LIBRAW_LIB_DIR: lib,
    });
    assert.equal(b.vcpkgRoot, vcpkg);
    const c = setupWindowsLibraw({
      VCPKG_ROOT: vcpkg,
      LIBRAW_INCLUDE_DIR: path.join(vcpkg, "installed", "x64-windows", "include"),
      LIBRAW_LIB_DIR: lib,
      LIBRAW_DLL_DIR: path.join(vcpkg, "installed", "x64-windows", "bin"),
    });
    assert.equal(c.vcpkgRoot, vcpkg);
    const env = { VCPKG_ROOT: vcpkg, LIBRAW_LIB_DIR: lib };
    assert.equal(setupWindowsLibraw(env).vcpkgRoot, vcpkg);
    assert.equal(env.LIBRAW_INCLUDE_DIR, path.join(vcpkg, "installed", "x64-windows", "include"));
    // vcpkg の外・VCPKG_ROOT がないときは止める。
    for (const env of [
      { VCPKG_ROOT: vcpkg, LIBRAW_INCLUDE_DIR: path.join(root, "other", "include"), LIBRAW_LIB_DIR: other },
      { LIBRAW_INCLUDE_DIR: path.join(root, "other", "include"), LIBRAW_LIB_DIR: other },
      { VCPKG_ROOT: vcpkg, VCPKG_DEFAULT_TRIPLET: "x64-windows-static-md", LIBRAW_INCLUDE_DIR: "x", LIBRAW_LIB_DIR: lib },
      // .lib は vcpkg の中でも、DLL・ヘッダが外なら止める。
      { VCPKG_ROOT: vcpkg, LIBRAW_INCLUDE_DIR: path.join(vcpkg, "installed", "x64-windows", "include"), LIBRAW_LIB_DIR: lib, LIBRAW_DLL_DIR: other },
      { VCPKG_ROOT: vcpkg, LIBRAW_INCLUDE_DIR: path.join(root, "other", "include"), LIBRAW_LIB_DIR: lib },
      // 片方だけの指定も確かめる（指定のないものは vcpkg の場所で埋める）。
      { VCPKG_ROOT: vcpkg, LIBRAW_INCLUDE_DIR: path.join(root, "other", "include") },
      { VCPKG_ROOT: vcpkg, LIBRAW_LIB_DIR: other },
      { VCPKG_ROOT: vcpkg, LIBRAW_DLL_DIR: other },
    ]) {
      assert.throws(() => setupWindowsLibraw(env), /配布物を作るには vcpkg の LibRaw が要る/);
    }
    assert.throws(() => setupWindowsLibraw({}), /LibRaw の場所が分からない/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
