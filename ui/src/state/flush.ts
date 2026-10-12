// カタログを閉じる・終了する前に済ませる書き込み（編集中のキャプションなど）。
//
// 画面の部品が持っている「まだコアに送っていない変更」を、閉じる前に送り切るための登録の仕組み。現像の送信待ちの
// 設定は state/develop.ts が持つ（closeDevelopSession）。ここには、それ以外の部品（キャプションの欄など）が
// 登録する。カタログを閉じる・切り替える・復元する・終了する前に、dialogs/catalogActions.ts が待つ。

/** 済ませる書き込み（登録した部品が消えるときに外す）。 */
const flushers = new Set<() => Promise<void>>();

/** 閉じる前に済ませる書き込みを登録する（戻り値は登録を外す関数）。 */
export function registerFlush(flush: () => Promise<void>): () => void {
  flushers.add(flush);
  return () => {
    flushers.delete(flush);
  };
}

/** 閉じる前の書き込みに失敗した（閉じる・終了するのをやめる。`causes` は失敗した書き込みのエラー）。 */
export class FlushError extends Error {
  constructor(readonly causes: unknown[]) {
    super("閉じる前の書き込みに失敗しました");
    this.name = "FlushError";
  }
}

/**
 * 登録された書き込みをすべて済ませる。1 つでも失敗したら FlushError を投げる（呼び出し側は閉じるのをやめる。
 * 失敗したことは、それぞれの部品も知らせる）。
 */
export async function flushPendingWrites(): Promise<void> {
  const results = await Promise.allSettled([...flushers].map((f) => f()));
  const causes = results.filter((r) => r.status === "rejected").map((r) => (r as PromiseRejectedResult).reason);
  if (causes.length > 0) throw new FlushError(causes);
}
