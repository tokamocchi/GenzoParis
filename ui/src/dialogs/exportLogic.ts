// 書き出しのダイアログ（EXP-01〜05）の入力の検証・ExportSettings への変換・ファイル名の規則・出力シャープと透かし・
// 動画の扱い・書き出しプリセットとの変換・前回の設定の復元・結果のまとめ（DOM を使わない純粋な計算）。
//
// ExportSettings の JSON の形は genzo-model の export.rs（`{"jpeg": {"quality": 90}}`・`"tiff16"`・
// `{"long_edge": 2048}` など）。値の範囲も同じところ（ExportSettings::validate）に合わせる。
//
// 書き出しプリセット（EXP-02。genzo-api の ExportPresetSettings）: 形式・色空間・大きさ・品質・メタデータ・同じ名前の
// 扱い・ファイル名の規則と、選べば書き出し先。プリセットを選ぶと入力をその値にする（書き出し先のないプリセットでは、
// 今の書き出し先のまま）。出力シャープ・透かし・解像度・動画（EXP-03・EXP-05）もプリセットに入る。この先プリセットの
// 項目が増えても、画面が知らない項目は選んだプリセットの値のまま保つ（presetFromForm の `base`）。別の OS で作った
// プリセットの書き出し先・透かしのファイルはコアが外して渡す（PresetInfo の warnings。書き出し先は今の値のまま、
// 透かしのファイルは選び直す）。
//
// 出力シャープ・透かし（EXP-03）・解像度・動画（EXP-05）は ExportSettings の項目（genzo-model の export.rs）。透かしの
// 大きさ・余白は写真の短辺に対する %（文字は文字の大きさ、画像は幅）。値の範囲は Watermark::problem に合わせる。

import type {
  ConflictPolicy,
  ErrorInfo,
  ExportPresetSettings,
  ExportReport,
  ExportSettings,
  JobInfo,
  OutputColorSpace,
  RenameRule,
  SharpenAmount,
  SharpenMedia,
  Watermark,
  WatermarkAnchor,
} from "../api/types";
import { type Notice, formatCount, groupErrors, groupReasons } from "./dialogFormat";
import { FORBIDDEN_NAME_CHARS, MAX_CUSTOM_TEXT_CHARS, MAX_SEQUENCE_START, MAX_TEMPLATE_CHARS } from "./importLogic";

/** JPEG の品質の範囲（genzo-model の ExportSettings::validate）。 */
export const JPEG_QUALITY_MIN = 1;
export const JPEG_QUALITY_MAX = 100;
/** JPEG の品質の既定値（genzo-model の ExportFormat::DEFAULT_JPEG_QUALITY）。 */
export const DEFAULT_JPEG_QUALITY = 90;
/** 長辺の画素数の上限（genzo-model の ExportSize::MAX_LONG_EDGE）。 */
export const MAX_LONG_EDGE = 65_535;
/** 初めて「長辺を指定」を選んだときの値（仮置き: 2048 px）。 */
export const DEFAULT_LONG_EDGE = 2048;

/** 解像度（ppi）の範囲と既定（genzo-model の ExportSettings）。 */
export const RESOLUTION_MIN = 1;
export const RESOLUTION_MAX = 1200;
export const DEFAULT_RESOLUTION = 72;
/** 透かし（genzo-model の Watermark）の範囲と既定（どれも仮置き）。 */
export const WATERMARK_SIZE_MIN = 1;
export const WATERMARK_SIZE_MAX = 100;
export const DEFAULT_TEXT_WATERMARK_SIZE = 4;
export const DEFAULT_IMAGE_WATERMARK_SIZE = 20;
export const WATERMARK_OPACITY_MIN = 1;
export const WATERMARK_OPACITY_MAX = 100;
export const DEFAULT_WATERMARK_OPACITY = 60;
export const WATERMARK_MARGIN_MAX = 25;
export const DEFAULT_WATERMARK_MARGIN = 2;
export const MAX_WATERMARK_TEXT_CHARS = 200;
export const MAX_WATERMARK_TEXT_LINES = 5;
export const MAX_FONT_INDEX = 255;

export type FormatKind = "jpeg" | "tiff16" | "png8" | "png16";
export type SizeMode = "original" | "long_edge";
export type WatermarkKind = "text" | "image";

