// Windows の実行ファイル・DLL（PE 形式）から、読み込む DLL の名前を取り出す（純粋な処理）。
//
// 配布用のビルドで、ワーカー（genzo-worker.exe）が読み込む LibRaw の DLL と、その DLL が読み込む DLL を
// たどって集めるために使う（scripts/lib/worker.mjs）。dumpbin（Visual Studio の開発者用の環境が要る）を
// 使わずに済むよう、PE のヘッダを直接読む。
//
// 読むもの（Microsoft の「PE Format」の仕様）:
// - インポートのテーブル（データディレクトリの 1 番）: 起動のときに読み込む DLL
// - 遅延読み込みのインポートのテーブル（データディレクトリの 13 番）: 最初に呼んだときに読み込む DLL
//
// 確認: この Linux のコンテナで、実際の PE ファイル（WebView2Loader.dll など）の結果が llvm-readobj
// --coff-imports と一致することを確かめた（2026-10-12）。テスト（pe.test.mjs）は合成した PE で確かめる。

/** PE32（32 ビット）の Optional Header の Magic。 */
const PE32_MAGIC = 0x10b;
/** PE32+（64 ビット）の Optional Header の Magic。 */
const PE32_PLUS_MAGIC = 0x20b;
/** データディレクトリの番号: インポートのテーブル。 */
const DIR_IMPORT = 1;
/** データディレクトリの番号: 遅延読み込みのインポートのテーブル。 */
const DIR_DELAY_IMPORT = 13;
/** インポートの記述子の大きさ（バイト）。 */
const IMPORT_DESCRIPTOR_SIZE = 20;
/** 遅延読み込みの記述子の大きさ（バイト）。 */
const DELAY_DESCRIPTOR_SIZE = 32;
/** 1 つのテーブルから読む記述子の上限（壊れたファイルで止まらなくならないため。仮置き）。 */
const MAX_DESCRIPTORS = 4096;

export class PeFormatError extends Error {
  constructor(message) {
    super(message);
    this.name = "PeFormatError";
  }
}

/**
 * PE のファイルの中身から、読み込む DLL の名前を返す。
 *
 * @param {Uint8Array} bytes ファイルの中身
 * @returns {{ machine: number, is64: boolean, imports: string[], delayImports: string[] }}
 *   `imports`・`delayImports` はファイルに書かれた順（大文字・小文字はそのまま。重複は除く）。
 */
export function readPeImports(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const u16 = (off) => {
    check(off + 2 <= bytes.length, "ファイルが途中で終わっている");
    return view.getUint16(off, true);
  };
  const u32 = (off) => {
    check(off + 4 <= bytes.length, "ファイルが途中で終わっている");
    return view.getUint32(off, true);
  };

  check(bytes.length >= 0x40 && bytes[0] === 0x4d && bytes[1] === 0x5a, "MZ のヘッダがない（PE ではない）");
  const peOff = u32(0x3c);
  check(
    peOff + 24 <= bytes.length &&
      bytes[peOff] === 0x50 &&
      bytes[peOff + 1] === 0x45 &&
      bytes[peOff + 2] === 0 &&
      bytes[peOff + 3] === 0,
    "PE の署名がない",
  );
  const coff = peOff + 4;
  const machine = u16(coff);
  const numberOfSections = u16(coff + 2);
  const sizeOfOptionalHeader = u16(coff + 16);
  const opt = coff + 20;
  const magic = u16(opt);
  let is64;
  if (magic === PE32_MAGIC) {
    is64 = false;
  } else if (magic === PE32_PLUS_MAGIC) {
    is64 = true;
  } else {
    throw new PeFormatError(`Optional Header の Magic が不明（0x${magic.toString(16)}）`);
  }
  // NumberOfRvaAndSizes とデータディレクトリの位置（PE32: 92・96、PE32+: 108・112）。
  const numberOfRvaAndSizes = u32(opt + (is64 ? 108 : 92));
  const dataDirs = opt + (is64 ? 112 : 96);
  const imageBase = is64 ? Number(view.getBigUint64(opt + 24, true)) : u32(opt + 28);

  const sectionTable = opt + sizeOfOptionalHeader;
  const sections = [];
  for (let i = 0; i < numberOfSections; i++) {
    const s = sectionTable + i * 40;
    sections.push({
      virtualSize: u32(s + 8),
      virtualAddress: u32(s + 12),
      sizeOfRawData: u32(s + 16),
      pointerToRawData: u32(s + 20),
    });
  }

  /** RVA（読み込んだときのアドレス）をファイルの中の位置に直す。どのセクションにもなければ null。 */
  const rvaToOffset = (rva) => {
    for (const s of sections) {
      const size = Math.max(s.virtualSize, s.sizeOfRawData);
      if (rva >= s.virtualAddress && rva < s.virtualAddress + size) {
        const off = rva - s.virtualAddress + s.pointerToRawData;
        return off < bytes.length ? off : null;
      }
    }
    return null;
  };

  const readCString = (off) => {
    let end = off;
    while (end < bytes.length && bytes[end] !== 0) end++;
    check(end < bytes.length, "DLL の名前が途中で終わっている");
    return new TextDecoder("latin1").decode(bytes.subarray(off, end));
  };

  const dir = (index) => {
    if (index >= numberOfRvaAndSizes) return { rva: 0, size: 0 };
    return { rva: u32(dataDirs + index * 8), size: u32(dataDirs + index * 8 + 4) };
  };

  const imports = [];
  const importDir = dir(DIR_IMPORT);
  if (importDir.rva !== 0) {
    const start = rvaToOffset(importDir.rva);
    check(start !== null, "インポートのテーブルの位置が不正");
    for (let i = 0; i < MAX_DESCRIPTORS; i++) {
      const d = start + i * IMPORT_DESCRIPTOR_SIZE;
      const originalFirstThunk = u32(d);
      const nameRva = u32(d + 12);
      const firstThunk = u32(d + 16);
      // 最後の記述子は 0 で埋まっている。
      if (originalFirstThunk === 0 && nameRva === 0 && firstThunk === 0) break;
      const nameOff = rvaToOffset(nameRva);
      check(nameOff !== null, "DLL の名前の位置が不正");
      pushUnique(imports, readCString(nameOff));
    }
  }

  const delayImports = [];
  const delayDir = dir(DIR_DELAY_IMPORT);
  if (delayDir.rva !== 0) {
    const start = rvaToOffset(delayDir.rva);
    check(start !== null, "遅延読み込みのテーブルの位置が不正");
    for (let i = 0; i < MAX_DESCRIPTORS; i++) {
      const d = start + i * DELAY_DESCRIPTOR_SIZE;
      const attributes = u32(d);
      const nameField = u32(d + 4);
      const iat = u32(d + 12);
      if (attributes === 0 && nameField === 0 && iat === 0) break;
      // 属性の 1 ビット目が立っていれば RVA。古い形式（立っていない）は仮想アドレスなので ImageBase を引く。
      const nameRva = (attributes & 1) !== 0 ? nameField : nameField - imageBase;
      const nameOff = rvaToOffset(nameRva);
      check(nameOff !== null, "遅延読み込みの DLL の名前の位置が不正");
      pushUnique(delayImports, readCString(nameOff));
    }
  }

  return { machine, is64, imports, delayImports };
}

