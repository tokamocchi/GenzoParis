// macho.mjs のテスト（node --test。otool -l の出力の例で確かめる。macOS の実機の出力では未確認）。

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  candidatePaths,
  checkMacosMinimum,
  compareMacosVersions,
  installNameToolArgs,
  parseOtoolLoadCommands,
  planMacosBundle,
  unbundledReferences,
} from "./macho.mjs";

/** otool -l の出力の形（Apple の cctools）を、Load command の並びから作る。 */
function otoolText(file, commands) {
  const lines = [`${file}:`];
  commands.forEach((c, i) => {
    lines.push(`Load command ${i}`);
    if (c.cmd === "LC_BUILD_VERSION") {
      lines.push("      cmd LC_BUILD_VERSION", "  cmdsize 32", " platform 1", `    minos ${c.minos}`, "      sdk 15.0", "   ntools 1");
    } else if (c.cmd === "LC_VERSION_MIN_MACOSX") {
      lines.push("      cmd LC_VERSION_MIN_MACOSX", "  cmdsize 16", `  version ${c.version}`, "      sdk 10.15");
    } else if (c.cmd === "LC_RPATH") {
      lines.push("          cmd LC_RPATH", "      cmdsize 32", `         path ${c.path} (offset 12)`);
    } else if (c.cmd === "LC_SEGMENT_64") {
      lines.push("      cmd LC_SEGMENT_64", "  cmdsize 72", "  segname __TEXT", "   vmaddr 0x0000000000000000");
    } else {
      lines.push(
        `          cmd ${c.cmd}`,
        "      cmdsize 72",
        `         name ${c.name} (offset 24)`,
        "   time stamp 2 Thu Jan  1 09:00:02 1970",
        "      current version 8.3.2",
        "compatibility version 8.0.0",
      );
    }
  });
  return `${lines.join("\n")}\n`;
}

const LIBRAW_OPT = "/opt/homebrew/opt/libraw/lib/libraw_r.24.dylib";
const LIBRAW_REAL = "/opt/homebrew/Cellar/libraw/0.22.2/lib/libraw_r.24.dylib";
const JPEG_OPT = "/opt/homebrew/opt/jpeg-turbo/lib/libjpeg.8.dylib";
const JPEG_REAL = "/opt/homebrew/Cellar/jpeg-turbo/3.1.2/lib/libjpeg.8.3.2.dylib";
const LCMS_OPT = "/opt/homebrew/opt/little-cms2/lib/liblcms2.2.dylib";
const LCMS_REAL = "/opt/homebrew/Cellar/little-cms2/2.17/lib/liblcms2.2.dylib";
const OMP_OPT = "/opt/homebrew/opt/libomp/lib/libomp.dylib";
const OMP_REAL = "/opt/homebrew/Cellar/libomp/21.1.0/lib/libomp.dylib";
const WORKER = "/repo/target/release/genzo-worker";

test("otool -l の出力から依存・識別名・rpath・最小の版を読む", () => {
  const text = otoolText(LIBRAW_REAL, [
    { cmd: "LC_SEGMENT_64" },
    { cmd: "LC_ID_DYLIB", name: LIBRAW_OPT },
    { cmd: "LC_BUILD_VERSION", minos: "15.0" },
    { cmd: "LC_LOAD_DYLIB", name: JPEG_OPT },
    { cmd: "LC_LOAD_WEAK_DYLIB", name: "@rpath/libfoo.dylib" },
    { cmd: "LC_LOAD_DYLIB", name: "/usr/lib/libc++.1.dylib" },
    { cmd: "LC_RPATH", path: "@loader_path/../lib" },
  ]);
  const r = parseOtoolLoadCommands(text);
  assert.equal(r.id, LIBRAW_OPT);
  assert.deepEqual(r.deps, [JPEG_OPT, "@rpath/libfoo.dylib", "/usr/lib/libc++.1.dylib"]);
  assert.deepEqual(r.rpaths, ["@loader_path/../lib"]);
  assert.equal(r.minos, "15.0");
});

test("fat のファイルで繰り返す参照は 1 つにまとめる", () => {
  const arch = [
    { cmd: "LC_LOAD_DYLIB", name: LIBRAW_OPT },
    { cmd: "LC_RPATH", path: "@loader_path/../lib" },
  ];
  const r = parseOtoolLoadCommands(otoolText(WORKER, [...arch, ...arch]));
  assert.deepEqual(r.deps, [LIBRAW_OPT]);
  assert.deepEqual(r.rpaths, ["@loader_path/../lib"]);
});

test("LC_VERSION_MIN_MACOSX の版も最小の版として読む・空白を含むパス", () => {
  const r = parseOtoolLoadCommands(
    otoolText(WORKER, [
      { cmd: "LC_VERSION_MIN_MACOSX", version: "11.0" },
      { cmd: "LC_LOAD_DYLIB", name: "/Users/a b/lib/libx.dylib" },
    ]),
  );
  assert.equal(r.id, null);
  assert.equal(r.minos, "11.0");
  assert.deepEqual(r.deps, ["/Users/a b/lib/libx.dylib"]);
});

