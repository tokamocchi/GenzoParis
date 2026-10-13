// 動画の再生（VID-04）の計算（videoLogic.ts）、コアのコマンドの形（api/core.ts と mock.ts）、画面が代表的な状態で
// 例外なく描けること（react-dom/server。useEffect は動かない）、ショートカットの一覧のテスト。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import * as core from "../api/core";
import { MockBridge, mockSummary, mockVideoCommands, mockVideoPlayback } from "../api/mock";
import { setWindowsStyleForTest, videoProxyUrl, videoUrl } from "../api/uri";
import { SHORTCUT_GROUPS } from "../dialogs/shortcutsData";
import { normalizeCombo } from "../keyboard";
import {
  DEFAULT_FPS,
  SHUTTLE_MAX,
  canPlayQuery,
  codecsFor,
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
import { VideoPlayer } from "./VideoPlayer";

describe("J・K・L（shuttle）", () => {
  it("L は前へ 1・2・4・8 倍、J は後ろへ −1・−2・−4・−8 倍、K で止める", () => {
    let s = 0;
    const seq: number[] = [];
    for (let i = 0; i < 5; i++) {
      s = shuttle(s, "l");
      seq.push(s);
    }
    expect(seq).toEqual([1, 2, 4, 8, 8]);
    expect(SHUTTLE_MAX).toBe(8);
    expect(shuttle(8, "k")).toBe(0);
    // 逆の向きに押すと、その向きの 1 倍から。
    expect(shuttle(4, "j")).toBe(-1);
    expect(shuttle(-1, "j")).toBe(-2);
    expect(shuttle(-8, "j")).toBe(-8);
    expect(shuttle(-2, "l")).toBe(1);
    expect(shuttle(0, "j")).toBe(-1);
  });

  it("速さの表示", () => {
    expect(shuttleLabel(0)).toBe("");
    expect(shuttleLabel(1)).toBe("1×");
    expect(shuttleLabel(-4)).toBe("−4×");
  });

  it("後ろへの再生は、速さ × 時間だけ戻す（0 で止まる）", () => {
    expect(reverseStep(10, -2, 100)).toBeCloseTo(9.8, 9);
    expect(reverseStep(0.05, -8, 100)).toBe(0);
    expect(reverseStep(5, -1, -50)).toBe(5);
  });
});

describe("コマ送り", () => {
  it("1 フレームぶん進む・戻る（フレームの始まりから 1/4 フレーム後ろ）", () => {
    expect(stepFrame(0, 25, 1, 10)).toBeCloseTo(1.25 / 25, 9);
    expect(stepFrame(1.25 / 25, 25, 1, 10)).toBeCloseTo(2.25 / 25, 9);
    expect(stepFrame(2.25 / 25, 25, -1, 10)).toBeCloseTo(1.25 / 25, 9);
    // 先頭より前へは戻らない。
    expect(stepFrame(0, 25, -1, 10)).toBeCloseTo(0.25 / 25, 9);
    // 最後より後ろへは進まない。
    expect(stepFrame(9.99, 25, 1, 10)).toBe(10);
    // 長さが分からなくても進める。
    expect(stepFrame(1, 30, 1, Number.NaN)).toBeCloseTo(31.25 / 30, 9);
  });

  it("フレームレートが分からなければ 30 fps とみなす", () => {
    expect(effectiveFps(null)).toBe(DEFAULT_FPS);
    expect(effectiveFps(0)).toBe(DEFAULT_FPS);
    expect(effectiveFps(Number.NaN)).toBe(DEFAULT_FPS);
    expect(effectiveFps(59.94)).toBe(59.94);
    expect(stepFrame(0, null, 1, 10)).toBeCloseTo(1.25 / 30, 9);
  });
});

describe("シーク（seekTarget）", () => {
  it("長さの中に収める（長さが分からなければ上限なし）", () => {
    expect(seekTarget(5, 10)).toBe(5);
    expect(seekTarget(12, 10)).toBe(10);
    expect(seekTarget(-1, 10)).toBe(0);
    expect(seekTarget(Number.POSITIVE_INFINITY, 10)).toBe(10);
    expect(seekTarget(3, Number.NaN)).toBe(3);
    expect(seekTarget(3, Number.POSITIVE_INFINITY)).toBe(3);
  });

  it("再現（検証 R1）: 読み込む前（長さが NaN）の End などで、有限でない位置へは移らない（null）", () => {
    // video 要素の currentTime に NaN・Infinity を入れると TypeError になる。
    expect(seekTarget(Number.NaN, Number.NaN)).toBeNull();
    expect(seekTarget(Number.NaN, 10)).toBeNull();
    expect(seekTarget(Number.POSITIVE_INFINITY, Number.NaN)).toBeNull();
    expect(seekTarget(Number.POSITIVE_INFINITY, Number.POSITIVE_INFINITY)).toBeNull();
  });
});

describe("表示", () => {
  it("再生の位置", () => {
    expect(formatPlaybackTime(0)).toBe("0:00");
    expect(formatPlaybackTime(75.9)).toBe("1:15");
    expect(formatPlaybackTime(3723)).toBe("1:02:03");
    expect(formatPlaybackTime(null)).toBe("--:--");
    expect(formatPlaybackTime(Number.NaN)).toBe("--:--");
    expect(formatPlaybackTime(Number.POSITIVE_INFINITY)).toBe("--:--");
  });

  it("再生の失敗の理由", () => {
    expect(mediaErrorMessage(4)).toContain("再生できません");
    expect(mediaErrorMessage(3)).toContain("デコード");
    expect(mediaErrorMessage(null)).toBe("動画を再生できません");
  });

  it("音量の保存の読み込み（壊れた値は既定）", () => {
    expect(parseVolume("0.4")).toBe(0.4);
    expect(parseVolume("3")).toBe(1);
    expect(parseVolume("-1")).toBe(0);
    expect(parseVolume("x")).toBe(1);
    expect(parseVolume(null)).toBe(1);
    expect(parseVolume("")).toBe(1);
    expect(parseMuted("true")).toBe(true);
    expect(parseMuted("1")).toBe(false);
    expect(parseMuted(null)).toBe(false);
  });
});

describe("再生できるかの問い合わせ（canPlayType）", () => {
  it("コーデックとビット深度から codecs を作る（MP4・MOV は video/mp4 で問う）", () => {
    expect(canPlayQuery("hevc", 10)).toBe('video/mp4; codecs="hvc1.2.4.L153.B0"');
    expect(canPlayQuery("hevc", 8)).toBe('video/mp4; codecs="hvc1.1.6.L153.B0"');
    expect(canPlayQuery("h264", null)).toBe('video/mp4; codecs="avc1.640033"');
    expect(canPlayQuery("H264", 10)).toBe('video/mp4; codecs="avc1.6e0033"');
    expect(codecsFor("mpeg4", 8)).toBe("mp4v.20.9");
    expect(codecsFor("prores", 10)).toBe(null);
    expect(canPlayQuery("prores", 10)).toBe("video/mp4");
    expect(canPlayQuery(null, null)).toBe("video/mp4");
  });

  it("結果の読み方", () => {
    expect(playabilityOf("probably")).toBe("probably");
    expect(playabilityOf("maybe")).toBe("maybe");
    expect(playabilityOf("")).toBe("no");
    expect(playabilityOf(undefined)).toBe("no");
  });
});

describe("どのファイルで再生するか", () => {
  const proxy = { size: 1000, long_edge: 1280 };

  it("再生できそうなら元のファイル", () => {
    expect(decideSource(mockVideoPlayback(), "probably", false, false)).toEqual({ kind: "original" });
    expect(decideSource(mockVideoPlayback(), "maybe", false, false)).toEqual({ kind: "original" });
  });

  it("再生できない形式・再生の失敗: 再生用のファイルがあれば使い、なければ作る", () => {
    expect(decideSource(mockVideoPlayback(), "no", false, false)).toEqual({
      kind: "create_proxy",
      reason: "unsupported",
    });
    expect(decideSource(mockVideoPlayback(), "maybe", true, false)).toEqual({ kind: "create_proxy", reason: "failed" });
    expect(decideSource(mockVideoPlayback({ proxy }), "no", false, false)).toEqual({ kind: "proxy" });
    expect(decideSource(mockVideoPlayback({ proxy }), "maybe", true, false)).toEqual({ kind: "proxy" });
  });

  it("ffmpeg がなければ、作れない理由を出す", () => {
    const p = mockVideoPlayback({ proxy_unavailable: "ffmpeg・ffprobe が見つからない", proxy_encoder: null });
    expect(decideSource(p, "no", false, false)).toEqual({ kind: "unplayable", reason: "ffmpeg・ffprobe が見つからない" });
    // 再生できそうなら、ffmpeg がなくても元のファイルで再生する。
    expect(decideSource(p, "maybe", false, false)).toEqual({ kind: "original" });
  });

  it("利用者が再生用のファイルを選んだら、それを使う（なければ作る）", () => {
    expect(decideSource(mockVideoPlayback(), "probably", false, true)).toEqual({
      kind: "create_proxy",
      reason: "chosen",
    });
    expect(decideSource(mockVideoPlayback({ proxy }), "probably", false, true)).toEqual({ kind: "proxy" });
  });

  it("元のファイルが見つからなくても、再生用のファイルがあれば再生できる", () => {
    const missing = mockVideoPlayback({ available: false, unavailable_reason: "元の動画が見つかりません" });
    expect(decideSource(missing, "maybe", false, false)).toEqual({
      kind: "missing",
      reason: "元の動画が見つかりません",
    });
    expect(decideSource({ ...missing, proxy }, "maybe", false, false)).toEqual({ kind: "proxy" });
    // 見つからない動画の再生用のファイルは作らない。
    expect(decideSource(missing, "no", false, true).kind).toBe("missing");
  });
});

describe("コアのコマンドと URI", () => {
  let mock: MockBridge;
  let sent: Record<string, unknown>[];
  beforeEach(() => {
    mock = new MockBridge();
    installBridge(mock);
    sent = [];
    mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
      sent.push(args?.command as Record<string, unknown>);
      return { type: "ok" };
    });
  });

  it("再生の情報・再生用のファイルの作成・キャッシュの状況と消去", async () => {
    const { jobs } = mockVideoCommands(mock);
    const p = await core.videoPlayback(7);
    expect(p.variant_id).toBe(7);
    expect(p.content_type).toBe("video/quicktime");
    const job = await core.createVideoProxy(7);
    expect(jobs).toEqual([{ variant_id: 7, job_id: job }]);
    expect((await core.videoProxyCache()).count).toBe(3);
    const cleared = await core.clearVideoProxies();
    expect([cleared.removed, cleared.count]).toEqual([3, 0]);
    await core.cancelJob(job);
    expect(sent).toEqual([{ type: "cancel_job", job_id: job }]);
  });

  it("動画の URI（Windows は http://genzo.localhost）", () => {
    setWindowsStyleForTest(false);
    expect(videoUrl(7)).toBe("genzo://localhost/video/7");
    expect(videoProxyUrl(7, 1234)).toBe("genzo://localhost/video-proxy/7?rev=1234");
    expect(videoProxyUrl(7)).toBe("genzo://localhost/video-proxy/7");
    setWindowsStyleForTest(true);
    expect(videoUrl(7)).toBe("http://genzo.localhost/video/7");
    setWindowsStyleForTest(null);
  });

  it("動画のルーペを描ける（再生の情報を読む前）", () => {
    mockVideoCommands(mock);
    const html = renderToString(
      createElement(VideoPlayer, {
        summary: mockSummary({ variant_id: 7, kind: "video", file_name: "DJI_0001.MOV" }),
        details: null,
        thumbRev: "ab",
        hotkeysEnabled: true,
      }),
    );
    expect(html).toContain("OS のアプリで再生");
    expect(html).toContain("1 コマ進む");
    expect(html).toContain("--:--");
  });
});

describe("ショートカットの一覧", () => {
  it("動画のキーが載っていて、ほかの区分の全体のキーと重ならない", () => {
    const video = SHORTCUT_GROUPS.find((g) => g.title === "動画（ルーペ）");
    expect(video).toBeDefined();
    const keys = video!.items.flatMap((i) => i.keys.map(normalizeCombo));
    for (const k of ["Space", "j", "k", "l", ",", ".", "m", "Home", "End"]) expect(keys).toContain(k);
    // 表示・選別の区分（ルーペで効く全体のキー）とは重ならない。
    const global = SHORTCUT_GROUPS.filter((g) => g.title === "表示" || g.title === "選別").flatMap((g) =>
      g.items.flatMap((i) => i.keys.map(normalizeCombo)),
    );
    for (const k of keys) expect(global).not.toContain(k);
  });
});
