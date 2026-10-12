// 標準プレビュー（L1）の先読み（PERF-04: ルーペの写真送りで次の写真を 0.1 秒以内に出す）。
//
// - 画像を読み込んで（decode まで）持っておく。ルーペが同じ URI を <img> に入れると、文書が読み込み済みの
//   画像を使うので、すぐに表示できる。
// - L1 がまだない（404）ときは、genzo:// の処理がコアに作る処理を予約する（genzo-app の protocol.rs）。
//   失敗したものは持たない（作り終えた後にもう一度先読みできるように）。
// - 持っておく数は PREFETCH_KEEP まで（古いものから捨てる）。

import { PREFETCH_KEEP, RecentMap } from "./viewerMath";

const images = new RecentMap<string, HTMLImageElement>(PREFETCH_KEEP);

/** 画像を先読みする（読み込み済み・読み込み中なら何もしない）。 */
export function prefetchImage(url: string): void {
  if (images.touch(url)) return;
  const img = new Image();
  img.decoding = "async";
  img.src = url;
  images.set(url, img);
  img.decode().catch(() => {
    // 404（まだ作っていない）など。次の先読みでもう一度読む。
    if (images.get(url) === img) images.delete(url);
  });
}
