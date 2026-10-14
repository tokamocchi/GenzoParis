// 写真のサムネイル（L0）と標準プレビュー（L1）の読み込み（ルーペ・比較表示・選別表示で共通。LIB-02・LIB-03・
// PERF-04/05）。
//
// - L1 が読めるまでは L0 を同じ大きさで出す（PERF-05: 先に埋め込み JPEG、後で現像結果）。読めるまで前の画像を
//   出しておく（useImageSlot・SlotImages。同じ URI の要素は作り直さない）。
// - L1 がまだない（404）ときは、genzo:// の処理がコアに作る処理を予約する（genzo-app の protocol.rs）。
//   要求し直すたびにコアの作り直しが最初からになるので、404 になった写真は作成待ちとして覚え（l1Status.ts）、
//   作り終えた知らせ（thumbnail_updated）が届くまで要求しない。rev が変われば URI が変わって読み直す。
//   rev が同じまま（L1 だけがキャッシュの回収で消えていた）のときは、URI に印（retry）を付けて読み直す。
//   知らせを取りこぼした場合に備え、作成待ちのまま L1_CHECK_MS たったら、作る処理を予約しない preview_path で
//   1 回だけ確かめる。
// - L1 の要求は `waitMs`（ルーペの写真送りの途中なら残りの待ち時間）だけ待つ。先読み中（prefetch.ts）なら、
//   その結果を待つ（同じ URI を重ねて要求しない）。
// - `wantL1` が偽なら L1 を要求しない（選別表示の小さな枠では L0 で足りる。LIB-03。決定事項ログ No.116）。
//
// 写真ごとに作り直す部品（key に variant の id）で使う（待つのは最初の 1 回だけ）。

import { useEffect, useRef, useState } from "react";

import * as core from "../api/core";
import type { VariantSummary } from "../api/types";
import { previewUrl, thumbUrl } from "../api/uri";
import { useCoreEvent } from "./hooks";
import type { L1State } from "./l1Status";
import { l1Status, prefetchState } from "./prefetch";
import { L1_CHECK_MS, withRetryParam } from "./viewerMath";

/** L1 の読み直しの回数の上限（同じ rev のまま読めない場合。仮置き）。 */
const MAX_L1_RETRY = 5;

// ---------------------------------------------------------------------------
// 画像の読み込み（読めるまで前の画像を出しておく）
// ---------------------------------------------------------------------------

export interface LoadedImage {
  url: string;
  width: number;
  height: number;
}

export function useImageSlot(url: string | null) {
  const [shown, setShown] = useState<LoadedImage | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  return {
    url,
    /** 最後に読めた画像（URI が変わっても、新しい画像が読めるまで出しておく）。 */
    shown,
    ready: url !== null && shown?.url === url,
    failed: url !== null && failed === url,
    onLoad: (u: string, img: HTMLImageElement) =>
      setShown({ url: u, width: img.naturalWidth, height: img.naturalHeight }),
    onError: (u: string) => setFailed(u),
  };
}

export type ImageSlot = ReturnType<typeof useImageSlot>;

/** 読めた画像と、読み込み中の画像（隠す）。同じ URI の要素は作り直さない（key が URI）。 */
export function SlotImages(props: { slot: ImageSlot; onError?: (url: string) => void }) {
  const { slot } = props;
  const urls: string[] = [];
  if (slot.shown) urls.push(slot.shown.url);
  if (slot.url && slot.url !== slot.shown?.url && !slot.failed) urls.push(slot.url);
  return (
    <>
      {urls.map((u) => (
        <img
          key={u}
          className="loupe-img"
          src={u}
          alt=""
          draggable={false}
          decoding="async"
          style={u === slot.shown?.url ? undefined : { visibility: "hidden" }}
          onLoad={(e) => slot.onLoad(u, e.currentTarget)}
          onError={() => {
            slot.onError(u);
            props.onError?.(u);
          }}
        />
      ))}
    </>
  );
}

// ---------------------------------------------------------------------------
// L0 と L1
// ---------------------------------------------------------------------------

export interface PreviewImages {
  l0: ImageSlot;
  l1: ImageSlot;
  /** L1 の作成待ちの状態（l1Status.ts）。 */
  l1State: L1State;
  /** L1 を作れなかった（preview_failed の知らせ・作成の失敗の印）。 */
  l1Failed: boolean;
  /** L1 の読み込みの失敗（SlotImages の onError に渡す）。 */
  onL1Error: (url: string) => void;
  /** 表示中の写真の L1 が決まった（読めた・作成待ち・作れなかった）。先読みを始めてよい。 */
  l1Settled: boolean;
  /** L1 を今は出せない（読めなかった・作成待ち・作れなかった）。 */
  l1Gone: boolean;
}

