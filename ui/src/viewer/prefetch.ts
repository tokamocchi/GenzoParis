// 標準プレビュー（L1）の先読み（PERF-04: ルーペの写真送りで次の写真を 0.1 秒以内に出す）と、L1 の作成待ち
// （l1Status.ts）の記録。
//
// - 画像を読み込んで（decode まで）持っておく。ルーペが同じ URI を <img> に入れると、文書が読み込み済みの
//   画像を使うので、すぐに表示できる。
// - L1 がまだない（404）ときは、genzo:// の処理がコアに作る処理を予約する（genzo-app の protocol.rs）。
//   そのたびに作り直しが最初からになるので、404 になった写真は作成待ちとして覚え、作り終えた知らせが届く
//   まで要求しない（l1Status）。失敗した画像は持たない（作り終えた後にもう一度先読みできるように）。
// - 持っておく数は PREFETCH_KEEP まで（古いものから捨てる）。
// - 作り終えた知らせ・作れなかった知らせは、このモジュールを読み込んだときから受け取る（ルーペの部品より
//   先に受け取るので、部品が描き直すときには印が更新されている）。

import { onCoreEvent } from "../api/events";
import type { VariantId } from "../api/types";
import { appStore } from "../state/app";
import { L1Status } from "./l1Status";
import { PREFETCH_KEEP, RecentMap } from "./viewerMath";

/** アプリで 1 つの、L1 の作成待ちの記録。 */
export const l1Status = new L1Status();

interface Entry {
  img: HTMLImageElement;
  /** 読めたら true（失敗なら false）。 */
  done: Promise<boolean>;
  ready: boolean;
}

const images = new RecentMap<string, Entry>(PREFETCH_KEEP);

onCoreEvent((e) => {
  if (e.type === "thumbnail_updated") l1Status.markUpdated(e.variant_id);
  else if (e.type === "warning" && e.code === "preview_failed" && e.variant_id !== null) {
    l1Status.markFailed(e.variant_id, Date.now());
  }
});

// 別のカタログを開いた・閉じた: id が別のものを指すので忘れる。
let catalogPath = appStore.get().catalog?.catalog_path ?? null;
appStore.subscribe(() => {
  const p = appStore.get().catalog?.catalog_path ?? null;
  if (p === catalogPath) return;
  catalogPath = p;
  l1Status.reset();
  images.clear();
});

/**
 * 写真の L1 を先読みする（読み込み済み・読み込み中・作成待ちなら何もしない）。`rev` は URI の rev
 * （L0 の rev）。
 */
export function prefetchL1(id: VariantId, rev: string | null, url: string): void {
  if (images.touch(url)) return;
  if (l1Status.state(id, rev, Date.now()) !== "none") return;
  const stamp = l1Status.stamp();
  const img = new Image();
  img.decoding = "async";
  img.src = url;
  const entry: Entry = {
    img,
    ready: false,
    done: img.decode().then(
      () => {
        entry.ready = true;
        return true;
      },
      () => {
        // 404（まだ作っていない）など。作成待ちにして、作り終えた知らせの後でもう一度読む。
        if (images.get(url) === entry) images.delete(url);
        l1Status.markMissing(id, rev, stamp, Date.now());
        return false;
      },
    ),
  };
  images.set(url, entry);
}

/**
 * 先読みの状態。読めていれば `ready`、読み込み中なら `done`（結果を待てば、同じ URI を重ねて要求しない）。
 * 先読みしていなければ null。
 */
export function prefetchState(url: string): { ready: boolean; done: Promise<boolean> } | null {
  const e = images.get(url);
  return e ? { ready: e.ready, done: e.done } : null;
}
