// コピー取り込み（IMP-02）・リネーム規則（IMP-03）・取り込み時のキーワードと説明のメタデータ（IMP-05）の入力と
// 結果の知らせのテスト。

import { describe, expect, it } from "vitest";

import { MOCK_COPY_IMPORT_REPORT, MOCK_IMPORT_PREFERENCES, MOCK_IMPORT_REPORT } from "../api/mock";
import type { ImportReport, JobInfo } from "../api/types";
import { continueSequence } from "../state/app";
import {
  DEFAULT_IMPORT_PREFERENCES,
  FOLDER_RULE_OPTIONS,
  TEMPLATE_TOKENS,
  copySettingsOf,
  describeImportReport,
  importFormFrom,
  importMetadataOf,
  importOptionsOf,
  importPreferencesOf,
  insertToken,
  metadataCount,
  validateImportForm,
} from "./importLogic";

describe("取り込みの設定の入力（アプリに保存したものが初めの値）", () => {
  it("保存した設定から入力を作り、保存する形に戻す", () => {
    const form = importFormFrom(MOCK_IMPORT_PREFERENCES);
    expect(form.copy).toBe(true);
    expect(form.destination).toBe("/home/u/写真");
    expect(form.template).toBe("{date}_{text}-{seq}");
    expect(form.sequenceStart).toBe("1");
    expect(form.keywords).toBe("場所 > 京都, 家族");
    expect(form.fields.creator).toBe("岡本");
    expect(importPreferencesOf(form)).toEqual(MOCK_IMPORT_PREFERENCES);
  });

  it("保存した設定がない（古いアプリ）ときは既定", () => {
    const form = importFormFrom(undefined);
    expect(form.copy).toBe(false);
    expect(form.recursive).toBe(true);
    expect(form.folderRule).toBe("year_date");
    expect(form.template).toBe("{date}-{seq}");
    expect(importPreferencesOf(form)).toEqual(DEFAULT_IMPORT_PREFERENCES);
  });

  it("フォルダの規則の選択肢はコアと同じ 7 種類、項目は 12 種類", () => {
    expect(FOLDER_RULE_OPTIONS.map((o) => o.value)).toEqual([
      "year_date",
      "year_month_day",
      "year_month",
      "date",
      "date_compact",
      "single",
      "source",
    ]);
    expect(TEMPLATE_TOKENS).toHaveLength(12);
    expect(new Set(TEMPLATE_TOKENS.map((t) => t.token)).size).toBe(12);
  });
});

describe("取り込みの指定", () => {
  it("コピーしないときは copy を送らず、付けるものがなければ metadata も送らない", () => {
    const form = importFormFrom({ ...DEFAULT_IMPORT_PREFERENCES });
    expect(copySettingsOf(form)).toBeNull();
    expect(importOptionsOf(form, { skip_duplicates: true, separate_raw_jpeg: false })).toEqual({
      skip_duplicates: true,
      separate_raw_jpeg: false,
    });
  });

  it("コピー・リネーム・付けるものを送る（空の項目・空のキーワードは除く）", () => {
    const form = {
      ...importFormFrom(MOCK_IMPORT_PREFERENCES),
      keywords: "場所 > 京都, , 家族, 家族",
      fields: { creator: " 岡本 ", city: "   ", copyright_status: "" },
    };
    const options = importOptionsOf(form, { skip_duplicates: false, separate_raw_jpeg: true });
    expect(options.skip_duplicates).toBe(false);
    expect(options.copy).toEqual({
      destination: "/home/u/写真",
      folder_rule: "year_date",
      rename: { template: "{date}_{text}-{seq}", custom_text: "京都", sequence_start: 1, sequence_digits: 4 },
    });
    expect(options.metadata).toEqual({ keywords: [["場所", "京都"], ["家族"]], fields: { creator: "岡本" } });
    expect(metadataCount(form)).toBe(3);
    // 名前を変えないときは rename を null にする。
    expect(copySettingsOf({ ...form, rename: false })?.rename).toBeNull();
    expect(importMetadataOf({ ...form, keywords: "", fields: {} })).toEqual({ keywords: [], fields: {} });
  });
});

describe("入力の確かめ", () => {
  const base = importFormFrom(MOCK_IMPORT_PREFERENCES);

  it("正しい入力は誤りなし", () => {
    expect(validateImportForm(base)).toEqual([]);
  });

  it("コピー先・テンプレート・任意の文字・連番・キーワード・長すぎる値", () => {
    expect(validateImportForm({ ...base, destination: " " })).toEqual(["コピー先のフォルダを選んでください"]);
    expect(validateImportForm({ ...base, template: "" })).toEqual(["ファイル名のテンプレートを入れてください"]);
    expect(validateImportForm({ ...base, template: "{date}/{seq}" })[0]).toContain("「/」");
    // 項目の名前の中（波かっこの中）は確かめない（コアが名前の例で確かめる）。
    expect(validateImportForm({ ...base, template: "{da:te}" })).toEqual([]);
    expect(validateImportForm({ ...base, customText: "a?b" })[0]).toContain("任意の文字");
    expect(validateImportForm({ ...base, sequenceStart: "-1" })[0]).toContain("連番の開始番号");
    expect(validateImportForm({ ...base, sequenceStart: "1000000000" })[0]).toContain("連番の開始番号");
    expect(validateImportForm({ ...base, sequenceDigits: 0 })).toEqual(["連番の桁数は 1〜9 です"]);
    expect(validateImportForm({ ...base, keywords: "場所 > a\tb" })[0]).toContain("キーワード");
    expect(validateImportForm({ ...base, fields: { creator: "あ".repeat(2001) } })).toEqual([
      "作成者は 2000 文字までです",
    ]);
    // 名前を変えない・コピーしないときは、テンプレートを確かめない。
    expect(validateImportForm({ ...base, rename: false, template: "" })).toEqual([]);
    expect(validateImportForm({ ...base, copy: false, destination: "" })).toEqual([]);
  });
});

