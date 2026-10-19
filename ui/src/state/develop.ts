// 現像のセッション（コアの open_develop で開いた 1 枚）と、設定の送信の順序。
//
// ルーペの等倍表示（表示範囲の描画）と現像の画面の両方が、同じセッションを使う（コアの現像のセッションは
// 1 つだけ）。
//
// # 設定の送信（04 の 6.2 節）
//
// スライダーを動かすたびに set_settings を送るが、Tauri のコマンドは別々のスレッドで実行されるため、
// 同時に送ると順序が入れ替わりうる（古い値が最新になってしまう）。そこで送信を 1 本の列にし、送信中に
// 来た値は「最新の 1 つ」だけを残して、前の送信が終わってから送る。ドラッグの終了（end_drag）・
// Undo / Redo も同じ列に入れて、順序を保つ。
//
// # ビフォー / アフター（DEV-31）
//
// ビフォーはコアの現像のセッションが持つ（set_before）。設定も同じ列で送り（写真を開いた後・設定の送信の後に
// 届くように）、描けたら before_updated で知らせが来るので、before_frame で画素を取る。写真を開き直すと
// コアのビフォーはやめた状態に戻るので、画面（DevelopView）は写真ごとに設定し直す。

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import { onCoreEvent } from "../api/events";
import { type Frame, fetchBeforeFrame, fetchDevelopFrame } from "../api/frames";
import type {
  BeforeInfo,
  BeforeSource,
  DevelopSettings,
  DevelopState,
  ErrorInfo,
  FrameInfo,
  VariantId,
} from "../api/types";
import { createStore, useStore } from "./store";

export interface DevelopStoreState {
  /** 開いている（開こうとしている）写真。 */
  variantId: VariantId | null;
  /** コアの状態（開けたら）。 */
  state: DevelopState | null;
  /** 画面の設定（送信前の値を含む最新の値）。 */
  settings: DevelopSettings | null;
  opening: boolean;
  error: ErrorInfo | null;
  /** 最新のプレビュー。 */
  frame: Frame<FrameInfo> | null;
  /** 履歴を読み直す合図。 */
  historyVersion: number;
  /** ビフォー / アフターの表示のビフォー（DEV-31。設定していなければ null）。 */
  before: BeforeInfo | null;
  /** ビフォーの最新のプレビュー。 */
  beforeFrame: Frame<FrameInfo> | null;
  /**
   * コアが最後に受け付けた設定（applySettings の通し番号 `seq` と、コアが割り当てた世代）。`seq` 以前に渡した設定は、
   * 世代 `generation` 以降のプレビュー・重ねて表示に入っている（後の設定は前の設定を含むため）。コアが受け付けなかった
   * （送信が失敗した）ら `generation` は null（`seq` 以前の、まだ受け付けていない設定は入っていない）。
   */
  settingsAck: { seq: number; generation: number | null } | null;
}

export const developStore = createStore<DevelopStoreState>({
  variantId: null,
  state: null,
  settings: null,
  opening: false,
  error: null,
  frame: null,
  historyVersion: 0,
  before: null,
  beforeFrame: null,
  settingsAck: null,
});

export function useDevelop<T>(selector: (s: DevelopStoreState) => T, equal?: (a: T, b: T) => boolean): T {
  return useStore(developStore, selector, equal);
}

// ---------------------------------------------------------------------------
// 写真を開く
// ---------------------------------------------------------------------------

let openSeq = 0;
let openPromise: Promise<DevelopState | null> | null = null;

/** 写真を現像のために開く（同じ写真なら開いているものを返す。最新の要求だけが反映される）。 */
export function openDevelopFor(variantId: VariantId): Promise<DevelopState | null> {
  const cur = developStore.get();
  if (cur.variantId === variantId && cur.state && !cur.opening) return Promise.resolve(cur.state);
  if (cur.variantId === variantId && openPromise) return openPromise;
  const seq = ++openSeq;
  developStore.set({
    variantId,
    state: null,
    settings: null,
    opening: true,
    error: null,
    frame: null,
    before: null,
    beforeFrame: null,
  });
  // 前の写真の送信待ちの値（送信中に来た最新の値）は、捨てずに列の後ろへ移して前の写真に送る（列が空いて
  // から開くので、別の写真には届かない）。
  flushPending();
  const p = (async () => {
    // 前の写真への送信が終わってから開く（順序を保つ）。
    await queueIdle();
    try {
      const state = await core.openDevelop(variantId);
      if (seq !== openSeq) return null;
      developStore.set({ state, settings: state.settings, opening: false, before: state.before ?? null });
      void refreshFrame();
      return state;
    } catch (e) {
      if (seq !== openSeq) return null;
      const err = toApiError(e);
      if (err.kind === "cancelled") return null;
      developStore.set({ opening: false, error: err.info });
      return null;
    }
  })();
  openPromise = p;
  void p.finally(() => {
    if (openPromise === p) openPromise = null;
  });
  return p;
}