/** Windows の DLL の名前を比べるための形（Windows のファイル名は大文字・小文字を区別しない）。 */
export function dllKey(name) {
  return name.toLowerCase();
}

/**
 * Visual C++ のランタイムの DLL か（Visual Studio の再頒布用のフォルダから、アプリと一緒に置く。
 * scripts/lib/bundle.mjs の planWindowsDlls、決定事項ログ No.73）。
 */
export function isVcRuntimeDll(name) {
  return /^(msvcp\d+(_\d+|_atomic_wait|_codecvt_ids)?|vcruntime\d+(_\d+)?|concrt\d+|vccorlib\d+|vcomp\d+)\.dll$/i.test(
    name,
  );
}

/** Windows 自身が持っている DLL とみなす名前か（API セットと、よく使う OS の DLL）。 */
export function isWindowsSystemDll(name) {
  const n = dllKey(name);
  if (n.startsWith("api-ms-win-") || n.startsWith("ext-ms-")) return true;
  return WINDOWS_SYSTEM_DLLS.has(n);
}

/**
 * OS の DLL の名前（小文字）。ここにない DLL も、LibRaw のフォルダで見つからなければ同梱せずに一覧に出す
 * （scripts/lib/worker.mjs）。一覧は網羅ではない（仮置き）。
 */
const WINDOWS_SYSTEM_DLLS = new Set([
  "advapi32.dll",
  "bcrypt.dll",
  "bcryptprimitives.dll",
  "comctl32.dll",
  "comdlg32.dll",
  "crypt32.dll",
  "d3d11.dll",
  "d3d12.dll",
  "dbghelp.dll",
  "dwmapi.dll",
  "dxgi.dll",
  "gdi32.dll",
  "gdiplus.dll",
  "imm32.dll",
  "kernel32.dll",
  "kernelbase.dll",
  "msvcrt.dll",
  "ncrypt.dll",
  "ntdll.dll",
  "ole32.dll",
  "oleaut32.dll",
  "opengl32.dll",
  "propsys.dll",
  "psapi.dll",
  "rpcrt4.dll",
  "secur32.dll",
  "setupapi.dll",
  "shell32.dll",
  "shlwapi.dll",
  "ucrtbase.dll",
  "user32.dll",
  "userenv.dll",
  "uxtheme.dll",
  "version.dll",
  "winmm.dll",
  "ws2_32.dll",
]);

function pushUnique(list, name) {
  if (!list.some((n) => dllKey(n) === dllKey(name))) list.push(name);
}

function check(cond, message) {
  if (!cond) throw new PeFormatError(message);
}
