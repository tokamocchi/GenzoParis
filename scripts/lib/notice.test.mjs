// notice.mjs のテスト（node --test）。

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";

import {
  LIBRAW_LICENSE_FILES,
  LIBRAW_LICENSE_SOURCE_DIR,
  licenseResources,
  librawNotice,
  librawVersionHeaderCandidates,
  parseLibrawVersion,
  recipeLines,
} from "./notice.mjs";
import { APP_DIR } from "./proc.mjs";

test("LibRaw のヘッダから版を読む", () => {
  const header = [
    "#ifndef __VERSION_H",
    "#define LIBRAW_MAJOR_VERSION 0",
    "#define LIBRAW_MINOR_VERSION 22",
    "#define LIBRAW_PATCH_VERSION 2",
    "#define LIBRAW_VERSION_TAIL Release",
  ].join("\r\n");
  assert.equal(parseLibrawVersion(header), "0.22.2");
  assert.equal(parseLibrawVersion("#  define LIBRAW_MAJOR_VERSION 0\n#define LIBRAW_MINOR_VERSION 21\n#define LIBRAW_PATCH_VERSION 4\n"), "0.21.4");
  assert.equal(parseLibrawVersion("#define LIBRAW_MAJOR_VERSION 0\n#define LIBRAW_MINOR_VERSION 22\n"), null);
  assert.equal(parseLibrawVersion(""), null);
});

test("版のヘッダを探す場所（libraw/libraw.h・libraw.h のどちらのあるフォルダでもよい）", () => {
  assert.deepEqual(librawVersionHeaderCandidates(["C:/vcpkg/include", "", "/opt/homebrew/include"]), [
    "C:/vcpkg/include/libraw/libraw_version.h",
    "C:/vcpkg/include/libraw_version.h",
    "/opt/homebrew/include/libraw/libraw_version.h",
    "/opt/homebrew/include/libraw_version.h",
  ]);
});

test("ライセンスの文書・案内・ビルドの手順の写しを、配布物の licenses/LibRaw に置く対応", () => {
  assert.deepEqual(licenseResources(), {
    "binaries/licenses/LibRaw/LICENSE.LGPL": "licenses/LibRaw/LICENSE.LGPL",
    "binaries/licenses/LibRaw/COPYRIGHT": "licenses/LibRaw/COPYRIGHT",
    "binaries/licenses/LibRaw/README.txt": "licenses/LibRaw/README.txt",
  });
  const r = licenseResources(["vcpkg-ports-libraw/vcpkg.json", "vcpkg-ports-libraw/portfile.cmake"]);
  assert.equal(r["binaries/licenses/LibRaw/build-recipe/vcpkg-ports-libraw/portfile.cmake"], "licenses/LibRaw/build-recipe/vcpkg-ports-libraw/portfile.cmake");
  assert.equal(Object.keys(r).length, 5);
  const withSource = licenseResources([], ["LibRaw-LibRaw-0.22.2.tar.gz"]);
  assert.equal(withSource["binaries/licenses/LibRaw/source/LibRaw-LibRaw-0.22.2.tar.gz"], "licenses/LibRaw/source/LibRaw-LibRaw-0.22.2.tar.gz");
});

