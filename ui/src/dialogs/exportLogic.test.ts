import { describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import * as core from "../api/core";
import { MOCK_FONT_DIRS, MockBridge, mockWatermarkCommands } from "../api/mock";
import type { ErrorInfo, ExportReport, JobInfo } from "../api/types";
import {
  DEFAULT_IMAGE_WATERMARK_SIZE,
  defaultExportForm,
  describeExportJob,
  describeExportPreset,
  describeExportReport,
  formFromPreset,
  formMatchesPreset,
  parseIntInput,
  presetFromForm,
  restoreExportForm,
  restoreExportPresetId,
  savedExport,
  switchWatermarkKind,
  toExportSettings,
  toFileNaming,
  validateExportForm,
  validateNaming,
  validateWatermark,
  videoNotice,
} from "./exportLogic";

const err = (kind: ErrorInfo["kind"], message: string): ErrorInfo => ({
  kind,
  message,
  user_actionable: true,
  retryable: false,
  hint: null,
});

describe("書き出しの入力", () => {
  it("既定は JPEG 品質 90・元の大きさ・GPS を削除・連番・キーワードを書く", () => {
    const f = { ...defaultExportForm("display_p3"), destDir: "/out" };
    expect(validateExportForm(f)).toEqual({});
    // genzo-model の export.rs の JSON の形と同じ。
    expect(JSON.stringify(toExportSettings(f))).toBe(
      '{"format":{"jpeg":{"quality":90}},"color_space":"display_p3","size":"original","remove_gps":true,"on_conflict":"sequence","remove_keywords":false,"resolution_ppi":72,"output_sharpening":null,"watermark":null,"videos":"copy"}',
    );
  });

  it("TIFF・PNG と長辺の指定", () => {
    const f = {
      ...defaultExportForm("srgb"),
      format: "png16" as const,
      sizeMode: "long_edge" as const,
      longEdge: "2048",
      removeGps: false,
      removeKeywords: true,
      onConflict: "skip" as const,
      destDir: "/out",
    };
    expect(JSON.stringify(toExportSettings(f))).toBe(
      '{"format":"png16","color_space":"srgb","size":{"long_edge":2048},"remove_gps":false,"on_conflict":"skip","remove_keywords":true,"resolution_ppi":72,"output_sharpening":null,"watermark":null,"videos":"copy"}',
    );
    expect(toExportSettings({ ...f, format: "tiff16" })?.format).toBe("tiff16");
    expect(toExportSettings({ ...f, format: "png8" })?.format).toBe("png8");
  });

  it("品質は 1〜100 の整数", () => {
    const base = { ...defaultExportForm("srgb"), destDir: "/out" };
    expect(validateExportForm({ ...base, quality: "1" })).toEqual({});
    expect(validateExportForm({ ...base, quality: "100" })).toEqual({});
    expect(validateExportForm({ ...base, quality: "0" }).quality).toBeDefined();
    expect(validateExportForm({ ...base, quality: "101" }).quality).toBeDefined();
    expect(validateExportForm({ ...base, quality: "9.5" }).quality).toBeDefined();
    expect(validateExportForm({ ...base, quality: "" }).quality).toBeDefined();
    expect(toExportSettings({ ...base, quality: "0" })).toBeNull();
    // JPEG 以外では品質を見ない。
    expect(validateExportForm({ ...base, format: "tiff16", quality: "0" })).toEqual({});
  });

  it("長辺は 1〜65535 の整数", () => {
    const base = { ...defaultExportForm("srgb"), destDir: "/out", sizeMode: "long_edge" as const };
    expect(validateExportForm({ ...base, longEdge: "1" })).toEqual({});
    expect(validateExportForm({ ...base, longEdge: "65535" })).toEqual({});
    expect(validateExportForm({ ...base, longEdge: "0" }).longEdge).toBeDefined();
    expect(validateExportForm({ ...base, longEdge: "65536" }).longEdge).toBeDefined();
    // 元の大きさのときは見ない。
    expect(validateExportForm({ ...base, sizeMode: "original", longEdge: "0" })).toEqual({});
  });

  it("書き出し先は必須", () => {
    expect(validateExportForm(defaultExportForm("srgb")).destDir).toBeDefined();
    expect(validateExportForm({ ...defaultExportForm("srgb"), destDir: "  " }).destDir).toBeDefined();
  });

  it("整数の入力は全角と桁区切りも読む", () => {
    expect(parseIntInput("２０４８")).toBe(2048);
    expect(parseIntInput("2,048")).toBe(2048);
    expect(parseIntInput(" 90 ")).toBe(90);
    expect(parseIntInput("-1")).toBeNull();
    expect(parseIntInput("1e3")).toBeNull();
  });
});

describe("前回の設定", () => {
  it("保存したものを戻す", () => {
    const f = { ...defaultExportForm("adobe_rgb"), format: "tiff16" as const, destDir: "/out" };
    const saved = JSON.parse(JSON.stringify(savedExport(f, "srgb")));
    expect(restoreExportForm(saved, "srgb")).toEqual(f);
  });

  it("既定の色空間を設定で変えていれば、新しい既定を使う", () => {
    const f = { ...defaultExportForm("adobe_rgb"), destDir: "/out" };
    const saved = savedExport(f, "srgb");
    expect(restoreExportForm(saved, "display_p3").colorSpace).toBe("display_p3");
    expect(restoreExportForm(saved, "display_p3").destDir).toBe("/out");
  });

  it("壊れた値は既定値", () => {
    expect(restoreExportForm(null, "srgb")).toEqual(defaultExportForm("srgb"));
    expect(restoreExportForm("x", "display_p3")).toEqual(defaultExportForm("display_p3"));
    const r = restoreExportForm(
      { v: 1, defaultColorSpace: "srgb", form: { format: "gif", removeGps: "yes", onConflict: "keep", quality: 80 } },
      "srgb",
    );
    expect(r.format).toBe("jpeg");
    expect(r.removeGps).toBe(true);
    // 保存した値にない項目（キーワードを書き出さない。v1 の organize-a で足した）は既定値。
    expect(r.removeKeywords).toBe(false);
    expect(r.onConflict).toBe("sequence");
    expect(r.quality).toBe("90");
  });
});

describe("ファイル名の規則と書き出しプリセット（EXP-02）", () => {
  const base = () => ({ ...defaultExportForm("srgb"), destDir: "/out" });

  it("ファイル名: 既定は元のファイル名。テンプレートを使うときだけ規則を渡す", () => {
    const f = base();
    expect(f.rename).toBe(false);
    expect(toFileNaming(f)).toBeNull();
    const named = { ...f, rename: true, template: "{date}-{seq}", sequenceStart: "７", sequenceDigits: 3 };
    expect(validateExportForm(named)).toEqual({});
    expect(toFileNaming(named)).toEqual({
      template: "{date}-{seq}",
      custom_text: "",
      sequence_start: 7,
      sequence_digits: 3,
    });
    expect(validateNaming({ ...named, template: " " })).toContain("テンプレート");
    expect(validateNaming({ ...named, customText: "a/b" })).toBe("任意の文字に「/」は使えません");
    expect(validateNaming({ ...named, sequenceStart: "-1" })).toContain("開始番号");
    expect(validateNaming({ ...named, sequenceDigits: 10 })).toBe("連番の桁数は 1〜9 です");
    expect(validateExportForm({ ...named, template: "" }).naming).toBeDefined();
    expect(toFileNaming({ ...named, template: "" })).toBeNull();
  });

  it("入力 → プリセット → 入力（書き出し先は選んだときだけ保存する）", () => {
    const f = {
      ...base(),
      format: "png16" as const,
      sizeMode: "long_edge" as const,
      longEdge: "1024",
      rename: true,
      template: "{name}-{text}",
      customText: "京都",
    };
    const p = presetFromForm(f, false);
    expect(p).toEqual({
      export: {
        format: "png16",
        color_space: "srgb",
        size: { long_edge: 1024 },
        remove_gps: true,
        on_conflict: "sequence",
        remove_keywords: false,
        resolution_ppi: 72,
        output_sharpening: null,
        watermark: null,
        videos: "copy",
      },
      destination: null,
      file_naming: { template: "{name}-{text}", custom_text: "京都", sequence_start: 1, sequence_digits: 4 },
    });
    expect(presetFromForm(f, true)?.destination).toBe("/out");
    expect(presetFromForm({ ...f, destDir: " " }, true)?.destination).toBeNull();
    expect(presetFromForm({ ...f, longEdge: "0" }, false)).toBeNull();
    // プリセットを選ぶと入力をその値にする（書き出し先がなければ今のまま）。
    const other = { ...defaultExportForm("display_p3"), destDir: "/今の場所" };
    const back = formFromPreset(p!, other);
    expect(back.format).toBe("png16");
    expect(back.longEdge).toBe("1024");
    expect(back.colorSpace).toBe("srgb");
    expect(back.destDir).toBe("/今の場所");
    expect(back.rename).toBe(true);
    expect(back.customText).toBe("京都");
    expect(formMatchesPreset(back, p!)).toBe(true);
    expect(formMatchesPreset({ ...back, quality: "50" }, p!)).toBe(true);
    expect(formMatchesPreset({ ...back, longEdge: "800" }, p!)).toBe(false);
    // JPEG の品質。名前の規則のないプリセットは元のファイル名にする。
    const jpeg = formFromPreset(
      {
        export: { ...p!.export, format: { jpeg: { quality: 72 } }, size: "original" },
        destination: "/web",
        file_naming: null,
      },
      back,
    );
    expect(jpeg.format).toBe("jpeg");
    expect(jpeg.quality).toBe("72");
    expect(jpeg.sizeMode).toBe("original");
    expect(jpeg.destDir).toBe("/web");
    expect(jpeg.rename).toBe(false);
    expect(describeExportPreset(p!)).toBe("PNG（16bit）・sRGB・長辺 1,024 px・GPS を削除・名前 {name}-{text}・書き出し先は選ぶ");
  });

  it("更新では、画面が知らない項目（新しい版のアプリが足した項目など）を残す", () => {
    const f = base();
    const known = presetFromForm(f, false)!;
    const future = {
      ...known,
      export: { ...known.export, future_option: { amount: "standard" } },
      future_top: { text: "©" },
    } as unknown as Parameters<typeof presetFromForm>[2];
    const updated = presetFromForm({ ...f, quality: "70" }, false, future) as unknown as Record<string, unknown>;
    expect(updated.future_top).toEqual({ text: "©" });
    expect((updated.export as Record<string, unknown>).future_option).toEqual({ amount: "standard" });
    expect((updated.export as Record<string, unknown>).format).toEqual({ jpeg: { quality: 70 } });
  });

  it("前回の設定: ファイル名の規則と選んだプリセットを戻す（前の版の保存には無いので既定）", () => {
    const f = { ...base(), rename: true, template: "{seq}", sequenceDigits: 2 };
    const saved = JSON.parse(JSON.stringify(savedExport(f, "srgb", "00000000000000e1")));
    expect(restoreExportForm(saved, "srgb")).toEqual(f);
    expect(restoreExportPresetId(saved)).toBe("00000000000000e1");
    const old = { v: 1, defaultColorSpace: "srgb", form: { format: "png8", destDir: "/o" } };
    const r = restoreExportForm(old, "srgb");
    expect(r.rename).toBe(false);
    expect(r.template).toBe("{name}");
    expect(r.sequenceDigits).toBe(4);
    expect(restoreExportPresetId(old)).toBeNull();
    expect(restoreExportPresetId(null)).toBeNull();
  });
});

describe("出力シャープ・透かし・解像度・動画（EXP-03・EXP-05）", () => {
  const base = () => ({ ...defaultExportForm("srgb"), destDir: "/out" });
  const textMark = () => ({
    ...base(),
    watermark: true,
    watermarkText: "© 2026 山田",
    watermarkFont: "/fonts/a.ttc",
    watermarkFontIndex: "1",
    watermarkColor: "#ff0000",
  });

  it("出力シャープと解像度を ExportSettings にする", () => {
    const s = toExportSettings({ ...base(), resolution: "300", sharpen: true, sharpenMedia: "matte_paper", sharpenAmount: "high" });
    expect(s?.resolution_ppi).toBe(300);
    expect(s?.output_sharpening).toEqual({ media: "matte_paper", amount: "high" });
    expect(toExportSettings({ ...base(), sharpen: false })?.output_sharpening).toBeNull();
    for (const bad of ["0", "1201", "7.5", ""]) {
      expect(validateExportForm({ ...base(), resolution: bad }).resolution).toBeDefined();
      expect(toExportSettings({ ...base(), resolution: bad })).toBeNull();
    }
  });

  it("文字の透かしは genzo-model の Watermark の JSON の形にする", () => {
    const s = toExportSettings(textMark());
    expect(JSON.stringify(s?.watermark)).toBe(
      '{"content":{"text":{"text":"© 2026 山田","font":"/fonts/a.ttc","font_index":1,"color":"#FF0000"}},"anchor":"bottom_right","size":4,"opacity":60,"margin":2}',
    );
    // 付けない・画像。
    expect(toExportSettings({ ...textMark(), watermark: false })?.watermark).toBeNull();
    const img = toExportSettings({ ...base(), watermark: true, watermarkKind: "image", watermarkImage: "/logo.png", watermarkSize: "20" });
    expect(img?.watermark?.content).toEqual({ image: { path: "/logo.png" } });
  });

  it("透かしの入力を確かめる（コアの Watermark::problem と同じ範囲）", () => {
    expect(validateWatermark(textMark())).toBeNull();
    const cases: [Partial<ReturnType<typeof textMark>>, string][] = [
      [{ watermarkText: " " }, "文字を入れて"],
      [{ watermarkText: "あ".repeat(201) }, "200 文字まで"],
      [{ watermarkText: "1\n2\n3\n4\n5\n6" }, "5 行まで"],
      [{ watermarkText: "a\tb" }, "制御文字"],
      [{ watermarkFont: "" }, "フォント"],
      [{ watermarkFontIndex: "256" }, "番号"],
      [{ watermarkColor: "red" }, "#RRGGBB"],
      [{ watermarkSize: "0" }, "大きさ"],
      [{ watermarkOpacity: "101" }, "不透明度"],
      [{ watermarkMargin: "26" }, "余白"],
      [{ watermarkKind: "image", watermarkImage: "" }, "画像"],
    ];
    for (const [patch, needle] of cases) {
      const f = { ...textMark(), ...patch };
      expect(validateWatermark(f)).toContain(needle);
      expect(validateExportForm(f).watermark).toBeDefined();
      expect(toExportSettings(f)).toBeNull();
    }
    // 最後の改行は行に数えない（コアの str::lines と同じ）。
    expect(validateWatermark({ ...textMark(), watermarkText: "1\n2\n3\n4\n5\n" })).toBeNull();
    // 付けないときは見ない。
    expect(validateWatermark({ ...textMark(), watermark: false, watermarkText: "" })).toBeNull();
  });

  it("透かしの種類を替えると、既定のままの大きさは新しい種類の既定にする", () => {
    const f = textMark();
    expect(switchWatermarkKind(f, "image").watermarkSize).toBe(String(DEFAULT_IMAGE_WATERMARK_SIZE));
    expect(switchWatermarkKind(switchWatermarkKind(f, "image"), "text").watermarkSize).toBe("4");
    expect(switchWatermarkKind({ ...f, watermarkSize: "7" }, "image").watermarkSize).toBe("7");
  });

  it("書き出しプリセットに保存して戻す（省略された項目は既定値として比べる）", () => {
    const f = { ...textMark(), sharpen: true, sharpenMedia: "glossy_paper" as const, resolution: "240", videos: false };
    const p = presetFromForm(f, false)!;
    expect(p.export.videos).toBe("skip");
    const back = formFromPreset(p, base());
    expect(back.watermark).toBe(true);
    expect(back.watermarkText).toBe("© 2026 山田");
    expect(back.watermarkFontIndex).toBe("1");
    expect(back.sharpenMedia).toBe("glossy_paper");
    expect(back.resolution).toBe("240");
    expect(back.videos).toBe(false);
    expect(formMatchesPreset(back, p)).toBe(true);
    expect(formMatchesPreset({ ...back, watermarkOpacity: "30" }, p)).toBe(false);
    // 古いプリセット（項目がない）は、出力シャープなし・透かしなし・72 ppi・動画をコピー。
    const old = { export: { format: { jpeg: { quality: 80 } }, color_space: "srgb", size: "original", remove_gps: false, on_conflict: "sequence" }, destination: null, file_naming: null } as const;
    const restored = formFromPreset(old as unknown as Parameters<typeof formFromPreset>[0], f);
    expect(restored.sharpen).toBe(false);
    expect(restored.watermark).toBe(false);
    expect(restored.resolution).toBe("72");
    expect(restored.videos).toBe(true);
    expect(formMatchesPreset(restored, old as unknown as Parameters<typeof formMatchesPreset>[1])).toBe(true);
    expect(describeExportPreset(p)).toContain("透かし「© 2026 山田」");
    expect(describeExportPreset(p)).toContain("出力シャープ 光沢紙・標準");
    expect(describeExportPreset(p)).toContain("動画は書き出さない");
  });

  it("別の OS で作ったプリセット（コアが書き出し先・フォントを外したもの）: 書き出し先は今のまま、フォントは選び直す", () => {
    const f = textMark();
    const p = presetFromForm(f, false)!;
    const mark = p.export.watermark!;
    const foreign = {
      ...p,
      destination: null,
      export: { ...p.export, watermark: { ...mark, content: { text: { ..."text" in mark.content ? mark.content.text : { text: "" }, font: "" } } } },
    } as unknown as Parameters<typeof formFromPreset>[0];
    const back = formFromPreset(foreign, { ...f, destDir: "/here" });
    expect(back.destDir).toBe("/here");
    expect(back.watermark).toBe(true);
    expect(back.watermarkFont).toBe("");
    expect(validateWatermark(back)).toBe("透かしの文字のフォントのファイルを選んでください");
  });

  it("前回の設定に戻す（前の版の保存には無いので既定）", () => {
    const f = { ...textMark(), sharpen: true, watermarkAnchor: "top_left" as const };
    expect(restoreExportForm(JSON.parse(JSON.stringify(savedExport(f, "srgb"))), "srgb")).toEqual(f);
    const old = restoreExportForm({ v: 1, defaultColorSpace: "srgb", form: { format: "png8", watermarkAnchor: "nowhere" } }, "srgb");
    expect(old.watermark).toBe(false);
    expect(old.watermarkAnchor).toBe("bottom_right");
    expect(old.videos).toBe(true);
  });

  it("動画の注意書き", () => {
    expect(videoNotice({ videos: true, removeGps: false }, 0)).toBeNull();
    expect(videoNotice({ videos: true, removeGps: false }, 2)?.text).toContain("動画 2 本は再エンコードせずに元のファイルのままコピー");
    const gps = videoNotice({ videos: true, removeGps: true }, 1);
    expect(gps?.level).toBe("warn");
    expect(gps?.text).toContain("位置情報");
    expect(videoNotice({ videos: false, removeGps: false }, 3)?.text).toContain("動画 3 本は書き出しません");
  });

  it("透かしの見本・フォントのフォルダのコマンドの形（api/core.ts と mock.ts）", async () => {
    const mock = new MockBridge();
    installBridge(mock);
    const { sent } = mockWatermarkCommands(mock);
    const mark = toExportSettings({ ...textMark(), watermarkText: "漢 A" })!.watermark!;
    const p = await core.watermarkPreview(mark, 300, 200);
    expect([p.width, p.height, p.mark_width]).toEqual([300, 200, 120]);
    expect(p.warnings).toEqual(["フォントにない文字があります: 漢"]);
    expect(p).not.toHaveProperty("type");
    expect(sent[0]).toEqual({ type: "watermark_preview", watermark: mark, width: 300, height: 200 });
    expect(await core.systemFontDirs()).toEqual(MOCK_FONT_DIRS);
    expect(sent[1]).toEqual({ type: "system_font_dirs" });
  });
});

describe("書き出しの結果", () => {
  const report = (over: Partial<ExportReport>): ExportReport => ({
    dest_dir: "/out",
    items: [],
    written: 0,
    skipped: 0,
    failed: 0,
    cancelled: false,
    ...over,
  });

  it("動画のコピーの数と警告", () => {
    const n = describeExportReport(
      report({
        written: 1,
        copied: 2,
        items: [
          { variant_id: 1, outcome: { status: "copied", path: "/out/V.mp4", bytes: 10, full_hash: "ab", warnings: ["動画は上書きしません（同じ名前のファイルがあるため、連番を付けました）"] } },
        ],
      }),
    );
    expect(n.level).toBe("success");
    expect(n.message).toBe("書き出し: 1 枚を書き出しました、動画 2 本をコピーしました");
    expect(n.detail).toContain("動画は上書きしません");
    expect(describeExportReport(report({ copied: 1 })).message).toBe("書き出し: 動画 1 本をコピーしました");
  });

  it("すべて書けたら成功", () => {
    const n = describeExportReport(report({ written: 3 }));
    expect(n.level).toBe("success");
    expect(n.message).toBe("書き出し: 3 枚を書き出しました");
    expect(n.detail).toContain("書き出し先: /out");
  });

  it("原本の保護・飛ばしの理由をまとめる", () => {
    const n = describeExportReport(
      report({
        written: 1,
        skipped: 2,
        failed: 2,
        items: [
          { variant_id: 1, outcome: { status: "failed", error: err("protected_destination", "原本です") } },
          { variant_id: 2, outcome: { status: "failed", error: err("protected_destination", "原本です") } },
          { variant_id: 3, outcome: { status: "skipped", reason: "同じ名前のファイルがあるため、書き出しませんでした", existing: "/out/a.jpg" } },
          { variant_id: 4, outcome: { status: "skipped", reason: "動画を書き出さない設定です", existing: null } },
        ],
      }),
    );
    expect(n.level).toBe("warning");
    expect(n.message).toBe("書き出し: 1 枚を書き出しました、2 枚は飛ばしました、2 枚は失敗しました");
    expect(n.detail).toContain("・原本と同じ場所・同じ名前（2 枚）");
    expect(n.detail).toContain("例: 原本と同じ場所・同じ名前になるため書き出しませんでした（原本は変更していません）");
    expect(n.detail).toContain("同じ名前のファイルがあるため、書き出しませんでした（1 枚）");
    expect(n.detail).toContain("連番");
  });

  it("失敗はエラーの種類ごとにまとめる（パスの違う文を別の理由にしない）", () => {
    const items: ExportReport["items"] = Array.from({ length: 300 }, (_, i) => ({
      variant_id: i + 1,
      outcome: {
        status: "failed" as const,
        error: {
          ...err("file_access", `ファイルにアクセスできません（/m/ext/IMG_${i}.ARW）: 見つかりません`),
          hint: "ドライブの接続・権限を確かめてください",
        },
      },
    }));
    items.push({ variant_id: 999, outcome: { status: "failed", error: err("storage", "空き容量が足りません") } });
    const n = describeExportReport(report({ failed: 301, items }));
    expect(n.detail?.split("\n")).toEqual([
      "失敗:",
      "・ファイルにアクセスできない（300 枚）",
      "　例: ファイルにアクセスできません（/m/ext/IMG_0.ARW）: 見つかりません",
      "　例: ファイルにアクセスできません（/m/ext/IMG_1.ARW）: 見つかりません",
      "　→ ドライブの接続・権限を確かめてください",
      "・空き容量・書き込みの問題（1 枚）",
      "　例: 空き容量が足りません",
      "書き出し先: /out",
    ]);
  });

  it("1 枚も書けなければエラー、取り消しは警告", () => {
    expect(
      describeExportReport(
        report({ failed: 1, items: [{ variant_id: 1, outcome: { status: "failed", error: err("storage", "容量不足") } }] }),
      ).level,
    ).toBe("error");
    const c = describeExportReport(report({ written: 2, cancelled: true }));
    expect(c.level).toBe("warning");
    expect(c.message).toBe("書き出しを取り消しました: 2 枚を書き出しました");
  });

  it("ジョブの失敗は二重に知らせない", () => {
    const info = (over: Partial<JobInfo>): JobInfo => ({
      job_id: 1,
      kind: "export",
      state: "succeeded",
      done: 1,
      total: 1,
      result: null,
      error: null,
      ...over,
    });
    expect(describeExportJob(info({ state: "failed" }))).toBeNull();
    expect(describeExportJob(info({ state: "cancelled" }))?.message).toBe("書き出しを取り消しました");
    expect(describeExportJob(info({ result: { type: "export", ...report({ written: 1 }) } }))?.level).toBe("success");
  });
});