test("@rpath・@loader_path・@executable_path の候補", () => {
  const ctx = { loaderPath: LIBRAW_REAL, executablePath: WORKER, rpaths: ["@loader_path/../lib", "/opt/x/lib"] };
  assert.deepEqual(candidatePaths("@rpath/libfoo.dylib", ctx), [
    "/opt/homebrew/Cellar/libraw/0.22.2/lib/libfoo.dylib",
    "/opt/x/lib/libfoo.dylib",
  ]);
  assert.deepEqual(candidatePaths("@loader_path/libbar.dylib", ctx), ["/opt/homebrew/Cellar/libraw/0.22.2/lib/libbar.dylib"]);
  assert.deepEqual(candidatePaths("@executable_path/../lib/libbaz.dylib", ctx), ["/repo/target/lib/libbaz.dylib"]);
  assert.deepEqual(candidatePaths(JPEG_OPT, ctx), [JPEG_OPT]);
});

/** 仮のファイルの並び（参照 → 実体、実体 → otool の出力）。 */
function fakeIo(files) {
  const links = new Map(Object.entries(files.links));
  const commands = new Map(Object.entries(files.commands));
  return {
    loadCommands: (f) => {
      const c = commands.get(f);
      if (!c) throw new Error(`otool: ${f} がない`);
      return parseOtoolLoadCommands(otoolText(f, c));
    },
    realpath: (f) => links.get(f) ?? f,
    exists: (f) => links.has(f) || commands.has(f),
  };
}

test("ワーカーから Homebrew の dylib をたどり、書き換えの計画を作る", () => {
  const io = fakeIo({
    links: { [LIBRAW_OPT]: LIBRAW_REAL, [JPEG_OPT]: JPEG_REAL, [LCMS_OPT]: LCMS_REAL, [OMP_OPT]: OMP_REAL },
    commands: {
      [WORKER]: [
        { cmd: "LC_BUILD_VERSION", minos: "11.0" },
        { cmd: "LC_LOAD_DYLIB", name: LIBRAW_OPT },
        { cmd: "LC_LOAD_DYLIB", name: "/usr/lib/libSystem.B.dylib" },
        { cmd: "LC_LOAD_DYLIB", name: "/System/Library/Frameworks/Security.framework/Versions/A/Security" },
      ],
      [LIBRAW_REAL]: [
        { cmd: "LC_ID_DYLIB", name: LIBRAW_OPT },
        { cmd: "LC_BUILD_VERSION", minos: "15.0" },
        { cmd: "LC_LOAD_DYLIB", name: JPEG_OPT },
        { cmd: "LC_LOAD_DYLIB", name: LCMS_OPT },
        { cmd: "LC_LOAD_DYLIB", name: OMP_OPT },
        { cmd: "LC_LOAD_DYLIB", name: "/usr/lib/libc++.1.dylib" },
      ],
      // little-cms2 が jpeg-turbo を @rpath で参照する場合（同じ実体は 1 つにまとめる）。
      [LCMS_REAL]: [
        { cmd: "LC_ID_DYLIB", name: LCMS_OPT },
        { cmd: "LC_BUILD_VERSION", minos: "15.0" },
        { cmd: "LC_LOAD_DYLIB", name: "@rpath/libjpeg.8.dylib" },
        { cmd: "LC_RPATH", path: "/opt/homebrew/opt/jpeg-turbo/lib" },
      ],
      [JPEG_REAL]: [{ cmd: "LC_ID_DYLIB", name: JPEG_OPT }, { cmd: "LC_BUILD_VERSION", minos: "15.0" }],
      [OMP_REAL]: [{ cmd: "LC_ID_DYLIB", name: OMP_OPT }, { cmd: "LC_BUILD_VERSION", minos: "15.0" }],
    },
  });
  const plan = planMacosBundle(WORKER, io);
  assert.deepEqual(plan.worker, {
    changes: [[LIBRAW_OPT, "@executable_path/../Frameworks/libraw_r.24.dylib"]],
    minos: "11.0",
  });
  assert.deepEqual(plan.libs, [
    {
      name: "libraw_r.24.dylib",
      source: LIBRAW_REAL,
      id: "@rpath/libraw_r.24.dylib",
      minos: "15.0",
      changes: [
        [JPEG_OPT, "@loader_path/libjpeg.8.dylib"],
        [LCMS_OPT, "@loader_path/liblcms2.2.dylib"],
        [OMP_OPT, "@loader_path/libomp.dylib"],
      ],
    },
    { name: "libjpeg.8.dylib", source: JPEG_REAL, id: "@rpath/libjpeg.8.dylib", minos: "15.0", changes: [] },
    {
      name: "liblcms2.2.dylib",
      source: LCMS_REAL,
      id: "@rpath/liblcms2.2.dylib",
      minos: "15.0",
      changes: [["@rpath/libjpeg.8.dylib", "@loader_path/libjpeg.8.dylib"]],
    },
    { name: "libomp.dylib", source: OMP_REAL, id: "@rpath/libomp.dylib", minos: "15.0", changes: [] },
  ]);
});