/** 画面の入力（数値は入力欄の文字列のまま持ち、書き出すときに検証する）。 */
export interface ExportForm {
  format: FormatKind;
  quality: string;
  colorSpace: OutputColorSpace;
  sizeMode: SizeMode;
  longEdge: string;
  removeGps: boolean;
  /** キーワード（LIB-09。XMP の dc:subject）を書かない（既定はオフ = 書く）。 */
  removeKeywords: boolean;
  onConflict: ConflictPolicy;
  destDir: string;
  /** ファイル名の規則を使う（EXP-02。オフなら元のファイル名。仮想コピーは名前を足す）。 */
  rename: boolean;
  /** ファイル名のテンプレート（取り込みのリネームと同じ項目）。 */
  template: string;
  /** テンプレートの {text}。 */
  customText: string;
  /** 連番の開始番号（入力欄の文字列）。 */
  sequenceStart: string;
  /** 連番の桁数（1〜9）。 */
  sequenceDigits: number;
  /** 解像度（ppi。入力欄の文字列）。 */
  resolution: string;
  /** 出力シャープをかける（EXP-03）。 */
  sharpen: boolean;
  sharpenMedia: SharpenMedia;
  sharpenAmount: SharpenAmount;
  /** 透かしを付ける（EXP-03）。 */
  watermark: boolean;
  watermarkKind: WatermarkKind;
  /** 透かしの文字（改行で複数の行）。 */
  watermarkText: string;
  /** 文字のフォントのファイル（OS のフォント。アプリは同梱しない）。 */
  watermarkFont: string;
  /** フォントのコレクション（.ttc）の中の番号（入力欄の文字列）。 */
  watermarkFontIndex: string;
  /** 文字の色（#RRGGBB）。 */
  watermarkColor: string;
  /** 透かしの画像のファイル。 */
  watermarkImage: string;
  watermarkAnchor: WatermarkAnchor;
  /** 大きさ（%。入力欄の文字列）。 */
  watermarkSize: string;
  /** 不透明度（%）。 */
  watermarkOpacity: string;
  /** 余白（%）。 */
  watermarkMargin: string;
  /** 動画を書き出す（元のファイルのまま再エンコードせずにコピーする。EXP-05）。 */
  videos: boolean;
}

/** 書き出しのファイル名のテンプレートの既定（仮置き: 元の名前のまま。テンプレートを選ぶ前の入力欄の値）。 */
export const DEFAULT_EXPORT_TEMPLATE = "{name}";

export const FORMAT_OPTIONS: { value: FormatKind; label: string }[] = [
  { value: "jpeg", label: "JPEG（8bit）" },
  { value: "tiff16", label: "TIFF（16bit）" },
  { value: "png8", label: "PNG（8bit）" },
  { value: "png16", label: "PNG（16bit）" },
];

export const COLOR_SPACE_OPTIONS: { value: OutputColorSpace; label: string }[] = [
  { value: "srgb", label: "sRGB" },
  { value: "display_p3", label: "Display P3" },
  { value: "adobe_rgb", label: "Adobe RGB (1998)" },
];

export const CONFLICT_OPTIONS: { value: ConflictPolicy; label: string }[] = [
  { value: "sequence", label: "連番を付けて別の名前にする" },
  { value: "overwrite", label: "上書きする" },
  { value: "skip", label: "書き出さない（飛ばす）" },
];

export const SHARPEN_MEDIA_OPTIONS: { value: SharpenMedia; label: string }[] = [
  { value: "screen", label: "画面" },
  { value: "matte_paper", label: "マット紙" },
  { value: "glossy_paper", label: "光沢紙" },
];

export const SHARPEN_AMOUNT_OPTIONS: { value: SharpenAmount; label: string }[] = [
  { value: "low", label: "弱" },
  { value: "standard", label: "標準" },
  { value: "high", label: "強" },
];

/** 透かしの位置（左上から右下へ、行ごと。画面は 3 × 3 のボタンで選ぶ）。 */
export const WATERMARK_ANCHOR_OPTIONS: { value: WatermarkAnchor; label: string }[] = [
  { value: "top_left", label: "左上" },
  { value: "top", label: "上" },
  { value: "top_right", label: "右上" },
  { value: "left", label: "左" },
  { value: "center", label: "中央" },
  { value: "right", label: "右" },
  { value: "bottom_left", label: "左下" },
  { value: "bottom", label: "下" },
  { value: "bottom_right", label: "右下" },
];

export function colorSpaceLabel(cs: OutputColorSpace): string {
  return COLOR_SPACE_OPTIONS.find((o) => o.value === cs)?.label ?? cs;
}

