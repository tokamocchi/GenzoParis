// pe.mjs のテスト（node --test。合成した PE で確かめる）。

import assert from "node:assert/strict";
import { test } from "node:test";

import { PeFormatError, isVcRuntimeDll, isWindowsSystemDll, readPeImports } from "./pe.mjs";

/**
 * インポートのテーブルだけを持つ最小の PE を作る（セクションは 1 つ。仕様の位置に値を書く）。
 *
 * @param {{ is64: boolean, imports?: string[], delayImports?: string[], oldStyleDelay?: boolean }} o
 */
function buildPe({ is64, imports = [], delayImports = [], oldStyleDelay = false }) {
  const imageBase = 0x400000;
  const optSize = is64 ? 240 : 224;
  const peOff = 0x40;
  const coff = peOff + 4;
  const opt = coff + 20;
  const secTable = opt + optSize;
  const rawPtr = 0x400;
  const va = 0x2000;
  const buf = new Uint8Array(rawPtr + 0x400);
  const v = new DataView(buf.buffer);
  buf[0] = 0x4d;
  buf[1] = 0x5a;
  v.setUint32(0x3c, peOff, true);
  buf.set([0x50, 0x45, 0, 0], peOff);
  v.setUint16(coff, is64 ? 0x8664 : 0x14c, true);
  v.setUint16(coff + 2, 1, true);
  v.setUint16(coff + 16, optSize, true);
  v.setUint16(opt, is64 ? 0x20b : 0x10b, true);
  if (is64) v.setBigUint64(opt + 24, BigInt(imageBase), true);
  else v.setUint32(opt + 28, imageBase, true);
  v.setUint32(opt + (is64 ? 108 : 92), 16, true);
  const dataDirs = opt + (is64 ? 112 : 96);
  // セクション: 仮想アドレス va、ファイルの位置 rawPtr、大きさ 0x400。
  v.setUint32(secTable + 8, 0x400, true);
  v.setUint32(secTable + 12, va, true);
  v.setUint32(secTable + 16, 0x400, true);
  v.setUint32(secTable + 20, rawPtr, true);

  // セクションの中: インポートの記述子は +0、遅延読み込みは +0x100、名前は +0x200 から。
  let nameCursor = 0x200;
  const putName = (s) => {
    const rva = va + nameCursor;
    buf.set(new TextEncoder().encode(`${s}\0`), rawPtr + nameCursor);
    nameCursor += s.length + 1;
    return rva;
  };
  if (imports.length > 0) {
    v.setUint32(dataDirs + 8, va, true);
    v.setUint32(dataDirs + 12, (imports.length + 1) * 20, true);
    imports.forEach((name, i) => {
      const d = rawPtr + i * 20;
      v.setUint32(d, va + 0x300, true);
      v.setUint32(d + 12, putName(name), true);
      v.setUint32(d + 16, va + 0x380, true);
    });
  }
  if (delayImports.length > 0) {
    v.setUint32(dataDirs + 13 * 8, va + 0x100, true);
    v.setUint32(dataDirs + 13 * 8 + 4, (delayImports.length + 1) * 32, true);
    delayImports.forEach((name, i) => {
      const d = rawPtr + 0x100 + i * 32;
      const rva = putName(name);
      v.setUint32(d, oldStyleDelay ? 0 : 1, true);
      v.setUint32(d + 4, oldStyleDelay ? imageBase + rva : rva, true);
      v.setUint32(d + 12, va + 0x3c0, true);
    });
  }
  return buf;
}

test("PE32+ のインポートと遅延読み込みの DLL を読む", () => {
  const pe = buildPe({
    is64: true,
    imports: ["KERNEL32.dll", "raw_r.dll", "VCRUNTIME140.dll"],
    delayImports: ["ADVAPI32.dll", "ole32.dll"],
  });
  const r = readPeImports(pe);
  assert.equal(r.is64, true);
  assert.equal(r.machine, 0x8664);
  assert.deepEqual(r.imports, ["KERNEL32.dll", "raw_r.dll", "VCRUNTIME140.dll"]);
  assert.deepEqual(r.delayImports, ["ADVAPI32.dll", "ole32.dll"]);
});

test("PE32 と古い形式の遅延読み込み（仮想アドレス）を読む", () => {
  const pe = buildPe({ is64: false, imports: ["KERNEL32.dll"], delayImports: ["WINHTTP.dll"], oldStyleDelay: true });
  const r = readPeImports(pe);
  assert.equal(r.is64, false);
  assert.deepEqual(r.imports, ["KERNEL32.dll"]);
  assert.deepEqual(r.delayImports, ["WINHTTP.dll"]);
});

test("大文字・小文字だけが違う名前は 1 つにまとめる", () => {
  const r = readPeImports(buildPe({ is64: true, imports: ["KERNEL32.dll", "kernel32.DLL"] }));
  assert.deepEqual(r.imports, ["KERNEL32.dll"]);
});

test("インポートのない PE は空の一覧", () => {
  const r = readPeImports(buildPe({ is64: true }));
  assert.deepEqual(r.imports, []);
  assert.deepEqual(r.delayImports, []);
});

test("PE でないファイル・途中で切れたファイルはエラー", () => {
  assert.throws(() => readPeImports(new TextEncoder().encode("#!/bin/sh\necho hello\n")), PeFormatError);
  const pe = buildPe({ is64: true, imports: ["KERNEL32.dll"] });
  assert.throws(() => readPeImports(pe.subarray(0, 0x150)), PeFormatError);
});

test("Visual C++ のランタイムと OS の DLL の判定", () => {
  for (const n of ["MSVCP140.dll", "msvcp140_1.dll", "VCRUNTIME140.dll", "vcruntime140_1.dll", "VCOMP140.DLL"]) {
    assert.equal(isVcRuntimeDll(n), true, n);
  }
  for (const n of ["raw_r.dll", "lcms2.dll", "KERNEL32.dll"]) assert.equal(isVcRuntimeDll(n), false, n);
  for (const n of ["KERNEL32.dll", "ntdll.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "WS2_32.dll"]) {
    assert.equal(isWindowsSystemDll(n), true, n);
  }
  for (const n of ["raw_r.dll", "jasper.dll", "zlib1.dll", "VCRUNTIME140.dll"]) {
    assert.equal(isWindowsSystemDll(n), false, n);
  }
});