/** 写真を閉じる（保存待ちの変更を保存する）。 */
export async function closeDevelopSession(): Promise<void> {
  openSeq++;
  // 送信待ちの最新の値は、閉じる前に送る（コアは閉じるときに保存する）。
  flushPending();
  await queueIdle();
  developStore.set({
    variantId: null,
    state: null,
    settings: null,
    opening: false,
    error: null,
    frame: null,
    before: null,
    beforeFrame: null,
  });
  try {
    await core.closeDevelop();
  } catch (e) {
    console.warn("現像の写真を閉じられません", e);
  }
}

// ---------------------------------------------------------------------------
// 設定の送信の列
// ---------------------------------------------------------------------------

type Op =
  | { kind: "settings"; settings: DevelopSettings; dragging: boolean; seq: number }
  | { kind: "end_drag" }
  | { kind: "call"; run: () => Promise<void> };

let running = false;
/** applySettings の通し番号（最後に渡した設定の番号）。 */
let applySeq = 0;
/** 送信待ちの設定（最新の 1 つ）。 */
let pending: Op | null = null;
/** 設定の後に順に実行する操作（end_drag・Undo など）。 */
const tail: Op[] = [];
const idleWaiters: (() => void)[] = [];

/** 送信待ちの設定を、列の後ろ（今の写真への操作の続き）へ移して送り始める。 */
function flushPending(): void {
  if (pending) {
    tail.push(pending);
    pending = null;
    void pump();
  }
}

function queueIdle(): Promise<void> {
  if (!running && !pending && tail.length === 0) return Promise.resolve();
  return new Promise((r) => idleWaiters.push(r));
}

async function pump(): Promise<void> {
  if (running) return;
  running = true;
  try {
    for (;;) {
      // tail には pending より古い操作だけが入る（end_drag・enqueue が pending を先に tail へ移すため）。
      let op: Op | undefined = tail.shift();
      if (!op && pending) {
        op = pending;
        pending = null;
      }
      if (!op) break;
      try {
        await runOp(op);
        // 操作が成功したら、前の操作の失敗の表示を消す（開けなかったときのエラーは state が null のときだけ
        // なので消さない）。
        if (developStore.get().error && developStore.get().state) developStore.set({ error: null });
      } catch (e) {
        if (op.kind === "settings") developStore.set({ settingsAck: { seq: op.seq, generation: null } });
        const err = toApiError(e);
        if (err.kind !== "cancelled") {
          console.error("現像の操作に失敗", err.info);
          developStore.set({ error: err.info });
        }
      }
    }
  } finally {
    running = false;
    for (const w of idleWaiters.splice(0)) w();
  }
}

async function runOp(op: Op): Promise<void> {
  switch (op.kind) {
    case "settings": {
      const generation = await core.setSettings(op.settings, op.dragging);
      developStore.set({ settingsAck: { seq: op.seq, generation } });
      break;
    }
    case "end_drag": {
      const state = await core.endDrag();
      developStore.set((s) => ({ state, before: beforeOf(state, s), historyVersion: s.historyVersion + 1 }));
      break;
    }
    case "call":
      await op.run();
      break;
  }
}

/**
 * 設定を変える（画面の値はすぐに変え、コアには順に送る）。
 *
 * - スライダーのドラッグ中は `dragging: true` で送り、離したら [`endDrag`] を呼ぶ（履歴に 1 件）。
 * - ドラッグでない変更（数値の入力・プリセット）は `dragging: false`（コアが 1 秒後に保存して履歴に記録）。
 *
 * 戻り値は通し番号（コアが受け付けたら [`DevelopStoreState.settingsAck`] に出る）。写真を開いていなければ null。
 */
export function applySettings(settings: DevelopSettings, dragging: boolean): number | null {
  const s = developStore.get();
  if (!s.state) return null;
  developStore.set({ settings });
  applySeq += 1;
  pending = { kind: "settings", settings, dragging, seq: applySeq };
  void pump();
  return applySeq;
}

