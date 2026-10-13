// レンズ補正（DEV-14）の画面: 操作の可否（RAW・処理バージョン・埋め込みの情報）、項目のそろえ方、処理バージョンの
// 更新（DATA-09）の送信と、パネルが代表的な状態で例外なく描けること（react-dom/server で文字列にする）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { MockBridge, mockDevelopSettings, mockDevelopState, mockSourceInfo } from "../api/mock";
import type { DevelopState } from "../api/types";
import { appStore } from "../state/app";
import { developStore, openDevelopFor, upgradeProcessVersion } from "../state/develop";
import { DevelopPanel } from "./DevelopPanel";
import { formatPercent, parseNumber } from "./scales";
import {
  canUpgradeProcess,
  defaultLens,
  fullLens,
  LENS_PROCESS_VERSION,
  lensControls,
  lensInfoNote,
} from "./settingsLogic";

describe("レンズ補正の操作の可否", () => {
  const v2 = mockDevelopSettings();
  const raw = mockSourceInfo();

  it("RAW で埋め込みの情報があれば、プロファイルの補正を有効にしたときだけ適用量を動かせる", () => {
    const off = lensControls(v2, raw, false);
    expect(off).toEqual({
      needsUpgrade: false,
      profile: true,
      distortionScale: false,
      vignettingScale: false,
      removeCa: true,
      manualDistortion: true,
      manualSensor: true,
    });
    const on = lensControls({ ...v2, lens: { ...v2.lens, profile_enabled: true } }, raw, false);
    expect(on.distortionScale).toBe(true);
    expect(on.vignettingScale).toBe(true);
  });

  it("埋め込みの情報にない項目の適用量は動かせない", () => {
    const s = { ...v2, lens: { ...v2.lens, profile_enabled: true } };
    const noInfo = lensControls(s, mockSourceInfo({ lens_correction: null }), false);
    expect(noInfo.profile).toBe(true);
    expect(noInfo.distortionScale).toBe(false);
    expect(noInfo.vignettingScale).toBe(false);
    const onlyDistortion = lensControls(
      s,
      mockSourceInfo({
        lens_correction: {
          source: "dng_opcode",
          description: "DNG の opcode（歪曲）",
          distortion: true,
          vignetting: false,
          chromatic_aberration: false,
        },
      }),
      false,
    );
    expect(onlyDistortion.distortionScale).toBe(true);
    expect(onlyDistortion.vignettingScale).toBe(false);
    // 古いコアの応答（lens_correction が無い）でも、情報なしとして扱う。
    const { lens_correction: _, ...old } = raw;
    expect(lensControls(s, old, false).distortionScale).toBe(false);
  });

  it("埋め込みの情報の説明: ある・壊れていて使えない・ない・RAW 以外を区別する", () => {
    expect(lensInfoNote(raw)).toBe("補正の情報: Sony の ARW（歪曲・周辺減光・色収差）");
    const broken = mockSourceInfo({
      lens_correction: null,
      lens_correction_note: "レンズ補正の情報が不正です: WarpRectilinear の係数が不正です",
    });
    expect(lensInfoNote(broken)).toBe(
      "埋め込みの補正の情報が壊れているため使えません（レンズ補正の情報が不正です: WarpRectilinear の係数が不正です）。手動で補正できます。",
    );
    expect(lensInfoNote(mockSourceInfo({ lens_correction: null }))).toBe(
      "この写真には埋め込みの補正の情報がありません（手動で補正できます）。",
    );
    // 古いコア（lens_correction_note が無い）でも、情報なしとして扱う。
    const { lens_correction_note: _n, ...old } = mockSourceInfo({ lens_correction: null });
    expect(lensInfoNote(old)).toContain("情報がありません");
    expect(lensInfoNote(mockSourceInfo({ is_raw: false, lens_correction: null }))).toBeNull();
    expect(lensInfoNote(null)).toBeNull();
  });

  it("RAW 以外は歪曲の手動の補正だけ", () => {
    const c = lensControls(v2, mockSourceInfo({ is_raw: false, lens_correction: null }), false);
    expect(c).toEqual({
      needsUpgrade: false,
      profile: false,
      distortionScale: false,
      vignettingScale: false,
      removeCa: false,
      manualDistortion: true,
      manualSensor: false,
    });
  });

  it("処理バージョン 1 の写真・開いていないときは何も動かせない", () => {
    const v1 = lensControls({ ...v2, process_version: 1 }, raw, false);
    expect(v1.needsUpgrade).toBe(true);
    expect(Object.entries(v1).filter(([k, v]) => k !== "needsUpgrade" && v)).toEqual([]);
    const closed = lensControls(v2, null, false);
    expect(closed.manualDistortion).toBe(false);
    const disabled = lensControls(v2, raw, true);
    expect(disabled.profile).toBe(false);
    expect(disabled.manualDistortion).toBe(false);
  });

  it("処理バージョンの更新は、コアが知らせる最新の版より古いときだけ", () => {
    expect(LENS_PROCESS_VERSION).toBe(2);
    expect(canUpgradeProcess(1, 2)).toBe(true);
    expect(canUpgradeProcess(2, 2)).toBe(false);
    expect(canUpgradeProcess(1, undefined)).toBe(false);
  });

  it("欠けた項目は既定値でそろえる（コアの serde の既定と同じ）", () => {
    expect(defaultLens()).toEqual(mockDevelopSettings().lens);
    expect(fullLens({ profile_enabled: true, distortion_scale: 50 })).toEqual({
      ...defaultLens(),
      profile_enabled: true,
      distortion_scale: 50,
    });
    expect(fullLens(undefined)).toEqual(defaultLens());
  });

  it("適用量の書式", () => {
    expect(formatPercent(100)).toBe("100%");
    expect(formatPercent(149.6)).toBe("150%");
    expect(parseNumber("120%")).toBe(120);
  });
});

