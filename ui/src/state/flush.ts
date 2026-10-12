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

/** 登録された書き込みをすべて済ませる（失敗したものは、それぞれの部品が知らせる。ここでは待つだけ）。 */
export async function flushPendingWrites(): Promise<void> {
  await Promise.allSettled([...flushers].map((f) => f()));
}
