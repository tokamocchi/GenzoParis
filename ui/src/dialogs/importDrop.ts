// ウィンドウへドロップしたフォルダを、取り込みのダイアログに渡す。
//
// ダイアログの状態（appStore.dialog の import）にはフォルダを持たせられないので、ここに置いてから開く。
// 開いているダイアログには、知らせて入れ替えさせる。

type Listener = (folders: string[]) => void;

let pending: string[] | null = null;
const listeners = new Set<Listener>();

/** ドロップしたフォルダを渡す（開いている取り込みのダイアログがあれば、そこへ）。 */
export function offerImportFolders(folders: string[]): void {
  if (listeners.size > 0) {
    for (const l of [...listeners]) l(folders);
    return;
  }
  pending = folders;
}

/** 渡されたフォルダを受け取る（1 回だけ）。 */
export function takeImportFolders(): string[] | null {
  const p = pending;
  pending = null;
  return p;
}

/** 開いている間、ドロップしたフォルダを受け取る（戻り値で解除）。 */
export function onImportFolders(l: Listener): () => void {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}
