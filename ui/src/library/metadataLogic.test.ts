import { describe, expect, it } from "vitest";

import type { CaptureTime, FileInfo, VideoMetadata } from "../api/types";
import {
  captureNote,
  dirName,
  extensionMark,
  fileCheckNotice,
  fileStatusText,
  fileStatusTitle,
  formatDimensions,
  formatGps,
  formatIso,
  gpsDecimal,
  hasVideoInfo,
  primaryFile,
  videoRows,
  virtualCopyLabel,
} from "./metadataLogic";

const capture = (patch: Partial<CaptureTime>): CaptureTime => ({
  raw: "2024:05:01 10:00:00",
  offset: "+09:00",
  tz_source: "exif",
  tz_assumed: "+09:00",
  correction_s: 0,
  utc: "2024-05-01T01:00:00Z",
  ...patch,
});

const video = (patch: Partial<VideoMetadata> = {}): VideoMetadata => ({
  duration_s: null,
  fps: null,
  codec: null,
  bit_depth: null,
  color_transfer: null,
  color_primaries: null,
  width: null,
  height: null,
  creation_time: null,
  ...patch,
});

describe("寸法・パス・種類の印", () => {
  it("向き 5〜8 では幅と高さを入れ替える", () => {
    expect(formatDimensions(6000, 4000)).toBe("6,000 × 4,000");
    expect(formatDimensions(6000, 4000, 6)).toBe("4,000 × 6,000");
    expect(formatDimensions(null, 4000)).toBe("—");
  });

  it("パスのフォルダの部分", () => {
    expect(dirName("/a/b/c.ARW")).toBe("/a/b");
    expect(dirName("/c.ARW")).toBe("/");
    expect(dirName("C:\\DCIM\\a.ARW")).toBe("C:\\DCIM");
    expect(dirName("C:\\a.jpg")).toBe("C:\\");
    expect(dirName("a.jpg")).toBeNull();
    expect(dirName(null)).toBeNull();
  });

  it("拡張子を種類の印にする", () => {
    expect(extensionMark("DSC00001.arw")).toBe("ARW");
    expect(extensionMark("DJI_0001.MP4")).toBe("MP4");
    expect(extensionMark(".hidden")).toBe("");
    expect(extensionMark("noext")).toBe("");
    expect(extensionMark("a.")).toBe("");
  });
});

describe("撮影日時の補足", () => {
  it("記録されたオフセットなら補足なし", () => {
    expect(captureNote(capture({}))).toBeNull();
    expect(captureNote(capture({ utc: null }))).toBeNull();
    expect(captureNote(null)).toBeNull();
  });

  it("推定したタイムゾーンと補正を書く", () => {
    expect(captureNote(capture({ tz_source: "user_default", offset: null }))).toBe(
      "タイムゾーンは既定の値（+09:00）で推定",
    );
    expect(captureNote(capture({ tz_source: "utc_only", offset: "Z" }))).toBe(
      "UTC だけが記録されているため、既定のタイムゾーン（+09:00）で表示",
    );
    expect(captureNote(capture({ correction_s: -3600 }))).toBe("時刻を −3,600 秒補正");
    expect(captureNote(capture({ tz_source: "user_set", tz_assumed: "+02:00", correction_s: 5 }))).toBe(
      "タイムゾーンは設定した値（+02:00）、時刻を +5 秒補正",
    );
  });
});

describe("GPS・ISO・動画", () => {
  it("GPS の座標を 10 進の度で書く", () => {
    expect(formatGps({ lat: 35.681236, lon: 139.767125 })).toBe("北緯 35.681236°、東経 139.767125°");
    expect(formatGps({ lat: -33.8568, lon: -151.2153 })).toBe("南緯 33.856800°、西経 151.215300°");
    expect(formatGps(null)).toBeNull();
    expect(gpsDecimal({ lat: 35.681236, lon: 139.767125 })).toBe("35.681236, 139.767125");
  });

  it("ISO 感度", () => {
    expect(formatIso(12800)).toBe("ISO 12,800");
    expect(formatIso(null)).toBe("—");
  });

  it("動画の情報の行", () => {
    const rows = videoRows(
      video({ duration_s: 83.4, fps: 59.94006, codec: "hevc", bit_depth: 10, width: 3840, height: 2160 }),
    );
    expect(rows).toEqual([
      ["長さ", "1:23"],
      ["フレームレート", "59.94 fps"],
      ["コーデック", "hevc"],
      ["ビット深度", "10 bit"],
      ["解像度", "3,840 × 2,160"],
      ["色の伝達特性", "—"],
      ["色域", "—"],
    ]);
    expect(hasVideoInfo(video())).toBe(false);
    expect(hasVideoInfo(video({ codec: "h264" }))).toBe(true);
    expect(hasVideoInfo(null)).toBe(false);
  });
});