/** 既定の入力（GPS の削除は既定でオン。EXP-04・SEC-03）。 */
export function defaultExportForm(colorSpace: OutputColorSpace): ExportForm {
  return {
    format: "jpeg",
    quality: String(DEFAULT_JPEG_QUALITY),
    colorSpace,
    sizeMode: "original",
    longEdge: String(DEFAULT_LONG_EDGE),
    removeGps: true,
    removeKeywords: false,
    onConflict: "sequence",
    destDir: "",
    rename: false,
    template: DEFAULT_EXPORT_TEMPLATE,
    customText: "",
    sequenceStart: "1",
    sequenceDigits: 4,
    resolution: String(DEFAULT_RESOLUTION),
    sharpen: false,
    sharpenMedia: "screen",
    sharpenAmount: "standard",
    watermark: false,
    watermarkKind: "text",
    watermarkText: "",
    watermarkFont: "",
    watermarkFontIndex: "0",
    watermarkColor: "#FFFFFF",
    watermarkImage: "",
    watermarkAnchor: "bottom_right",
    watermarkSize: String(DEFAULT_TEXT_WATERMARK_SIZE),
    watermarkOpacity: String(DEFAULT_WATERMARK_OPACITY),
    watermarkMargin: String(DEFAULT_WATERMARK_MARGIN),
    videos: true,
  };
}

/** 整数の入力を読む（全角の数字・桁区切りも受け付ける。整数でなければ null）。 */
export function parseIntInput(s: string): number | null {
  const t = s
    .trim()
    .replace(/[０-９]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0xfee0))
    .replace(/[,，]/g, "");
  if (!/^\d+$/.test(t)) return null;
  const n = Number(t);
  return Number.isSafeInteger(n) ? n : null;
}

export interface ExportFormErrors {
  quality?: string;
  longEdge?: string;
  destDir?: string;
  /** ファイル名の規則（テンプレート・任意の文字・連番）。 */
  naming?: string;
  /** 解像度。 */
  resolution?: string;
  /** 透かし（中身・大きさ・不透明度・余白）。 */
  watermark?: string;
}

const HEX_COLOR = /^#?[0-9a-fA-F]{6}$/;

/** 透かしの入力を確かめる（問題がなければ null。genzo-model の Watermark::problem と同じ範囲）。 */
export function validateWatermark(f: ExportForm): string | null {
  if (!f.watermark) return null;
  if (f.watermarkKind === "text") {
    if (f.watermarkText.trim() === "") return "透かしの文字を入れてください";
    if ([...f.watermarkText].length > MAX_WATERMARK_TEXT_CHARS) return `透かしの文字は ${MAX_WATERMARK_TEXT_CHARS} 文字までです`;
    // 行の数はコア（Rust の str::lines）と同じく、最後の改行の後を数えない。
    if (f.watermarkText.replace(/\n$/, "").split("\n").length > MAX_WATERMARK_TEXT_LINES) {
      return `透かしの文字は ${MAX_WATERMARK_TEXT_LINES} 行までです`;
    }
    // eslint-disable-next-line no-control-regex
    if (/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/.test(f.watermarkText)) return "透かしの文字に制御文字は使えません";
    if (f.watermarkFont.trim() === "") return "透かしの文字のフォントのファイルを選んでください";
    const idx = parseIntInput(f.watermarkFontIndex);
    if (idx === null || idx > MAX_FONT_INDEX) return `フォントの番号は 0〜${MAX_FONT_INDEX} の整数で指定してください`;
    if (!HEX_COLOR.test(f.watermarkColor.trim())) return "文字の色は #RRGGBB の形で指定してください";
  } else if (f.watermarkImage.trim() === "") {
    return "透かしの画像のファイルを選んでください";
  }
  const size = parseIntInput(f.watermarkSize);
  if (size === null || size < WATERMARK_SIZE_MIN || size > WATERMARK_SIZE_MAX) {
    return `透かしの大きさは ${WATERMARK_SIZE_MIN}〜${WATERMARK_SIZE_MAX} % の整数で指定してください`;
  }
  const opacity = parseIntInput(f.watermarkOpacity);
  if (opacity === null || opacity < WATERMARK_OPACITY_MIN || opacity > WATERMARK_OPACITY_MAX) {
    return `透かしの不透明度は ${WATERMARK_OPACITY_MIN}〜${WATERMARK_OPACITY_MAX} % の整数で指定してください`;
  }
  const margin = parseIntInput(f.watermarkMargin);
  if (margin === null || margin > WATERMARK_MARGIN_MAX) return `透かしの余白は 0〜${WATERMARK_MARGIN_MAX} % の整数で指定してください`;
  return null;
}

