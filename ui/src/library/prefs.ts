// ライブラリの画面の表示の好み（セルの大きさ・サブフォルダを含めるか・フォルダの開閉）をブラウザに保存する。
//
// 保存できない（容量・プライベートモードなど）ときも、その場では動くようにする（例外を外に出さない）。

const PREFIX = "genzo.library.";

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