/** 写真 `summary` の L0 と L1 を読む（`previewImages` の doc）。 */
export function usePreviewImages(opts: {
  summary: VariantSummary;
  thumbRev: string | null;
  /** L1 の要求までに待つ時間（ミリ秒。最初の描画のときに 1 回だけ呼ぶ）。 */
  waitMs: () => number;
  /** L1 を要求するか（偽なら L0 だけ）。 */
  wantL1?: boolean;
}): PreviewImages {
  const { summary, thumbRev } = opts;
  const wantL1 = opts.wantL1 ?? true;
  const id = summary.variant_id;
  const fileOk = summary.file_status === "ok";
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const [retry, setRetry] = useState<{ rev: string | null; n: number }>({ rev: thumbRev, n: 0 });
  const retryCount = retry.rev === thumbRev ? retry.n : 0;
  const revRef = useRef(thumbRev);
  revRef.current = thumbRev;
  const bumpRetry = () =>
    setRetry((r) => {
      const n = r.rev === revRef.current ? r.n : 0;
      return n >= MAX_L1_RETRY ? r : { rev: revRef.current, n: n + 1 };
    });
  const l0 = useImageSlot(thumbRev ? thumbUrl(id, thumbRev) : null);
  const l1Url = withRetryParam(previewUrl(id, thumbRev), retryCount);
  // 作成待ち（404 になった。作り終えた知らせを待つ）・作れなかった間は要求しない（l1Status.ts）。
  const l1State = l1Status.state(id, thumbRev, Date.now());
  // 待ってから要求する。先読み中なら、その結果を待つ（同じ URI を重ねて要求しない）。
  const [l1Go, setL1Go] = useState(() => {
    const p = prefetchState(l1Url);
    return p ? p.ready : opts.waitMs() <= 0;
  });
  useEffect(() => {
    if (l1Go) return;
    let cancelled = false;
    const p = prefetchState(l1Url);
    if (p) {
      void p.done.then(() => {
        if (!cancelled) setL1Go(true);
      });
      return () => {
        cancelled = true;
      };
    }
    const timer = setTimeout(() => setL1Go(true), Math.max(0, opts.waitMs()));
    return () => clearTimeout(timer);
    // 写真ごとに作り直す部品なので、待つのは最初の 1 回だけ。
  }, [l1Go]);
  const l1 = useImageSlot(wantL1 && l1Go && l1State === "none" ? l1Url : null);
  // 要求を始めたときの通し番号（404 と作り終えた知らせの順序の入れ替わりを見分ける。l1Status.ts）。
  const l1Request = useRef<{ url: string | null; rev: string | null; stamp: number }>({
    url: null,
    rev: null,
    stamp: 0,
  });
  if (l1Request.current.url !== l1.url) {
    l1Request.current = { url: l1.url, rev: thumbRev, stamp: l1Status.stamp() };
  }
  const l1ReadyRef = useRef(false);
  l1ReadyRef.current = l1.ready;
  const [previewFailed, setPreviewFailed] = useState(false);
  const l1Failed = previewFailed || l1State === "failed";

  // L1 が読めない: 作成待ちにする（作る処理は genzo:// の処理が予約した。作り終えた知らせを待つ）。要求の
  // 後に作り終えていれば、すぐ読み直す。もう使っていない URI の失敗は使わない（要求を始めた時点が分からない）。
  const onL1Error = (url: string) => {
    const r = l1Request.current;
    if (r.url !== url || !mounted.current) return;
    if (!l1Status.markMissing(id, r.rev, r.stamp, Date.now())) bumpRetry();
  };
  useCoreEvent((e) => {
    if (e.type === "thumbnail_updated" && e.variant_id === id) {
      // rev が変われば URI が変わって読み直す。同じ rev なら印を付けて読み直す（作成待ちの印は prefetch.ts が
      // 先に消している）。
      if (!l1ReadyRef.current && e.rev === revRef.current) bumpRetry();
    } else if (e.type === "warning" && e.code === "preview_failed" && e.variant_id === id) {
      setPreviewFailed(true);
    }
  });

  // 作成待ちのまま L1_CHECK_MS たった: 知らせを取りこぼしていないか、作る処理を予約しない preview_path で
  // 1 回だけ確かめる（request_preview は、なければ作り直しを予約し直すので使わない）。
  useEffect(() => {
    if (!wantL1 || l1State !== "waiting" || !fileOk) return;
    const timer = setTimeout(() => {
      core
        .execute({ type: "preview_path", variant_id: id }, "path")
        .then((r) => {
          if (!r.path || !mounted.current) return;
          l1Status.clear(id);
          bumpRetry();
        })
        .catch(() => undefined);
    }, L1_CHECK_MS);
    return () => clearTimeout(timer);
  }, [wantL1, l1State, fileOk, id]);

  const l1Settled = !wantL1 || l1.ready || l1.failed || l1State !== "none";
  const l1Gone = l1.failed || l1State !== "none";
  return { l0, l1, l1State, l1Failed, onL1Error, l1Settled, l1Gone };
}
