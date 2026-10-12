// ライブラリの画面の表示の好み（セルの大きさ・サブフォルダを含めるか・フォルダの開閉）をブラウザに保存する。
//
// - 保存できない（容量・プライベートモードなど）ときも、その場では動くようにする（例外を外に出さない）。
// - カタログの中の ID（フォルダの ID など）に結びつく値は、カタログごとに分ける（catalogKey）。ID は
//   カタログのデータベースの行の ID なので、別のカタログでは別のものを指す。

const PREFIX = "genzo.library.";

/** 文字列の 32 bit の FNV-1a ハッシュ（16 進 8 桁。UTF-16 の単位で計算する）。 */
export function hashString(s: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16).padStart(8, "0");
}

/**
 * カタログごとに分ける値の鍵（"folderOpen" → "folderOpen.<カタログのパスのハッシュ>"）。カタログを
 * 開いていなければ null（保存しない）。
 */
export function catalogKey(base: string, catalogPath: string | null | undefined): string | null {
  return catalogPath ? `${base}.${hashString(catalogPath)}` : null;
}

export function loadNumber(key: string, fallback: number, min: number, max: number): number {
  try {
    const v = localStorage.getItem(PREFIX + key);
    if (v === null) return fallback;
    const n = Number(v);
    return Number.isFinite(n) ? Math.min(max, Math.max(min, n)) : fallback;
  } catch {
    return fallback;
  }
}

export function loadBoolean(key: string, fallback: boolean): boolean {
  try {
    const v = localStorage.getItem(PREFIX + key);
    return v === null ? fallback : v === "1";
  } catch {
    return fallback;
  }
}

export function loadJson<T>(key: string, fallback: T, valid: (v: unknown) => v is T): T {
  try {
    const v = localStorage.getItem(PREFIX + key);
    if (v === null) return fallback;
    const parsed: unknown = JSON.parse(v);
    return valid(parsed) ? parsed : fallback;
  } catch {
    return fallback;
  }
}

export function save(key: string, value: number | boolean | object): void {
  try {
    const s = typeof value === "boolean" ? (value ? "1" : "0") : typeof value === "number" ? String(value) : JSON.stringify(value);
    localStorage.setItem(PREFIX + key, s);
  } catch {
    // 保存できなくても、このセッションでは使える。
  }
}