/** コアに渡す透かし（付けない・入力が正しくなければ null）。 */
export function toWatermark(f: ExportForm): Watermark | null {
  if (!f.watermark || validateWatermark(f) !== null) return null;
  const hex = f.watermarkColor.trim();
  return {
    content:
      f.watermarkKind === "text"
        ? {
            text: {
              text: f.watermarkText,
              font: f.watermarkFont.trim(),
              font_index: parseIntInput(f.watermarkFontIndex) as number,
              color: (hex.startsWith("#") ? hex : `#${hex}`).toUpperCase(),
            },
          }
        : { image: { path: f.watermarkImage.trim() } },
    anchor: f.watermarkAnchor,
    size: parseIntInput(f.watermarkSize) as number,
    opacity: parseIntInput(f.watermarkOpacity) as number,
    margin: parseIntInput(f.watermarkMargin) as number,
  };
}

/** 透かしの種類を切り替えたときの入力（大きさが前の種類の既定のままなら、新しい種類の既定にする）。 */
export function switchWatermarkKind(f: ExportForm, kind: WatermarkKind): ExportForm {
  const defaults: Record<WatermarkKind, string> = {
    text: String(DEFAULT_TEXT_WATERMARK_SIZE),
    image: String(DEFAULT_IMAGE_WATERMARK_SIZE),
  };
  const size = f.watermarkSize === defaults[f.watermarkKind] ? defaults[kind] : f.watermarkSize;
  return { ...f, watermarkKind: kind, watermarkSize: size };
}

/** ファイル名の規則の入力を確かめる（問題がなければ null。細かい規則はコアが確かめ、名前の例の誤りとして出す）。 */
export function validateNaming(f: Pick<ExportForm, "rename" | "template" | "customText" | "sequenceStart" | "sequenceDigits">): string | null {
  if (!f.rename) return null;
  if (f.template.trim() === "") return "ファイル名のテンプレートを入れてください";
  if ([...f.template].length > MAX_TEMPLATE_CHARS) return `テンプレートは ${MAX_TEMPLATE_CHARS} 文字までです`;
  if ([...f.customText].length > MAX_CUSTOM_TEXT_CHARS) return `任意の文字は ${MAX_CUSTOM_TEXT_CHARS} 文字までです`;
  const bad = [...f.customText].find((c) => FORBIDDEN_NAME_CHARS.includes(c));
  if (bad) return `任意の文字に「${bad}」は使えません`;
  const n = parseIntInput(f.sequenceStart);
  if (n === null || n > MAX_SEQUENCE_START) return `連番の開始番号は 0〜${formatCount(MAX_SEQUENCE_START)} の整数で指定してください`;
  if (!Number.isInteger(f.sequenceDigits) || f.sequenceDigits < 1 || f.sequenceDigits > 9) return "連番の桁数は 1〜9 です";
  return null;
}

/** コアに渡すファイル名の規則（使わない・入力が正しくなければ null）。 */
export function toFileNaming(f: ExportForm): RenameRule | null {
  if (!f.rename || validateNaming(f) !== null) return null;
  return {
    template: f.template,
    custom_text: f.customText,
    sequence_start: parseIntInput(f.sequenceStart) as number,
    sequence_digits: f.sequenceDigits,
  };
}

/** 入力を検証する（問題がなければ空のオブジェクト）。 */
export function validateExportForm(f: ExportForm): ExportFormErrors {
  const errors: ExportFormErrors = {};
  if (f.format === "jpeg") {
    const q = parseIntInput(f.quality);
    if (q === null || q < JPEG_QUALITY_MIN || q > JPEG_QUALITY_MAX) {
      errors.quality = `品質は ${JPEG_QUALITY_MIN}〜${JPEG_QUALITY_MAX} の整数で指定してください`;
    }
  }
  if (f.sizeMode === "long_edge") {
    const n = parseIntInput(f.longEdge);
    if (n === null || n < 1 || n > MAX_LONG_EDGE) {
      errors.longEdge = `長辺は 1〜${formatCount(MAX_LONG_EDGE)} px の整数で指定してください`;
    }
  }
  if (f.destDir.trim() === "") errors.destDir = "書き出し先のフォルダを選んでください";
  const naming = validateNaming(f);
  if (naming) errors.naming = naming;
  const ppi = parseIntInput(f.resolution);
  if (ppi === null || ppi < RESOLUTION_MIN || ppi > RESOLUTION_MAX) {
    errors.resolution = `解像度は ${RESOLUTION_MIN}〜${formatCount(RESOLUTION_MAX)} ppi の整数で指定してください`;
  }
  const watermark = validateWatermark(f);
  if (watermark) errors.watermark = watermark;
  return errors;
}