test("ビルドの手順の案内は、コミットを固定した URL にする", () => {
  const vcpkg = recipeLines({ kind: "vcpkg", version: "0.22.0#1", revision: "abc123", copied: true });
  assert.match(vcpkg.ja, /版 0\.22\.0#1。https:\/\/github\.com\/microsoft\/vcpkg\/tree\/abc123\/ports\/libraw。写しを build-recipe に同梱（port が当てるパッチを含む）/);
  assert.match(vcpkg.en, /tree\/abc123\/ports\/libraw; a copy is included in build-recipe/);
  const brew = recipeLines({ kind: "homebrew", version: "0.22.2", revision: "def456", copied: true });
  assert.match(brew.ja, /https:\/\/github\.com\/Homebrew\/homebrew-core\/blob\/def456\/Formula\/lib\/libraw\.rb/);
  assert.doesNotMatch(brew.ja, /パッチ/);
  // コミットが取れなければ、master・HEAD の URL ではなく「記録できなかった」と書く。
  const none = recipeLines({ kind: "homebrew", copied: false });
  assert.match(none.ja, /ビルドの時点のコミットは記録できませんでした/);
  assert.doesNotMatch(none.ja, /HEAD|master/);
  assert.match(recipeLines({ kind: "unknown" }).ja, /独自にビルド/);
});

test("上流の LibRaw のファイルをそのまま持つ（LGPL-2.1 の全文と著作権表示）", () => {
  const sha256 = (f) => createHash("sha256").update(fs.readFileSync(f)).digest("hex");
  const dir = path.join(APP_DIR, LIBRAW_LICENSE_SOURCE_DIR);
  assert.deepEqual(LIBRAW_LICENSE_FILES, ["LICENSE.LGPL", "COPYRIGHT"]);
  // LibRaw の GitHub の 0.22.0 のタグのファイル（LICENSE.LGPL は 0.21.2・0.21.4・master とも同じ）。
  assert.equal(sha256(path.join(dir, "LICENSE.LGPL")), "eea173a556abac0370461e57e12aab266894ea6be3874c2be05fd87871f75449");
  assert.equal(sha256(path.join(dir, "COPYRIGHT")), "574c88f5dd59414cfc896293a63defbc2d525a418ed413d7ab7dcc43cf79c509");
  const lgpl = fs.readFileSync(path.join(dir, "LICENSE.LGPL"), "utf8");
  assert.match(lgpl, /GNU LESSER GENERAL PUBLIC LICENSE\s+Version 2\.1, February 1999/);
  assert.match(lgpl, /END OF TERMS AND CONDITIONS/);
});

test("案内に版・同梱したファイル・ソースの入手先・差し替えの方法を書く", () => {
  const win = librawNotice({
    os: "windows",
    version: "0.22.0",
    bundled: ["z.dll", "raw_r.dll", "lcms2-2.dll", "vcruntime140.dll"],
    recipe: { kind: "vcpkg", version: "0.22.0", revision: "abc123", copied: true },
  });
  assert.match(win, /LibRaw 0\.22\.0 を使っています/);
  assert.match(win, /LGPL-2\.1 を選んで/);
  assert.match(win, /- LibRaw: raw_r\.dll\n/);
  assert.match(win, /- LibRaw などが使うライブラリ: lcms2-2\.dll, vcruntime140\.dll, z\.dll\n/);
  assert.match(win, /https:\/\/github\.com\/LibRaw\/LibRaw\/tree\/0\.22\.0/);
  assert.match(win, /%LOCALAPPDATA%\\GenzoParis/);
  assert.match(win, /vcpkg\/tree\/abc123\/ports\/libraw/);
  assert.doesNotMatch(win, /codesign/);
  assert.doesNotMatch(win, /source\//, "ソースを同梱しないときは書かない");
  assert.match(win, /GNU Lesser General Public\nLicense version 2\.1/);

  const mac = librawNotice({
    os: "macos",
    version: "0.22.2",
    bundled: ["libraw_r.25.dylib", "libomp.dylib"],
    recipe: { kind: "homebrew", version: "0.22.2", revision: "def456", copied: true },
    source: ["LibRaw-0.22.2.tar.gz"],
  });
  assert.match(mac, /このフォルダの source。入手元が取得したアーカイブをそのまま）:\n      source\/LibRaw-0\.22\.2\.tar\.gz\n/);
  assert.match(mac, /included in the source folder: LibRaw-0\.22\.2\.tar\.gz\./);
  assert.match(mac, /GenzoParis\.app\/Contents\/Frameworks の libraw_r\.25\.dylib/);
  assert.match(mac, /codesign --force --deep --sign - \/Applications\/GenzoParis\.app/);
  assert.match(mac, /homebrew-core\/blob\/def456\/Formula\/lib\/libraw\.rb/);
  assert.doesNotMatch(mac, /%LOCALAPPDATA%/);

  assert.throws(
    () => librawNotice({ os: "windows", version: "0.22.0", bundled: ["z.dll"], recipe: { kind: "unknown" } }),
    /LibRaw がない/,
  );
});
