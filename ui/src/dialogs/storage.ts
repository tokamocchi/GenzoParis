// ダイアログの前回の入力（書き出しの設定・一括適用の項目・取り込みのフォルダ）をブラウザに保存する。
//
// 保存できない（容量・プライベートモードなど）ときも、その場では動くようにする（例外を外に出さない）。

const PREFIX = "genzo.dialogs.";

/** 保存した値（JSON。なければ・読めなければ null）。 */
export function loadStored(key: string): unknown {
  try {
    const v = localStorage.getItem(PREFIX + key);
    return v === null ? null : (JSON.parse(v) as unknown);
  } catch {
    return null;
  }
}

export function saveStored(key: string, value: unknown): void {
  try {
    localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // 保存できなくても、このセッションでは使える。
  }
}