export function hasErrors(e: ExportFormErrors): boolean {
  return Object.keys(e).length > 0;
}

/** コアに渡す ExportSettings（形式・大きさの値が不正なら null）。 */
export function toExportSettings(f: ExportForm): ExportSettings | null {
  const e = validateExportForm(f);
  if (e.quality || e.longEdge || e.resolution || e.watermark) return null;
  return {
    format: f.format === "jpeg" ? { jpeg: { quality: parseIntInput(f.quality) as number } } : f.format,
    color_space: f.colorSpace,
    size: f.sizeMode === "original" ? "original" : { long_edge: parseIntInput(f.longEdge) as number },
    remove_gps: f.removeGps,
    on_conflict: f.onConflict,
    remove_keywords: f.removeKeywords,
    resolution_ppi: parseIntInput(f.resolution) as number,
    output_sharpening: f.sharpen ? { media: f.sharpenMedia, amount: f.sharpenAmount } : null,
    watermark: toWatermark(f),
    videos: f.videos ? "copy" : "skip",
  };
}

/** 選んだ動画の扱いの説明（画面の注意書き。動画がなければ null）。 */
export function videoNotice(f: Pick<ExportForm, "videos" | "removeGps">, videoCount: number): { level: "info" | "warn"; text: string } | null {
  if (videoCount <= 0) return null;
  const n = `動画 ${formatCount(videoCount)} 本`;
  if (!f.videos) return { level: "info", text: `${n}は書き出しません（「動画を書き出す」がオフ）` };
  if (f.removeGps) {
    return {
      level: "warn",
      text: `位置情報（GPS）を削除する設定では、${n}は書き出しません（動画は元のファイルのままコピーするため、位置情報を消せません）。動画も書き出すには「位置情報（GPS）を削除する」を外してください`,
    };
  }
  return {
    level: "info",
    text: `${n}は再エンコードせずに元のファイルのままコピーします（同じ名前があれば上書きせずに連番を付けます。透かし・出力シャープ・メタデータの設定は使いません）`,
  };
}

// ---------------------------------------------------------------------------
// 書き出しプリセット（EXP-02）
// ---------------------------------------------------------------------------

/** 書き出しの設定から入力の形式の部分（形式・品質）。 */
function formatFields(s: ExportSettings): Pick<ExportForm, "format" | "quality"> {
  if (typeof s.format === "object") return { format: "jpeg", quality: String(s.format.jpeg.quality) };
  return { format: s.format, quality: String(DEFAULT_JPEG_QUALITY) };
}

/**
 * プリセットの中身から入力を作る（書き出し先のないプリセットでは、今の書き出し先のまま。ファイル名の規則のない
 * プリセットでは元のファイル名にし、テンプレートの入力欄は今の値のまま）。
 */
export function formFromPreset(p: ExportPresetSettings, current: ExportForm): ExportForm {
  const e = p.export;
  const naming = p.file_naming;
  return {
    ...current,
    ...formatFields(e),
    colorSpace: e.color_space,
    sizeMode: e.size === "original" ? "original" : "long_edge",
    longEdge: e.size === "original" ? current.longEdge : String(e.size.long_edge),
    removeGps: e.remove_gps,
    removeKeywords: e.remove_keywords ?? false,
    onConflict: e.on_conflict,
    destDir: p.destination ?? current.destDir,
    rename: naming !== null,
    template: naming?.template ?? current.template,
    customText: naming?.custom_text ?? current.customText,
    sequenceStart: naming ? String(naming.sequence_start) : current.sequenceStart,
    sequenceDigits: naming?.sequence_digits ?? current.sequenceDigits,
    resolution: String(e.resolution_ppi ?? DEFAULT_RESOLUTION),
    sharpen: !!e.output_sharpening,
    sharpenMedia: e.output_sharpening?.media ?? current.sharpenMedia,
    sharpenAmount: e.output_sharpening?.amount ?? current.sharpenAmount,
    ...watermarkFields(e.watermark ?? null, current),
    videos: (e.videos ?? "copy") === "copy",
  };
}