test("見つからない依存・名前の重なりはエラー", () => {
  const missing = fakeIo({
    links: {},
    commands: { [WORKER]: [{ cmd: "LC_LOAD_DYLIB", name: "@rpath/libnone.dylib" }] },
  });
  assert.throws(() => planMacosBundle(WORKER, missing), /依存の dylib が見つからない: @rpath\/libnone\.dylib/);

  const clash = fakeIo({
    links: { "/a/libz.1.dylib": "/a/real/libz.1.dylib", "/b/libz.1.dylib": "/b/real/libz.1.dylib" },
    commands: {
      [WORKER]: [
        { cmd: "LC_LOAD_DYLIB", name: "/a/libz.1.dylib" },
        { cmd: "LC_LOAD_DYLIB", name: "/b/libz.1.dylib" },
      ],
      "/a/real/libz.1.dylib": [],
      "/b/real/libz.1.dylib": [],
    },
  });
  assert.throws(() => planMacosBundle(WORKER, clash), /名前が重なる: libz\.1\.dylib/);
});

test("install_name_tool の引数", () => {
  assert.deepEqual(
    installNameToolArgs({ id: "@rpath/libx.dylib", changes: [["/opt/a.dylib", "@loader_path/a.dylib"]] }, "out/libx.dylib"),
    ["-id", "@rpath/libx.dylib", "-change", "/opt/a.dylib", "@loader_path/a.dylib", "out/libx.dylib"],
  );
  assert.equal(installNameToolArgs({ changes: [["@loader_path/a.dylib", "@loader_path/a.dylib"]] }, "x"), null);
});

test("書き換えた後に残った参照を見つける", () => {
  assert.deepEqual(
    unbundledReferences([
      "@executable_path/../Frameworks/libraw_r.24.dylib",
      "@loader_path/libomp.dylib",
      "/usr/lib/libSystem.B.dylib",
      "/System/Library/Frameworks/Security.framework/Versions/A/Security",
      "/opt/homebrew/opt/libomp/lib/libomp.dylib",
      "@rpath/libfoo.dylib",
    ]),
    ["/opt/homebrew/opt/libomp/lib/libomp.dylib", "@rpath/libfoo.dylib"],
  );
});

test("macOS の版を比べる", () => {
  assert.ok(compareMacosVersions("15.0", "12.0") > 0);
  assert.ok(compareMacosVersions("11.0", "12.0") < 0);
  assert.equal(compareMacosVersions("12", "12.0"), 0);
  assert.ok(compareMacosVersions("12.0.1", "12.0") > 0);
});

test("同梱するものの最小の版が .app の最小の版より新しければ、既定ではエラー", () => {
  const items = [
    { name: "genzo-worker", minos: "11.0" },
    { name: "libraw_r.24.dylib", minos: "15.0" },
    { name: "libomp.dylib", minos: "14.0" },
    { name: "libjpeg.8.dylib", minos: null },
  ];
  const denied = checkMacosMinimum(items, "12.0", false);
  assert.equal(denied.newest, "15.0");
  assert.deepEqual(denied.newer, [
    { name: "libraw_r.24.dylib", minos: "15.0" },
    { name: "libomp.dylib", minos: "14.0" },
  ]);
  assert.deepEqual(denied.unknown, ["libjpeg.8.dylib"]);
  assert.match(denied.error ?? "", /GENZO_ALLOW_NEWER_MINOS=1/);
  assert.match(denied.error ?? "", /libraw_r\.24\.dylib（minos 15\.0）/);
  assert.equal(denied.minimumSystemVersion, "12.0");

  // 明示的に許すときは通し、.app の最小の版を minos の最大に上げる。
  const allowed = checkMacosMinimum(items, "12.0", true);
  assert.equal(allowed.error, null);
  assert.equal(allowed.minimumSystemVersion, "15.0");
});

test("同梱するものがすべて .app の最小の版以下なら、そのまま通す", () => {
  const r = checkMacosMinimum(
    [
      { name: "genzo-worker", minos: "11.0" },
      { name: "libraw_r.24.dylib", minos: "12.0" },
    ],
    "12.0",
    false,
  );
  assert.equal(r.error, null);
  assert.equal(r.newest, "12.0");
  assert.deepEqual(r.newer, []);
  assert.equal(r.minimumSystemVersion, "12.0");
  // minos がどれも読み取れなくても止めない（警告だけ。scripts/lib/worker.mjs）。
  const unknown = checkMacosMinimum([{ name: "x.dylib", minos: null }], "12.0", false);
  assert.equal(unknown.error, null);
  assert.equal(unknown.newest, null);
  assert.deepEqual(unknown.unknown, ["x.dylib"]);
});
