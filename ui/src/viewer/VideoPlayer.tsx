// 動画の再生（VID-04）: ルーペの動画の表示（再生・一時停止・シーク・コマ送り・音量・J / K / L・スペース）。
//
// - 元のファイルを genzo-app の URI スキーム（genzo://video/{id}。Range の要求に応える）から、WebView の video
//   要素で再生する（デコードは WebView。OS の動画の機能）。カタログに登録された動画だけを配信する（パスは渡さない）。
// - 再生できない形式（canPlayType が "" か、再生が失敗した）は、利用者の ffmpeg があれば、再生用のファイル
//   （H.264 ＋ AAC の MP4。長辺 1280。genzo-api の playback）を作って再生する。作り始めるのは、動画を開いてから
//   PROXY_START_DELAY_MS 後（動画を次々に送る間は作り始めない）。作っている途中に別の写真・動画へ移ったら取り消す
//   （コアも別の動画の作成を頼まれたら、前のものを取り消す）。作ったものはキャッシュに残り、次からすぐ再生できる。
//   ffmpeg がない・H.264 のエンコーダーがなければ、理由を出す（OS のアプリで再生はできる）。
// - 元のファイルが再生できても、色・コマがおかしいとき（OS のデコーダーの不具合など）のために、再生用のファイルに
//   切り替えられる（その逆も）。切り替えても、再生の位置はそのまま。
// - キー（ルーペで、ダイアログが開いていない間。一覧は dialogs/shortcutsData.ts）: スペースで再生・一時停止、
//   L で前へ（押すたびに 1・2・4・8 倍）、J で後ろへ（−1・−2・−4・−8 倍。止めて位置を戻していく）、K で止める、
//   「,」「.」で 1 コマ戻る・進む、M で消音、Home・End で先頭・最後へ。← → は写真送りのまま。
// - 音量と消音は、この PC のブラウザの保存領域に覚える（読めなければ既定の 1・消音なし）。