/** 透かしから入力の透かしの部分（透かしのないプリセットでは付けないにし、入力欄は今の値のまま）。 */
function watermarkFields(w: Watermark | null, current: ExportForm): Partial<ExportForm> {
  if (!w) return { watermark: false };
  const common = {
    watermark: true,
    watermarkAnchor: w.anchor ?? "bottom_right",
    watermarkSize: String(w.size),
    watermarkOpacity: String(w.opacity ?? DEFAULT_WATERMARK_OPACITY),
    watermarkMargin: String(w.margin ?? DEFAULT_WATERMARK_MARGIN),
  };
  if ("text" in w.content) {
    const t = w.content.text;
    return {
      ...common,
      watermarkKind: "text",
      watermarkText: t.text,
      watermarkFont: t.font,
      watermarkFontIndex: String(t.font_index ?? 0),
      watermarkColor: t.color ?? "#FFFFFF",
      watermarkImage: current.watermarkImage,
    };
  }
  return { ...common, watermarkKind: "image", watermarkImage: w.content.image.path };
}

/**
 * 入力からプリセットの中身を作る（入力が正しくなければ null）。`withDestination` なら今の書き出し先を保存する（空なら
 * 保存しない）。`base`（更新するプリセットの今の中身）があれば、画面が知らない項目（新しい版のアプリが足した項目
 * など）をその値のまま残す。
 */
export function presetFromForm(
  f: ExportForm,
  withDestination: boolean,
  base?: ExportPresetSettings,
): ExportPresetSettings | null {
  const settings = toExportSettings(f);
  if (!settings || validateNaming(f) !== null) return null;
  const dest = f.destDir.trim();
  return {
    ...base,
    export: { ...base?.export, ...settings },
    destination: withDestination && dest !== "" ? dest : null,
    file_naming: toFileNaming(f),
  };
}

/** 入力がプリセットと同じか（書き出し先はプリセットにあるときだけ比べる）。 */
export function formMatchesPreset(f: ExportForm, p: ExportPresetSettings): boolean {
  const mine = presetFromForm(f, p.destination !== null);
  if (!mine) return false;
  // 省略された項目は既定値として比べる（古いプリセット・古い画面が保存したもの）。
  const norm = (x: ExportPresetSettings) =>
    JSON.stringify({
      export: {
        ...x.export,
        remove_keywords: x.export.remove_keywords ?? false,
        resolution_ppi: x.export.resolution_ppi ?? DEFAULT_RESOLUTION,
        output_sharpening: x.export.output_sharpening ?? null,
        watermark: normWatermark(x.export.watermark ?? null),
        videos: x.export.videos ?? "copy",
      },
      destination: x.destination,
      file_naming: x.file_naming,
    });
  return norm(mine) === norm({ ...p, export: { ...p.export } });
}

/** 比べるための透かし（省略された項目を既定値で埋め、項目の順をそろえる）。 */
function normWatermark(w: Watermark | null): unknown {
  if (!w) return null;
  const content =
    "text" in w.content
      ? {
          text: {
            text: w.content.text.text,
            font: w.content.text.font,
            font_index: w.content.text.font_index ?? 0,
            color: (w.content.text.color ?? "#FFFFFF").toUpperCase(),
          },
        }
      : { image: { path: w.content.image.path } };
  return {
    content,
    anchor: w.anchor ?? "bottom_right",
    size: w.size,
    opacity: w.opacity ?? DEFAULT_WATERMARK_OPACITY,
    margin: w.margin ?? DEFAULT_WATERMARK_MARGIN,
  };
}

/** 出力シャープの説明（「マット紙・強」など）。 */
export function sharpeningLabel(media: SharpenMedia, amount: SharpenAmount): string {
  const m = SHARPEN_MEDIA_OPTIONS.find((o) => o.value === media)?.label ?? media;
  const a = SHARPEN_AMOUNT_OPTIONS.find((o) => o.value === amount)?.label ?? amount;
  return `${m}・${a}`;
}

/** プリセットの中身の要約（選択肢の説明）。 */
export function describeExportPreset(p: ExportPresetSettings): string {
  const e = p.export;
  const fmt = FORMAT_OPTIONS.find((o) => o.value === (typeof e.format === "object" ? "jpeg" : e.format))?.label ?? "";
  const parts = [typeof e.format === "object" ? `${fmt}・品質 ${e.format.jpeg.quality}` : fmt, colorSpaceLabel(e.color_space)];
  parts.push(e.size === "original" ? "元の大きさ" : `長辺 ${formatCount(e.size.long_edge)} px`);
  if (e.remove_gps) parts.push("GPS を削除");
  if (e.output_sharpening) parts.push(`出力シャープ ${sharpeningLabel(e.output_sharpening.media, e.output_sharpening.amount)}`);
  if (e.watermark) parts.push("text" in e.watermark.content ? `透かし「${e.watermark.content.text.text}」` : "透かし（画像）");
  if (e.videos === "skip") parts.push("動画は書き出さない");
  if (p.file_naming) parts.push(`名前 ${p.file_naming.template}`);
  parts.push(p.destination ? `書き出し先 ${p.destination}` : "書き出し先は選ぶ");
  return parts.join("・");
}