/** ドラッグを終える（最後の値を送ってから。履歴に 1 件記録して保存）。 */
export function endDrag(): void {
  if (!developStore.get().state) return;
  if (pending && pending.kind === "settings") {
    tail.unshift(pending);
    pending = null;
  }
  tail.push({ kind: "end_drag" });
  void pump();
}

/** 列の順序を保って、任意の操作を実行する（Undo / Redo・貼り付けなど）。 */
export function enqueue(run: () => Promise<void>): Promise<void> {
  return new Promise((resolve, reject) => {
    if (pending && pending.kind === "settings") {
      tail.push(pending);
      pending = null;
    }
    tail.push({
      kind: "call",
      run: async () => {
        try {
          await run();
          resolve();
        } catch (e) {
          reject(e);
          throw e;
        }
      },
    });
    void pump();
  });
}

/** 取り消す（DEV-27）。 */
export function undo(): Promise<void> {
  return enqueue(async () => {
    const id = developStore.get().variantId;
    if (id === null) return;
    const settings = await core.undo(id);
    const state = await core.developState();
    developStore.set((s) => ({
      settings: settings ?? s.settings,
      state: state ?? s.state,
      before: beforeOf(state, s),
      historyVersion: s.historyVersion + 1,
    }));
  });
}

/** やり直す（DEV-27）。 */
export function redo(): Promise<void> {
  return enqueue(async () => {
    const id = developStore.get().variantId;
    if (id === null) return;
    const settings = await core.redo(id);
    const state = await core.developState();
    developStore.set((s) => ({
      settings: settings ?? s.settings,
      state: state ?? s.state,
      before: beforeOf(state, s),
      historyVersion: s.historyVersion + 1,
    }));
  });
}

/**
 * 現像中の写真の処理バージョンを最新に更新する（DATA-09。レンズ補正などは処理バージョン 2 から）。履歴に
 * 1 件記録され、Undo で戻せる。送信待ちの設定を先に送ってから実行する。
 */
export function upgradeProcessVersion(): Promise<void> {
  const d = developStore.get();
  if (!d.state || !d.settings) return Promise.resolve();
  const variantId = d.state.variant_id;
  const old = d.settings.process_version;
  const latest = d.state.latest_process_version;
  if (latest !== undefined && old < latest) {
    // 更新を待つ間に変えた値も新しい版で送る（古い版のままの設定を後から送って、更新を戻さないように）。
    developStore.set({ settings: { ...d.settings, process_version: latest } });
  }
  return enqueue(async () => {
    let state: DevelopState;
    try {
      state = await core.upgradeProcessVersion();
    } catch (e) {
      // 更新できなければ、画面の版を戻す（コアの設定は古い版のまま）。
      const s = developStore.get();
      if (s.settings && s.state?.variant_id === variantId) {
        developStore.set({ settings: { ...s.settings, process_version: old } });
      }
      throw e;
    }
    if (state.variant_id !== developStore.get().variantId) return;
    // 更新の後に利用者が値を変えていたら（送信待ちがある）、画面の値は上書きしない。
    const busy = pending !== null || tail.length > 0;
    developStore.set((s) => ({
      state,
      settings: busy ? s.settings : state.settings,
      before: beforeOf(state, s),
      historyVersion: s.historyVersion + 1,
    }));
  });
}

/** コアの状態を読み直す（一括適用などで、開いている写真の設定が外から変わったとき）。 */
export function reloadState(): Promise<void> {
  return enqueue(async () => {
    const state = await core.developState();
    if (!state || state.variant_id !== developStore.get().variantId) return;
    // 読み直しの間に利用者が値を変えていたら（送信待ちがある・ドラッグ中）、画面の値は上書きしない
    // （古い値に戻って見えるため）。履歴・Undo / Redo の可否だけを更新する。
    const busy = pending !== null || tail.length > 0 || state.dragging;
    developStore.set((s) => ({
      state,
      settings: busy ? s.settings : state.settings,
      before: beforeOf(state, s),
      historyVersion: s.historyVersion + 1,
    }));
  });
}

// ---------------------------------------------------------------------------
// プレビューの受け取り
// ---------------------------------------------------------------------------

let fetching = false;
let fetchAgain = false;