describe("処理バージョンの更新（DATA-09）", () => {
  let mock: MockBridge;
  let fail: boolean;
  let opened: number;
  beforeEach(() => {
    fail = false;
    opened = 1;
    mock = new MockBridge();
    installBridge(mock);
    mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
      const c = args?.command as { type: string; variant_id?: number };
      switch (c.type) {
        case "open_develop":
          opened = c.variant_id ?? 1;
          return {
            type: "develop",
            ...mockDevelopState({ variant_id: opened, settings: mockDevelopSettings({ process_version: 1 }) }),
          };
        case "upgrade_process_version":
          if (fail) return new Error("保存できません");
          return { type: "develop", ...mockDevelopState({ variant_id: opened, can_undo: true }) };
        default:
          return { type: "ok" };
      }
    });
    mock.responses.set("develop_frame", new ArrayBuffer(0));
  });

  it("コアに送り、返った設定と状態にする", async () => {
    await openDevelopFor(1);
    expect(developStore.get().settings?.process_version).toBe(1);
    await upgradeProcessVersion();
    const sent = mock.calls
      .filter((c) => c.command === "core_execute")
      .map((c) => (c.args?.command as { type: string }).type);
    expect(sent).toContain("upgrade_process_version");
    expect(developStore.get().settings?.process_version).toBe(2);
    expect(developStore.get().state?.can_undo).toBe(true);
  });

  it("更新できなければ、画面の版を戻す", async () => {
    // 前のテストと別の写真を開く（同じ写真は開き直さない）。
    await openDevelopFor(2);
    expect(developStore.get().settings?.process_version).toBe(1);
    fail = true;
    await expect(upgradeProcessVersion()).rejects.toBeTruthy();
    expect(developStore.get().settings?.process_version).toBe(1);
  });
});

describe("レンズ補正のパネルを描く", () => {
  /** テストの実行環境（node）には localStorage がないので、パネルを開いた状態で代わりを置く。 */
  beforeEach(() => {
    const data = new Map<string, string>([
      ["genzo.section.develop.lens", "1"],
      ["genzo.section.develop.info", "1"],
    ]);
    (globalThis as { localStorage?: unknown }).localStorage = {
      getItem: (k: string) => data.get(k) ?? null,
      setItem: (k: string, v: string) => void data.set(k, String(v)),
      removeItem: (k: string) => void data.delete(k),
    };
  });
  afterEach(() => {
    delete (globalThis as { localStorage?: unknown }).localStorage;
    developStore.set({ variantId: null, state: null, settings: null });
  });

  function render(state: DevelopState): string {
    appStore.set((s) => ({ selection: { ...s.selection, current: 1, currentIndex: 0 }, dialog: null }));
    developStore.set({ variantId: 1, state, settings: state.settings, opening: false, error: null });
    return renderToString(createElement(DevelopPanel)).replace(/<!-- -->/g, "");
  }

  it("RAW で埋め込みの情報があれば、その説明と各項目を出す", () => {
    const html = render(mockDevelopState());
    expect(html).toContain("レンズ補正");
    expect(html).toContain("プロファイルによる補正を有効");
    expect(html).toContain("色収差を除去");
    expect(html).toContain("補正の情報: Sony の ARW（歪曲・周辺減光・色収差）");
    expect(html).toContain("歪曲（適用量）");
    expect(html).toContain("赤 / シアン");
    expect(html).not.toContain("処理バージョンを 2 に更新");
  });

  it("処理バージョン 1 の写真では、更新のボタンを出す", () => {
    const html = render(mockDevelopState({ settings: mockDevelopSettings({ process_version: 1 }) }));
    expect(html).toContain("処理バージョンを 2 に更新");
    expect(html).toContain("（最新は 2）");
  });

  it("RAW 以外では、使えない補正を知らせる", () => {
    const html = render(mockDevelopState({ source: mockSourceInfo({ is_raw: false, lens_correction: null }) }));
    expect(html).toContain("RAW 以外の写真では、プロファイルによる補正・周辺減光・色収差の補正は使われません");
  });
});
