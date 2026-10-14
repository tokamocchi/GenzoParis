// 動画の再生（VID-04）の計算。DOM を使わない純粋な関数（VideoPlayer.tsx が使う）。
//
// - 元のファイルを WebView で再生できるか: `canPlayType` に、コンテナ（MP4・MOV はどちらも ISO BMFF なので
//   video/mp4 で問う）とコーデック（ffprobe のコーデック名とビット深度から作る RFC 6381 の codecs）を渡す
//   （canPlayQuery）。"" なら再生できない、"maybe"・"probably" なら試す（再生の失敗は error の事件で分かる）。
// - どのファイルで再生するか（decideSource）: 元のファイル → 再生できなければ再生用のファイル（あれば）→
//   なければ作る（ffmpeg があれば）→ 作れなければ理由。元のファイルが見つからなくても、再生用のファイルが
//   あれば再生できる。
// - J / K / L（shuttle）: L で前へ（押すたびに 1・2・4・8 倍）、J で後ろへ（−1・−2・−4・−8 倍）、K で止める。
//   逆方向に押すと、その向きの 1 倍から。後ろへの再生は、WebView の video 要素が負の速さに対応しないので、
//   止めた上で一定の間隔で位置を戻す（reverseStep）。
// - コマ送り（stepFrame）: フレームレート（不明なら 30 fps とみなす）の 1 フレームぶん。フレームの境目にちょうど
//   合わせると、丸めで前のフレームが出ることがあるので、フレームの始まりから少しだけ後ろにする。

import type { VideoPlayback } from "../api/types";

/** J / K / L の速さの上限（倍。**仮置き**: 8）。 */
export const SHUTTLE_MAX = 8;

/** フレームレートが分からないときのフレームレート（fps。**仮置き**: 30）。 */
export const DEFAULT_FPS = 30;

/** 後ろへの再生で位置を戻す間隔（ミリ秒。**仮置き**: 100）。 */
export const REVERSE_TICK_MS = 100;

/** 再生用のファイルを作り始めるまでの待ち（ミリ秒。**仮置き**: 1000。動画を次々に送る間は作り始めない）。 */
export const PROXY_START_DELAY_MS = 1000;

/** J / K / L の次の速さ（0 は止める。負は後ろへ）。 */
export function shuttle(current: number, key: "j" | "k" | "l"): number {
  if (key === "k") return 0;
  const dir = key === "l" ? 1 : -1;
  if (Math.sign(current) !== dir) return dir;
  return dir * Math.min(SHUTTLE_MAX, Math.abs(current) * 2);
}

/** 速さの表示（"2×"・"−1×"。止まっていれば ""）。 */
export function shuttleLabel(speed: number): string {
  if (speed === 0) return "";
  return speed < 0 ? `−${-speed}×` : `${speed}×`;
}

/** 使うフレームレート（不明・不正なら DEFAULT_FPS）。 */
export function effectiveFps(fps: number | null | undefined): number {
  return fps && Number.isFinite(fps) && fps > 0 && fps <= 1000 ? fps : DEFAULT_FPS;
}

function clampTime(t: number, duration: number): number {
  const max = Number.isFinite(duration) && duration > 0 ? duration : Number.POSITIVE_INFINITY;
  return Math.max(0, Math.min(max, t));
}

/**
 * シークする位置（秒。0 から長さまでに収める。長さが分からなければ上限なし）。有限の位置にならなければ null
 * （読み込む前の End など。video 要素の currentTime に NaN・Infinity を入れると TypeError になる）。
 */
export function seekTarget(time: number, duration: number): number | null {
  const t = clampTime(time, duration);
  return Number.isFinite(t) ? t : null;
}

/** 1 フレーム前・後の位置（秒。`dir` は −1 / +1）。フレームの始まりから 1/4 フレーム後ろにする。 */
export function stepFrame(time: number, fps: number | null | undefined, dir: 1 | -1, duration: number): number {
  const f = effectiveFps(fps);
  const current = Math.floor(Math.max(0, time) * f + 1e-6);
  const next = Math.max(0, current + dir);
  const t = (next + 0.25) / f;
  // 最後のフレームより後ろには進めない（終わりの位置に止める）。
  return clampTime(t, duration);
}

/** 後ろへの再生の 1 回ぶんの位置（`speed` は負。`dtMs` は前の回からの時間）。 */
export function reverseStep(time: number, speed: number, dtMs: number): number {
  return Math.max(0, time - (Math.abs(speed) * Math.max(0, dtMs)) / 1000);
}