import { type SyntheticEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as appApi from "../api/app";
import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { VariantDetails, VariantSummary, VideoPlayback } from "../api/types";
import { thumbUrl, videoProxyUrl, videoUrl } from "../api/uri";
import { Progress } from "../components/Progress";
import { useHotkeys } from "../keyboard";
import { actions, useApp } from "../state/app";
import {
  MUTED_KEY,
  PROXY_START_DELAY_MS,
  REVERSE_TICK_MS,
  VOLUME_KEY,
  type Playability,
  canPlayQuery,
  decideSource,
  effectiveFps,
  formatPlaybackTime,
  mediaErrorMessage,
  parseMuted,
  parseVolume,
  playabilityOf,
  reverseStep,
  seekTarget,
  shuttle,
  shuttleLabel,
  stepFrame,
} from "./videoLogic";
import { formatCodec, formatFps, formatResolution } from "./viewerMath";

function readStorage(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStorage(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // 保存できなくても再生はできる。
  }
}

/** 元のファイルを WebView で再生できるか（canPlayType。DOM がなければ「試す」）。 */
function playabilityFor(p: VideoPlayback): Playability {
  if (typeof document === "undefined") return "maybe";
  try {
    const v = document.createElement("video");
    return playabilityOf(v.canPlayType(canPlayQuery(p.video?.codec, p.video?.bit_depth)));
  } catch {
    return "maybe";
  }
}

/** ボタンを押した後にフォーカスを残さない（スペースのキーで、ボタンのクリックと再生の切り替えが重ならないように）。 */
function blurAfter(fn: () => void) {
  return (e: SyntheticEvent<HTMLElement>) => {
    fn();
    e.currentTarget.blur();
  };
}

export function VideoPlayer(props: {
  summary: VariantSummary;
  details: VariantDetails | null;
  thumbRev: string | null;
  hotkeysEnabled: boolean;
}) {
  const { summary } = props;
  const id = summary.variant_id;
  const videoRef = useRef<HTMLVideoElement>(null);
  const mounted = useRef(true);
  const tools = useApp((s) => s.catalog?.tools ?? null);

  const [playback, setPlayback] = useState<VideoPlayback | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [reloadTick, setReloadTick] = useState(0);
  const [originalFailed, setOriginalFailed] = useState(false);
  const [preferProxy, setPreferProxy] = useState(false);
  const [proxyJob, setProxyJob] = useState<number | null>(null);
  const [proxyError, setProxyError] = useState<string | null>(null);
  const [proxyCancelled, setProxyCancelled] = useState(false);
  const [playError, setPlayError] = useState<string | null>(null);
  const [time, setTime] = useState(0);
  const [duration, setDuration] = useState(Number.NaN);
  const [paused, setPaused] = useState(true);
  const [speed, setSpeed] = useState(0);
  const [waiting, setWaiting] = useState(false);
  const [volume, setVolume] = useState(() => parseVolume(readStorage(VOLUME_KEY)));
  const [muted, setMuted] = useState(() => parseMuted(readStorage(MUTED_KEY)));
  /** 自分で作り始めた再生用のファイルのジョブ（離れたら取り消す）。 */
  const ownJob = useRef<number | null>(null);
  /** 次に読み込んだファイルで移る位置（元のファイルと再生用のファイルを切り替えたとき）。 */
  const pendingSeek = useRef<number | null>(null);
  const job = useApp((s) => (proxyJob !== null ? s.jobs[proxyJob] : undefined));

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      const j = ownJob.current;
      ownJob.current = null;
      if (j !== null) void core.cancelJob(j).catch(() => undefined);
    };
  }, []);

  /** 再生用のファイルのジョブを見守る（終わったら、作れたら再生の情報を読み直す）。 */
  const attach = useCallback(
    (jobId: number, own: boolean) => {
      setProxyJob(jobId);
      setProxyError(null);
      setProxyCancelled(false);
      if (own) ownJob.current = jobId;
      actions.trackJob(jobId, "video_proxy", `再生用のファイルの作成: ${summary.file_name}`);
      actions
        .waitJob(jobId)
        .then((info) => {
          if (ownJob.current === jobId) ownJob.current = null;
          if (!mounted.current) return;
          setProxyJob(null);
          if (info.state === "succeeded") setReloadTick((t) => t + 1);
          else if (info.state === "failed") setProxyError(info.error?.message ?? "再生用のファイルを作れませんでした");
          else setProxyCancelled(true);
        })
        .catch(() => undefined);
    },
    [summary.file_name],
  );

  // 再生の情報（元のファイルを読めるか・再生用のファイル・作れない理由）。作っている途中なら見守る。
  useEffect(() => {
    let alive = true;
    core
      .videoPlayback(id)
      .then((p) => {
        if (!alive) return;
        setPlayback(p);
        setLoadError(null);
        if (p.proxy_job !== null) attach(p.proxy_job, false);
      })
      .catch((e) => {
        if (alive) setLoadError(toApiError(e).info.message);
      });
    return () => {
      alive = false;
    };
  }, [id, reloadTick, attach]);

  const playability: Playability = useMemo(() => (playback ? playabilityFor(playback) : "maybe"), [playback]);
  const decision = playback ? decideSource(playback, playability, originalFailed, preferProxy) : null;
  const wantCreate = decision?.kind === "create_proxy" && proxyJob === null && proxyError === null && !proxyCancelled;

  // 再生用のファイルを作る（少し待ってから。待つ間に離れたら作らない）。
  useEffect(() => {
    if (!wantCreate) return;
    const timer = setTimeout(() => {
      core
        .createVideoProxy(id)
        .then((jobId) => {
          if (mounted.current) attach(jobId, true);
          else void core.cancelJob(jobId).catch(() => undefined);
        })
        .catch((e) => {
          if (mounted.current) setProxyError(toApiError(e).info.message);
        });
    }, PROXY_START_DELAY_MS);
    return () => clearTimeout(timer);
  }, [wantCreate, id, attach]);

  const src =
    decision?.kind === "original"
      ? videoUrl(id)
      : decision?.kind === "proxy" && playback?.proxy
        ? videoProxyUrl(id, playback.proxy.size)
        : null;
  const usingProxy = decision?.kind === "proxy";
  const poster = props.thumbRev ? thumbUrl(id, props.thumbRev) : undefined;
  const video = playback?.video ?? (props.details?.summary.variant_id === id ? props.details.video : null) ?? null;
  const fps = effectiveFps(video?.fps);

  // 読み込むファイルが変わったら、再生の状態を初めに戻す（video 要素は key で作り直す）。
  useEffect(() => {
    setTime(pendingSeek.current ?? 0);
    setDuration(Number.NaN);
    setPaused(true);
    setSpeed(0);
    setWaiting(false);
    setPlayError(null);
  }, [src]);

  // 後ろへの再生（止めた上で、一定の間隔で位置を戻す）。
  useEffect(() => {
    if (speed >= 0) return;
    let last = performance.now();
    const timer = setInterval(() => {
      const v = videoRef.current;
      if (!v) return;
      const now = performance.now();
      const t = reverseStep(v.currentTime, speed, now - last);
      last = now;
      v.currentTime = t;
      setTime(t);
      if (t <= 0) setSpeed(0);
    }, REVERSE_TICK_MS);
    return () => clearInterval(timer);
  }, [speed]);

  /** 速さを変える（正は前へ、負は後ろへ、0 は止める）。 */
  const applySpeed = (next: number) => {
    const v = videoRef.current;
    if (!v) return;
    setSpeed(next);
    if (next > 0) {
      v.playbackRate = next;
      v.play().catch((e: unknown) => {
        // 一時停止で再生の開始が中断されたもの（AbortError）は誤りではない。
        if (!(e instanceof DOMException && e.name === "AbortError")) setPlayError(mediaErrorMessage(v.error?.code));
      });
    } else {
      v.pause();
    }
  };
  const togglePlay = () => applySpeed(speed !== 0 || !paused ? 0 : 1);
  const seekTo = (t: number) => {
    const v = videoRef.current;
    if (!v) return;
    // 読み込む前（長さが NaN）の End などは何もしない（NaN を入れると TypeError になる）。
    const target = seekTarget(t, v.duration);
    if (target === null) return;
    v.currentTime = target;
    setTime(v.currentTime);
  };
  const step = (dir: 1 | -1) => {
    const v = videoRef.current;
    if (!v) return;
    applySpeed(0);
    seekTo(stepFrame(v.currentTime, fps, dir, v.duration));
  };
  const toggleMute = () => {
    const v = videoRef.current;
    if (v) v.muted = !v.muted;
  };
  const changeVolume = (value: number) => {
    const v = videoRef.current;
    if (!v) return;
    v.volume = value;
    if (value > 0 && v.muted) v.muted = false;
  };
  /** 元のファイルと再生用のファイルを切り替える（位置はそのまま）。 */
  const switchSource = (proxy: boolean) => {
    const v = videoRef.current;
    pendingSeek.current = v ? v.currentTime : null;
    setPreferProxy(proxy);
    if (!proxy) setOriginalFailed(false);
  };
  const retryProxy = () => {
    setProxyError(null);
    setProxyCancelled(false);
  };
  const cancelProxy = () => {
    if (proxyJob !== null) void core.cancelJob(proxyJob).catch((e) => actions.reportError(e, "取り消せません"));
  };
  const openInOs = () => {
    appApi.openVariantFile(id).catch((e) => actions.reportError(e, "動画を開けません"));
  };
  const openSettings = () => actions.openDialog({ kind: "settings", tab: "general" });

  useHotkeys(
    {
      Space: togglePlay,
      k: () => applySpeed(0),
      l: () => applySpeed(shuttle(speed, "l")),
      j: () => applySpeed(shuttle(speed, "j")),
      ",": () => step(-1),
      ".": () => step(1),
      m: toggleMute,
      Home: () => seekTo(0),
      End: () => seekTo(videoRef.current?.duration ?? Number.NaN),
    },
    props.hotkeysEnabled && src !== null,
  );

  const onLoadedMetadata = () => {
    const v = videoRef.current;
    if (!v) return;
    v.volume = volume;
    v.muted = muted;
    setDuration(v.duration);
    if (pendingSeek.current !== null) {
      v.currentTime = Math.min(pendingSeek.current, Number.isFinite(v.duration) ? v.duration : pendingSeek.current);
      pendingSeek.current = null;
    }
    setTime(v.currentTime);
  };
  const onError = () => {
    const v = videoRef.current;
    if (decision?.kind === "original") {
      // 元のファイルを再生できない: 再生用のファイルへ（あれば使い、なければ作る）。
      pendingSeek.current = v && Number.isFinite(v.currentTime) ? v.currentTime : null;
      setOriginalFailed(true);
    } else {
      setPlayError(mediaErrorMessage(v?.error?.code));
    }
  };
  const onVolumeChange = () => {
    const v = videoRef.current;
    if (!v) return;
    setVolume(v.volume);
    setMuted(v.muted);
    writeStorage(VOLUME_KEY, String(v.volume));
    writeStorage(MUTED_KEY, String(v.muted));
  };

  const codecText = formatCodec(video?.codec, video?.bit_depth);
  const canSwitchToProxy =
    !usingProxy && playback !== null && playback.proxy_unavailable === null && playback.available && decision?.kind === "original";
  const canSwitchToOriginal = usingProxy && playback?.available === true && playability !== "no" && preferProxy;

  return (
    <div className="loupe-video">
      <div className="loupe-view video">
        {src ? (
          <video
            key={src}
            ref={videoRef}
            className="loupe-video-el"
            src={src}
            poster={poster}
            preload="metadata"
            playsInline
            onClick={togglePlay}
            onLoadedMetadata={onLoadedMetadata}
            onDurationChange={() => setDuration(videoRef.current?.duration ?? Number.NaN)}
            onTimeUpdate={() => {
              if (speed >= 0) setTime(videoRef.current?.currentTime ?? 0);
            }}
            onPlay={() => {
              setPaused(false);
              setSpeed((s) => (s > 0 ? s : 1));
            }}
            onPause={() => {
              setPaused(true);
              // 後ろへの再生は止めた上で行うので、その間の一時停止では速さを変えない。
              setSpeed((s) => (s > 0 ? 0 : s));
            }}
            onEnded={() => {
              setPaused(true);
              setSpeed(0);
            }}
            onWaiting={() => setWaiting(true)}
            onPlaying={() => setWaiting(false)}
            onCanPlay={() => setWaiting(false)}
            onVolumeChange={onVolumeChange}
            onError={onError}
          />
        ) : (
          poster && <img className="loupe-video-poster" src={poster} alt="" draggable={false} />
        )}
        {src && paused && speed === 0 && !playError && (
          <span className="loupe-video-mark" aria-hidden="true">
            ▶
          </span>
        )}
        {waiting && !paused && (
          <div className="loupe-center-note">
            <div className="spinner" />
          </div>
        )}
        <VideoNotice
          playback={playback}
          loadError={loadError}
          decision={decision}
          codecText={codecText}
          job={job ? { done: job.done, total: job.total } : proxyJob !== null ? { done: 0, total: 0 } : null}
          proxyError={proxyError}
          proxyCancelled={proxyCancelled}
          playError={playError}
          ffmpegMissing={tools !== null && (!tools.ffmpeg || !tools.ffprobe)}
          onCancel={cancelProxy}
          onRetry={retryProxy}
          onSettings={openSettings}
          onOpenInOs={openInOs}
        />
      </div>
      <div className="loupe-video-controls">
        <button
          className="btn ghost icon"
          onClick={blurAfter(() => step(-1))}
          disabled={!src}
          title="1 コマ戻る（,）"
          aria-label="1 コマ戻る"
        >
          ◀|
        </button>
        <button
          className="btn icon video-play"
          onClick={blurAfter(togglePlay)}
          disabled={!src}
          title="再生 / 一時停止（スペース。J・K・L で速さを変える）"
          aria-label={paused ? "再生" : "一時停止"}
        >
          {paused ? "▶" : "❚❚"}
        </button>
        <button
          className="btn ghost icon"
          onClick={blurAfter(() => step(1))}
          disabled={!src}
          title="1 コマ進む（.）"
          aria-label="1 コマ進む"
        >
          |▶
        </button>
        <span className="video-speed" aria-live="polite">
          {shuttleLabel(speed)}
        </span>
        <span className="video-time">
          {formatPlaybackTime(time)} / {formatPlaybackTime(Number.isFinite(duration) ? duration : video?.duration_s)}
        </span>
        <input
          className="video-seek"
          type="range"
          min={0}
          max={Number.isFinite(duration) && duration > 0 ? duration : 0}
          step="any"
          value={Math.min(time, Number.isFinite(duration) ? duration : time)}
          onChange={(e) => seekTo(Number(e.target.value))}
          disabled={!src || !Number.isFinite(duration)}
          aria-label="再生の位置"
        />
        <button
          className={`btn ghost small${muted ? " active" : ""}`}
          onClick={blurAfter(toggleMute)}
          disabled={!src}
          title="消音（M）"
          aria-pressed={muted}
        >
          {muted ? "消音中" : "消音"}
        </button>
        <input
          className="video-volume"
          type="range"
          min={0}
          max={1}
          step={0.05}
          value={muted ? 0 : volume}
          onChange={(e) => changeVolume(Number(e.target.value))}
          disabled={!src}
          aria-label="音量"
        />
      </div>
      <div className="loupe-video-bar">
        <dl className="loupe-video-meta">
          <div>
            <dt>長さ</dt>
            <dd>{formatPlaybackTime(video?.duration_s)}</dd>
          </div>
          <div>
            <dt>フレームレート</dt>
            <dd>{formatFps(video?.fps)}</dd>
          </div>
          <div>
            <dt>コーデック</dt>
            <dd>{codecText}</dd>
          </div>
          <div>
            <dt>解像度</dt>
            <dd>{formatResolution(video?.width, video?.height)}</dd>
          </div>
          <div>
            <dt>再生</dt>
            <dd>
              {usingProxy && playback?.proxy
                ? `再生用のファイル（H.264・長辺 ${playback.proxy.long_edge}）`
                : decision?.kind === "original"
                  ? "元のファイル"
                  : "—"}
            </dd>
          </div>
        </dl>
        {playback && playback.video === null && (
          <span className="hint">
            {tools && !tools.ffprobe ? "ffprobe が見つからないため、動画の情報を読めません" : "動画の情報がありません"}
          </span>
        )}
        <span className="loupe-video-spacer" />
        {canSwitchToProxy && (
          <button
            className="btn"
            onClick={blurAfter(() => switchSource(true))}
            title="色・コマの表示がおかしいときなどに、ffmpeg で作った再生用のファイル（H.264）で再生します"
          >
            再生用のファイルで再生
          </button>
        )}
        {canSwitchToOriginal && (
          <button className="btn" onClick={blurAfter(() => switchSource(false))}>
            元のファイルで再生
          </button>
        )}
        <button
          className="btn"
          onClick={blurAfter(openInOs)}
          disabled={summary.file_status === "missing"}
          title={summary.file_status === "missing" ? "元ファイルが見つかりません" : undefined}
        >
          OS のアプリで再生
        </button>
      </div>
    </div>
  );
}