/** 最新のプレビューを取りにいく（取得中に更新が来たら、終わってからもう一度）。 */
export async function refreshFrame(): Promise<void> {
  if (fetching) {
    fetchAgain = true;
    return;
  }
  fetching = true;
  try {
    do {
      fetchAgain = false;
      const frame = await fetchDevelopFrame();
      const s = developStore.get();
      if (frame && frame.header.variant_id === s.variantId) {
        const cur = s.frame;
        if (!cur || frame.header.generation >= cur.header.generation) developStore.set({ frame });
      }
    } while (fetchAgain);
  } catch (e) {
    const err = toApiError(e);
    if (err.kind !== "cancelled" && err.kind !== "closed") console.error("プレビューを取得できません", err.info);
  } finally {
    fetching = false;
  }
}

// ---------------------------------------------------------------------------
// ビフォー / アフター（DEV-31）
// ---------------------------------------------------------------------------

/**
 * コアの状態のビフォー（開いている写真の状態のときだけ。古いコアで無ければ今の値のまま）。切り抜き・角度を
 * Undo などで変えると、コアはビフォーを描き直して世代を進める。
 */
function beforeOf(state: DevelopState | null, s: DevelopStoreState): BeforeInfo | null {
  if (!state || state.variant_id !== s.variantId || state.before === undefined) return s.before;
  return state.before;
}

/**
 * ビフォーを設定する（null でやめる。設定の送信の列の後ろで）。開いている写真が変わっていたら何もしない。
 * 失敗したら知らせずに例外を返す（呼び出し側が知らせる）。
 */
export function setBeforeSource(source: BeforeSource | null): Promise<void> {
  return enqueue(async () => {
    const id = developStore.get().variantId;
    if (id === null || !developStore.get().state) return;
    const info = await core.setBefore(source);
    if (developStore.get().variantId !== id) return;
    developStore.set((s) => ({
      before: info,
      // やめたら画素も捨てる。出どころを変えたら、新しいビフォーが届くまで前の画素を出しておく（描画中の印は
      // 世代の違いで出す。beforePending）。
      beforeFrame: info ? s.beforeFrame : null,
    }));
    if (info) void refreshBeforeFrame();
  });
}

/** 新しいビフォーを描いている途中か（設定したビフォーの世代の画素がまだ届いていない）。 */
export function beforePending(s: Pick<DevelopStoreState, "before" | "beforeFrame" | "variantId">): boolean {
  if (!s.before) return false;
  const f = s.beforeFrame;
  return !f || f.header.variant_id !== s.variantId || f.header.generation < s.before.generation;
}

let fetchingBefore = false;
let fetchBeforeAgain = false;

/** ビフォーの最新のプレビューを取りにいく（取得中に更新が来たら、終わってからもう一度）。 */
export async function refreshBeforeFrame(): Promise<void> {
  if (fetchingBefore) {
    fetchBeforeAgain = true;
    return;
  }
  fetchingBefore = true;
  try {
    do {
      fetchBeforeAgain = false;
      const frame = await fetchBeforeFrame();
      const s = developStore.get();
      if (frame && s.before && frame.header.variant_id === s.variantId) {
        const cur = s.beforeFrame;
        if (!cur || cur.header.variant_id !== s.variantId || frame.header.generation >= cur.header.generation) {
          developStore.set({ beforeFrame: frame });
        }
      }
    } while (fetchBeforeAgain);
  } catch (e) {
    const err = toApiError(e);
    if (err.kind !== "cancelled" && err.kind !== "closed") console.error("ビフォーを取得できません", err.info);
  } finally {
    fetchingBefore = false;
  }
}

let installed = false;

/** プレビューの更新・カタログの変更のイベントを受け取る（起動時に 1 回）。 */
export function installDevelopEvents(): void {
  if (installed) return;
  installed = true;
  onCoreEvent((e) => {
    if (e.type === "preview_updated" && e.variant_id === developStore.get().variantId) {
      void refreshFrame();
    }
    if (e.type === "before_updated" && e.variant_id === developStore.get().variantId && developStore.get().before) {
      void refreshBeforeFrame();
    }
    if (
      e.type === "catalog_changed" &&
      e.change === "develop" &&
      !developStore.get().state?.dragging &&
      developStore.get().variantId !== null &&
      (e.all || e.variant_ids.includes(developStore.get().variantId as VariantId))
    ) {
      // 自動保存・一括適用などで、開いている写真の設定・履歴が変わった。コアの状態（設定・Undo / Redo の
      // 可否・保存待ち）を読み直す（送信の列の後ろで。送信待ちの値は上書きしない）。
      void reloadState().catch(() => undefined);
    }
  });
}