/** 再生の位置の表示（"0:05"・"1:02:03"。分からなければ "--:--"）。 */
export function formatPlaybackTime(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds) || seconds < 0) return "--:--";
  const t = Math.floor(seconds);
  const h = Math.floor(t / 3600);
  const m = Math.floor((t % 3600) / 60);
  const s = t % 60;
  const p = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${p(m)}:${p(s)}` : `${m}:${p(s)}`;
}

/**
 * コーデック（ffprobe の codec_name）とビット深度の、canPlayType の codecs（RFC 6381）。分からないコーデックは
 * null（codecs を付けずに問う）。値は代表的なプロファイル・レベル（H.264 は High・High 10 の 5.1、H.265 は
 * Main・Main 10 の 5.1）。
 */
export function codecsFor(codec: string | null | undefined, bitDepth: number | null | undefined): string | null {
  const ten = (bitDepth ?? 8) > 8;
  switch ((codec ?? "").toLowerCase()) {
    case "h264":
      return ten ? "avc1.6e0033" : "avc1.640033";
    case "hevc":
    case "h265":
      return ten ? "hvc1.2.4.L153.B0" : "hvc1.1.6.L153.B0";
    case "mpeg4":
      return "mp4v.20.9";
    case "av1":
      return ten ? "av01.0.13M.10" : "av01.0.13M.08";
    case "vp9":
      return ten ? "vp09.02.51.10" : "vp09.00.51.08";
    default:
      return null;
  }
}

/** canPlayType に渡す文字列（MP4・MOV は video/mp4 で問う）。 */
export function canPlayQuery(codec: string | null | undefined, bitDepth: number | null | undefined): string {
  const codecs = codecsFor(codec, bitDepth);
  return codecs ? `video/mp4; codecs="${codecs}"` : "video/mp4";
}

/** canPlayType の結果。 */
export type Playability = "probably" | "maybe" | "no";

export function playabilityOf(result: string | null | undefined): Playability {
  return result === "probably" ? "probably" : result === "maybe" ? "maybe" : "no";
}

/** どのファイルで再生するか。 */
export type SourceDecision =
  | { kind: "original" }
  | { kind: "proxy" }
  /** 再生用のファイルを作る（作れる。まだない）。 */
  | { kind: "create_proxy"; reason: "unsupported" | "failed" | "chosen" }
  /** 再生できない（再生用のファイルも作れない）。 */
  | { kind: "unplayable"; reason: string }
  /** 元のファイルを読めない（再生用のファイルもない）。 */
  | { kind: "missing"; reason: string };

/**
 * どのファイルで再生するかを決める。`playability` は元のファイルの canPlayType の結果、`originalFailed` は元の
 * ファイルの再生が失敗したか、`preferProxy` は利用者が再生用のファイルを選んだか。
 */
export function decideSource(
  p: VideoPlayback,
  playability: Playability,
  originalFailed: boolean,
  preferProxy: boolean,
): SourceDecision {
  const canCreate = p.proxy_unavailable === null && p.available;
  if (!p.available) {
    if (p.proxy) return { kind: "proxy" };
    return { kind: "missing", reason: p.unavailable_reason ?? "元の動画を読めません" };
  }
  if (preferProxy) {
    if (p.proxy) return { kind: "proxy" };
    if (canCreate) return { kind: "create_proxy", reason: "chosen" };
  }
  if (originalFailed || playability === "no") {
    if (p.proxy) return { kind: "proxy" };
    if (canCreate) return { kind: "create_proxy", reason: originalFailed ? "failed" : "unsupported" };
    return { kind: "unplayable", reason: p.proxy_unavailable ?? "再生用のファイルを作れません" };
  }
  return { kind: "original" };
}

/** video 要素の MediaError の code の説明。 */
export function mediaErrorMessage(code: number | null | undefined): string {
  switch (code) {
    case 1:
      return "読み込みが中断されました";
    case 2:
      return "ファイルを読み込めませんでした";
    case 3:
      return "動画をデコードできませんでした（壊れているか、この PC で扱えない形式です）";
    case 4:
      return "この形式の動画は、この PC の WebView で再生できません";
    default:
      return "動画を再生できません";
  }
}

/** 音量の保存（この PC のブラウザの保存領域。読めなければ既定の 1・消音なし）。 */
export const VOLUME_KEY = "genzo.video.volume";
export const MUTED_KEY = "genzo.video.muted";

export function parseVolume(raw: string | null | undefined): number {
  const v = raw === null || raw === undefined || raw.trim() === "" ? Number.NaN : Number(raw);
  return Number.isFinite(v) ? Math.max(0, Math.min(1, v)) : 1;
}

export function parseMuted(raw: string | null | undefined): boolean {
  return raw === "true";
}