// ---------------------------------------------------------------------------
// 前回の設定（localStorage）
// ---------------------------------------------------------------------------

/** 保存する形（`defaultColorSpace` は保存した時点のカタログの既定の色空間）。 */
export interface SavedExport {
  v: 1;
  form: ExportForm;
  defaultColorSpace: OutputColorSpace;
  /** 前回選んだ書き出しプリセット（EXP-02。なければ null）。 */
  presetId?: string | null;
}

const FORMATS: readonly string[] = ["jpeg", "tiff16", "png8", "png16"];
const COLOR_SPACES: readonly string[] = ["srgb", "display_p3", "adobe_rgb"];
const CONFLICTS: readonly string[] = ["sequence", "overwrite", "skip"];
const MEDIAS: readonly string[] = ["screen", "matte_paper", "glossy_paper"];
const AMOUNTS: readonly string[] = ["low", "standard", "high"];
const ANCHORS: readonly string[] = WATERMARK_ANCHOR_OPTIONS.map((o) => o.value);

export function savedExport(
  form: ExportForm,
  coreDefault: OutputColorSpace,
  presetId: string | null = null,
): SavedExport {
  return { v: 1, form, defaultColorSpace: coreDefault, presetId };
}

/** 前回選んだ書き出しプリセット（なければ null）。 */
export function restoreExportPresetId(saved: unknown): string | null {
  if (typeof saved !== "object" || saved === null) return null;
  const id = (saved as Partial<SavedExport>).presetId;
  return typeof id === "string" ? id : null;
}

/**
 * 前回の設定から入力を作る（壊れた項目は既定値）。
 *
 * 色空間は、保存した後に設定の画面で既定の色空間を変えていれば、新しい既定を使う（変えていなければ前回の
 * 色空間）。
 */
export function restoreExportForm(saved: unknown, coreDefault: OutputColorSpace): ExportForm {
  const base = defaultExportForm(coreDefault);
  if (typeof saved !== "object" || saved === null) return base;
  const s = saved as Partial<SavedExport>;
  const f = (typeof s.form === "object" && s.form !== null ? s.form : {}) as Partial<ExportForm>;
  const str = (v: unknown, fallback: string) => (typeof v === "string" ? v : fallback);
  const bool = (v: unknown, fallback: boolean) => (typeof v === "boolean" ? v : fallback);
  const pick = <T extends string>(v: unknown, allowed: readonly string[], fallback: T): T =>
    typeof v === "string" && allowed.includes(v) ? (v as T) : fallback;
  const defaultChanged = s.defaultColorSpace !== coreDefault;
  return {
    format: pick(f.format, FORMATS, base.format),
    quality: str(f.quality, base.quality),
    colorSpace: defaultChanged ? coreDefault : pick(f.colorSpace, COLOR_SPACES, base.colorSpace),
    sizeMode: pick(f.sizeMode, ["original", "long_edge"], base.sizeMode),
    longEdge: str(f.longEdge, base.longEdge),
    removeGps: typeof f.removeGps === "boolean" ? f.removeGps : base.removeGps,
    removeKeywords: typeof f.removeKeywords === "boolean" ? f.removeKeywords : base.removeKeywords,
    onConflict: pick(f.onConflict, CONFLICTS, base.onConflict),
    destDir: str(f.destDir, base.destDir),
    // ファイル名の規則（EXP-02）。前の版の保存した値になければ既定（元のファイル名）。
    rename: typeof f.rename === "boolean" ? f.rename : base.rename,
    template: str(f.template, base.template),
    customText: str(f.customText, base.customText),
    sequenceStart: str(f.sequenceStart, base.sequenceStart),
    sequenceDigits:
      typeof f.sequenceDigits === "number" && Number.isInteger(f.sequenceDigits) ? f.sequenceDigits : base.sequenceDigits,
    // 解像度・出力シャープ・透かし・動画（EXP-03・EXP-05）。前の版の保存した値になければ既定。
    resolution: str(f.resolution, base.resolution),
    sharpen: bool(f.sharpen, base.sharpen),
    sharpenMedia: pick(f.sharpenMedia, MEDIAS, base.sharpenMedia),
    sharpenAmount: pick(f.sharpenAmount, AMOUNTS, base.sharpenAmount),
    watermark: bool(f.watermark, base.watermark),
    watermarkKind: pick(f.watermarkKind, ["text", "image"], base.watermarkKind),
    watermarkText: str(f.watermarkText, base.watermarkText),
    watermarkFont: str(f.watermarkFont, base.watermarkFont),
    watermarkFontIndex: str(f.watermarkFontIndex, base.watermarkFontIndex),
    watermarkColor: str(f.watermarkColor, base.watermarkColor),
    watermarkImage: str(f.watermarkImage, base.watermarkImage),
    watermarkAnchor: pick(f.watermarkAnchor, ANCHORS, base.watermarkAnchor),
    watermarkSize: str(f.watermarkSize, base.watermarkSize),
    watermarkOpacity: str(f.watermarkOpacity, base.watermarkOpacity),
    watermarkMargin: str(f.watermarkMargin, base.watermarkMargin),
    videos: bool(f.videos, base.videos),
  };
}