describe("テンプレートの入力欄", () => {
  it("選択範囲を項目で置き換え、カーソルは項目の後ろ", () => {
    expect(insertToken("{date}-", "{seq}", 7, 7)).toEqual({ text: "{date}-{seq}", cursor: 12 });
    expect(insertToken("abcdef", "{seq}", 1, 3)).toEqual({ text: "a{seq}def", cursor: 6 });
    // 位置が分からなければ末尾に足す。範囲の外は丸める。
    expect(insertToken("x", "{name}", null, null)).toEqual({ text: "x{name}", cursor: 7 });
    expect(insertToken("x", "{name}", 9, 12)).toEqual({ text: "x{name}", cursor: 7 });
  });
});

describe("コピー取り込みの結果の知らせ", () => {
  it("コピーした数・大きさ・既にあったもの・付けたもの・コピーしたファイルの一覧", () => {
    const n = describeImportReport(MOCK_COPY_IMPORT_REPORT);
    expect(n.level).toBe("success");
    expect(n.message).toBe("コピー取り込み: 3 件をコピーし、3 件を追加");
    expect(n.detail).toContain("コピー先: /home/u/写真");
    expect(n.detail).toContain("コピーした大きさ: 30.0 MB");
    expect(n.detail).toContain("コピーせずに登録したファイル: 1 件");
    expect(n.detail).toContain("・DSC01234.ARW → /home/u/写真/2026/2026-10-13/20261013_京都-0001.ARW");
    expect(n.detail).toContain("2 枚の写真に、取り込み時のキーワード・メタデータを付けました");
  });

  it("確認だけ・取り消し・登録できなかったもの・更新日時で日付を決めたもの", () => {
    const r: ImportReport = {
      ...MOCK_COPY_IMPORT_REPORT,
      cancelled: true,
      not_registered: [{ path: "/media/u/SD/x.ARW", reason: "コピーした内容が元と一致しません" }],
      copy: { ...MOCK_COPY_IMPORT_REPORT.copy!, dry_run: true, dated_by_file_time: 2, bytes_copied: 0 },
    };
    const n = describeImportReport(r);
    expect(n.level).toBe("warning");
    expect(n.message).toBe("コピーの確認: 3 件のコピー先を決めました（コピーしていません）（途中で取り消しました）");
    expect(n.detail).toContain("コピー先（予定）:");
    expect(n.detail).toContain("ファイルの更新日時で日付を決めたファイル: 2 件");
    expect(n.detail).toContain("・登録できなかった: /media/u/SD/x.ARW（コピーした内容が元と一致しません）");
    expect(n.detail).not.toContain("コピーした大きさ");
  });

  it("その場で登録する取り込みの知らせは今までどおり", () => {
    expect(describeImportReport(MOCK_IMPORT_REPORT).message).toBe(
      "取り込み: 10 件を追加、0 件を更新、重複の 2 件は取り込まず",
    );
  });
});

describe("複数のフォルダのコピー取り込みの連番", () => {
  const done = (report: ImportReport): JobInfo => ({
    job_id: 1,
    kind: "import",
    state: "succeeded",
    done: 1,
    total: 1,
    result: { type: "import", ...report },
    error: null,
  });

  it("次のフォルダは前の結果の次の番号から", () => {
    const options = importOptionsOf(importFormFrom(MOCK_IMPORT_PREFERENCES), {});
    const next = continueSequence(options, done(MOCK_COPY_IMPORT_REPORT));
    expect(next?.copy?.rename?.sequence_start).toBe(3);
    expect(options.copy?.rename?.sequence_start).toBe(1);
  });

  it("名前を変えない・コピーしない・結果がないときはそのまま", () => {
    const plain = importOptionsOf(importFormFrom(DEFAULT_IMPORT_PREFERENCES), {});
    expect(continueSequence(plain, done(MOCK_IMPORT_REPORT))).toBe(plain);
    const copy = importOptionsOf(importFormFrom(MOCK_IMPORT_PREFERENCES), {});
    expect(continueSequence(copy, null)).toBe(copy);
    expect(continueSequence(undefined, done(MOCK_COPY_IMPORT_REPORT))).toBeUndefined();
  });
});
