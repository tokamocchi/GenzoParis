// 標準プレビュー（L1）がまだない写真を覚える（同じ写真の L1 を何度も要求しない。PERF-04/05）。
//
// genzo:// の L1 の要求が 404 になるたびに、コアは作る処理を予約し直す（genzo-app の protocol.rs →
// genzo-api の request_preview → genzo-jobs の submit_latest）。同じ写真の作り直しが実行中なら取り消して
// 最初からやり直し（SupersedePolicy::CancelRunning）、待っていればキューの末尾に回る。そこで 404 になった
// 写真は「作成待ち」として覚え、作り終えた知らせ（thumbnail_updated）か、作れなかった知らせ
// （preview_failed の警告）が届くまで、同じ写真の L1 を要求しない（先読みもルーペ本体も）。
//
// - 要求を始めたときの通し番号（stamp）を受け取り、要求の後に作り終えた知らせが届いていれば、その 404 は
//   作る前の古い結果として扱い、作成待ちにしない（知らせと 404 の順序の入れ替わり）。
// - 知らせを取りこぼしても、ずっと要求しないままにならないよう、印は expireMs を過ぎたら使わない。
// - 作れなかった写真も同じ間は要求しない（要求のたびに作り直しが失敗し、警告の通知が重なるため）。
// - DOM を使わない（時刻は呼び出し側が渡す）。

import type { VariantId } from "../api/types";
import { RecentMap } from "./viewerMath";

/**
 * 作成待ち・失敗の印を使う時間（ミリ秒。仮置き）。過ぎたら、もう一度要求してよい（知らせの取りこぼし・
 * コアの作り直しの取り消しへの備え）。
 */
export const L1_WAIT_EXPIRE_MS = 60_000;

/** 覚えておく写真の数（仮置き。古いものから忘れる）。 */
export const L1_STATUS_CAPACITY = 2048;

/** L1 の状態: none（要求してよい）・waiting（作成待ち）・failed（作れなかった）。 */
export type L1State = "none" | "waiting" | "failed";

interface Mark {
  /** 404 になった URI の rev（L0 の rev。作り直されると変わる）。 */
  rev: string | null;
  /** 印を付けた時刻（ミリ秒）。 */
  since: number;
  failed: boolean;
}

export class L1Status {
  private seq = 0;
  /** 作り終えた知らせの通し番号（id ごとの最後の値）。 */
  private readonly updated: RecentMap<VariantId, number>;
  private readonly marks: RecentMap<VariantId, Mark>;

  constructor(
    readonly expireMs = L1_WAIT_EXPIRE_MS,
    capacity = L1_STATUS_CAPACITY,
  ) {
    this.updated = new RecentMap(capacity);
    this.marks = new RecentMap(capacity);
  }

  /** 要求を始める（戻り値を markMissing に渡す）。 */
  stamp(): number {
    return this.seq;
  }

  /**
   * L1 を読めなかった（404 など）。作成待ちにしたら true。要求を始めた後（`stamp` より後）に作り終えた
   * 知らせが届いていれば、作成待ちにしない（false。読み直せばよい）。
   */
  markMissing(id: VariantId, rev: string | null, stamp: number, now: number): boolean {
    const u = this.updated.get(id);
    if (u !== undefined && u > stamp) return false;
    const cur = this.marks.get(id);
    // 作れなかった印は、期限まで残す（作成待ちで上書きしない）。
    if (cur && cur.failed && now - cur.since < this.expireMs) return true;
    this.marks.set(id, { rev, since: now, failed: false });
    return true;
  }

  /** 作り終えた（thumbnail_updated）。印を消す。 */
  markUpdated(id: VariantId): void {
    this.seq++;
    this.updated.set(id, this.seq);
    this.marks.delete(id);
  }

  /** 作れなかった（preview_failed の警告）。 */
  markFailed(id: VariantId, now: number): void {
    const cur = this.marks.get(id);
    this.marks.set(id, { rev: cur?.rev ?? null, since: now, failed: true });
  }

  /**
   * 今の状態。作成待ちの印は、同じ rev のときだけ使う（rev が変われば作り直されたので要求してよい）。
   * 作れなかった印は rev によらない。期限を過ぎた印は消す。
   */
  state(id: VariantId, rev: string | null, now: number): L1State {
    const m = this.marks.get(id);
    if (!m) return "none";
    if (now - m.since >= this.expireMs) {
      this.marks.delete(id);
      return "none";
    }
    if (m.failed) return "failed";
    return m.rev === rev ? "waiting" : "none";
  }

  /** 1 枚の印を消す（作ってあることを確かめた）。 */
  clear(id: VariantId): void {
    this.marks.delete(id);
  }

  /** すべて忘れる（別のカタログを開いた）。 */
  reset(): void {
    this.marks.clear();
    this.updated.clear();
  }
}