/** 再生できない・再生用のファイルを作っている間の知らせ（動画の上に重ねる）。 */
function VideoNotice(props: {
  playback: VideoPlayback | null;
  loadError: string | null;
  decision: ReturnType<typeof decideSource> | null;
  codecText: string;
  job: { done: number; total: number } | null;
  proxyError: string | null;
  proxyCancelled: boolean;
  playError: string | null;
  ffmpegMissing: boolean;
  onCancel: () => void;
  onRetry: () => void;
  onSettings: () => void;
  onOpenInOs: () => void;
}) {
  const { decision } = props;
  if (props.loadError) {
    return (
      <div className="loupe-center-note">
        <p>動画の情報を読めません</p>
        <p className="hint">{props.loadError}</p>
      </div>
    );
  }
  if (!props.playback || !decision) {
    return (
      <div className="loupe-center-note">
        <div className="spinner" />
      </div>
    );
  }
  if (props.playError) {
    return (
      <div className="loupe-center-note video-notice">
        <p>{props.playError}</p>
        <button className="btn small" onClick={props.onOpenInOs}>
          OS のアプリで再生
        </button>
      </div>
    );
  }
  switch (decision.kind) {
    case "original":
    case "proxy":
    case "missing":
      // 見つからない元ファイルは、ルーペの知らせ（FileNotice）が出す。
      return null;
    case "unplayable":
      return (
        <div className="loupe-center-note video-notice">
          <p>この形式（{props.codecText}）の動画は、この PC の WebView で再生できません。</p>
          <p className="hint">{decision.reason}</p>
          <div className="dlg-row">
            {props.ffmpegMissing && (
              <button className="btn small" onClick={props.onSettings}>
                設定で FFmpeg の場所を指定する
              </button>
            )}
            <button className="btn small" onClick={props.onOpenInOs}>
              OS のアプリで再生
            </button>
          </div>
        </div>
      );
    case "create_proxy": {
      const why =
        decision.reason === "chosen"
          ? "再生用のファイル（H.264・長辺 1280）を作っています。"
          : `この形式（${props.codecText}）の動画は、この PC の WebView で${
              decision.reason === "failed" ? "再生できなかった" : "再生できない"
            }ため、再生用のファイル（H.264・長辺 1280）を作って再生します。`;
      if (props.proxyError) {
        return (
          <div className="loupe-center-note video-notice">
            <p>再生用のファイルを作れませんでした</p>
            <p className="hint">{props.proxyError}</p>
            <div className="dlg-row">
              <button className="btn small" onClick={props.onRetry}>
                もう一度作る
              </button>
              <button className="btn small" onClick={props.onOpenInOs}>
                OS のアプリで再生
              </button>
            </div>
          </div>
        );
      }
      if (props.proxyCancelled) {
        return (
          <div className="loupe-center-note video-notice">
            <p>再生用のファイルの作成を取り消しました</p>
            <button className="btn small" onClick={props.onRetry}>
              再生用のファイルを作る
            </button>
          </div>
        );
      }
      return (
        <div className="loupe-center-note video-notice">
          <p>{why}</p>
          {props.job ? (
            <>
              <div className="video-proxy-progress">
                <Progress done={props.job.done} total={props.job.total} />
              </div>
              <button className="btn small" onClick={props.onCancel}>
                取り消す
              </button>
            </>
          ) : (
            <div className="spinner small" />
          )}
          <p className="hint">作ったファイルはキャッシュに残り、次からはすぐに再生できます（設定の「カタログ」で消せます）。</p>
        </div>
      );
    }
  }
}