// ---------------------------------------------------------------------------
// 結果のまとめ
// ---------------------------------------------------------------------------

/** 原本の保護（protected_destination）の分かりやすい文と案内。 */
const PROTECTED_MESSAGE = "原本と同じ場所・同じ名前になるため書き出しませんでした（原本は変更していません）";
const PROTECTED_HINT = "書き出し先のフォルダか、同じ名前があるときの扱い（連番）を変えてください";

/** 失敗のエラーを表示用にする（原本の保護は分かりやすい文にする）。 */
function displayError(e: ErrorInfo): ErrorInfo {
  if (e.kind !== "protected_destination") return e;
  return { ...e, message: PROTECTED_MESSAGE, hint: PROTECTED_HINT };
}

/**
 * 書き出しの結果の説明。失敗はエラーの種類ごと（件数・例の文・案内）、飛ばした理由と警告は文ごとにまとめる
 * （失敗の文にはファイルのパスが入るため。dialogFormat.ts の groupErrors）。
 */
export function describeExportReport(r: ExportReport): Notice {
  const copied = r.copied ?? 0;
  const parts = r.written > 0 || copied === 0 ? [`${formatCount(r.written)} 枚を書き出しました`] : [];
  if (copied > 0) parts.push(`動画 ${formatCount(copied)} 本をコピーしました`);
  if (r.skipped > 0) parts.push(`${formatCount(r.skipped)} 枚は飛ばしました`);
  if (r.failed > 0) parts.push(`${formatCount(r.failed)} 枚は失敗しました`);
  const head = r.cancelled ? "書き出しを取り消しました" : "書き出し";
  const message = `${head}: ${parts.join("、")}`;

  const failures: ErrorInfo[] = [];
  const skippedReasons: string[] = [];
  const warnings: string[] = [];
  for (const item of r.items) {
    const o = item.outcome;
    if (o.status === "failed") failures.push(displayError(o.error));
    else if (o.status === "skipped") skippedReasons.push(o.reason);
    else warnings.push(...o.warnings);
  }
  const lines: string[] = [];
  if (failures.length > 0) lines.push("失敗:", ...groupErrors(failures, "枚"));
  if (skippedReasons.length > 0) lines.push("飛ばした理由:", ...groupReasons(skippedReasons, "枚"));
  if (warnings.length > 0) lines.push("警告:", ...groupReasons(warnings, "件", 3));
  lines.push(`書き出し先: ${r.dest_dir}`);

  let level: Notice["level"];
  if (r.failed > 0) level = r.written + copied > 0 ? "warning" : "error";
  else if (r.skipped > 0 || r.cancelled) level = "warning";
  else level = "success";
  return { level, message, detail: lines.join("\n") };
}

/**
 * 書き出しのジョブの結果の説明。ジョブそのものの失敗（state が failed）は、state/app.ts が
 * job_finished のイベントで通知するので null（二重に知らせない）。
 */
export function describeExportJob(info: JobInfo): Notice | null {
  if (info.state === "failed") return null;
  const r = info.result;
  if (r && r.type === "export") return describeExportReport(r);
  if (info.state === "cancelled") return { level: "info", message: "書き出しを取り消しました" };
  return { level: "success", message: "書き出しが終わりました" };
}