describe("ファイル", () => {
  const file = (patch: Partial<FileInfo>): FileInfo => ({
    file_id: 1,
    role: "primary",
    path: "/a/b.ARW",
    size: 100,
    status: "ok",
    status_reason: null,
    ...patch,
  });

  it("主のファイルを選ぶ", () => {
    const sidecar = file({ file_id: 2, role: "sidecar_jpeg" });
    const primary = file({ file_id: 3 });
    expect(primaryFile([sidecar, primary])).toBe(primary);
    expect(primaryFile([sidecar])).toBe(sidecar);
    expect(primaryFile([])).toBeNull();
  });

  it("状態に理由を添える", () => {
    expect(fileStatusText(file({}))).toBe("あり");
    expect(fileStatusText(file({ status: "missing", status_reason: "ドライブがない" }))).toBe(
      "見つからない（ドライブがない）",
    );
    expect(fileStatusText(file({ status: "error", status_reason: null }))).toBe("読めない");
    expect(fileStatusTitle("ok")).toBeNull();
    expect(fileStatusTitle("missing")).toBe("元ファイルが見つかりません");
  });

  it("仮想コピーの表示名", () => {
    expect(virtualCopyLabel({ is_master: true, variant_name: null })).toBeNull();
    expect(virtualCopyLabel({ is_master: false, variant_name: null })).toBe("仮想コピー");
    expect(virtualCopyLabel({ is_master: false, variant_name: "白黒" })).toBe("仮想コピー: 白黒");
  });
});

describe("元ファイルを確かめ直した結果の知らせ", () => {
  const ok = { status: "ok" as const, status_reason: null };

  it("すべて確かめられて状態が ok なら成功", () => {
    expect(fileCheckNotice({ checked: 2, ok: 2, missing: [] }, ok)).toEqual({
      level: "success",
      message: "元ファイルを確かめました",
    });
  });

  it("見つからないファイルがあれば、その場所を知らせる", () => {
    const n = fileCheckNotice(
      {
        checked: 2,
        ok: 0,
        missing: [
          { path: "/a/IMG_0001.ARW", reason: "ファイルが見つかりません" },
          { path: "/a/IMG_0001.JPG", reason: "ファイルが見つかりません" },
        ],
      },
      { status: "missing", status_reason: "ファイルが見つかりません" },
    );
    expect(n.level).toBe("warning");
    expect(n.message).toBe("元ファイルが見つかりません");
    expect(n.detail).toBe("/a/IMG_0001.ARW\nほか 1 件");
  });

  it("場所が分からない（ドライブが接続されていない）ファイルは、確かめられなかったと知らせる", () => {
    // check_files は path が None のファイルを checked にだけ数える。
    const n = fileCheckNotice({ checked: 1, ok: 0, missing: [] }, { status: "missing", status_reason: null });
    expect(n.level).toBe("warning");
    expect(n.message).toBe("元ファイルを確かめられませんでした");
  });

  it("読めない（権限がないなど）ファイルは ok に数えられても、状態が残っていれば成功にしない", () => {
    const missing = fileCheckNotice({ checked: 1, ok: 1, missing: [] }, { status: "missing", status_reason: "x" });
    expect(missing.level).toBe("warning");
    expect(missing.message).toBe("元ファイルを確かめられませんでした");
    const error = fileCheckNotice({ checked: 1, ok: 1, missing: [] }, { status: "error", status_reason: "壊れている" });
    expect(error).toEqual({ level: "warning", message: "元ファイルを読めません", detail: "壊れている" });
  });

  it("後の状態を読めなくても、結果だけで判断する", () => {
    expect(fileCheckNotice({ checked: 1, ok: 1, missing: [] }, null).level).toBe("success");
  });
});
