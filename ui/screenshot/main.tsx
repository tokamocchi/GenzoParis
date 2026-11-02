// README のスクリーンショット（docs/images）を撮るための画面（Tauri なし）。
//
// Bridge をモックに差し替え、架空のカタログ（合成の画像 30 枚）で画面を描く。撮影は scripts/screenshots/shoot.cjs。
// アプリのビルド（ui/index.html）には入らない。型の検査（tsconfig の include）の対象外。
//
// - 写真の画像は genzo:// の URI（Windows の形 http://genzo.localhost/...）で読み、shoot.cjs が合成の画像で応える。
// - 現像のプレビュー（develop_frame）は、その画像を canvas で RGBA にして返す（現像の処理はしない）。
// - URL の ?sections=develop.curve=1,... で、右のパネルの節の開閉（components/Section.tsx の保存の値）を決める。

import { createRoot } from "react-dom/client";

import { installBridge } from "../src/api/bridge";
import {
  MOCK_COLLECTIONS,
  MOCK_CORE_SETTINGS,
  MOCK_IPTC,
  MOCK_KEYWORDS,
  mockAppStatus,
  mockDevelopPresets,
  mockDevelopSettings,
  mockDevelopState,
  mockExportPresets,
  mockProfileEntries,
  mockSnapshots,
  mockSummary,
} from "../src/api/mock";
import { previewUrl } from "../src/api/uri";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Any = any;

const W = window as Any;
W.__unknown = [] as string[];

const sections = new URLSearchParams(location.search).get("sections");
if (sections) {
  for (const kv of sections.split(",")) {
    const [k, v] = kv.split("=");
    localStorage.setItem(`genzo.section.${k}`, v);
  }
}

// --- 架空のカタログ ---------------------------------------------------------

const N = 30;
const TITLES = ["夕景", "山と湖", "夜景", "森", "砂丘", "オーロラ", "花畑", "霧", "桜", "海岸"];
const CAMERAS = ["SONY ILCE-7M4", "SONY ILCE-7CM2", "SONY ILCE-7RM5"];
const LENSES = ["FE 24-70mm F2.8 GM II", "FE 70-200mm F2.8 GM OSS II", "FE 20mm F1.8 G"];
const RATINGS = [5, 3, 4, 0, 2, 4, 5, 1, 3, 0];
const FLAGS = [1, 0, 1, 0, -1, 0, 1, 0, 0, 0];
const LABELS = [null, null, "red", null, "green", null, "yellow", null, "blue", null];
const pad = (n: number, w = 2) => String(n).padStart(w, "0");

const items = Array.from({ length: N }, (_, i) => {
  const id = i + 1;
  const utc = new Date(Date.UTC(2026, 4, 3 + Math.floor(i / 6), 1 + (i % 6), (i * 7) % 60, 0));
  const local = new Date(utc.getTime() + 9 * 3600_000);
  const raw = `${local.getUTCFullYear()}:${pad(local.getUTCMonth() + 1)}:${pad(local.getUTCDate())} ${pad(local.getUTCHours())}:${pad(local.getUTCMinutes())}:00`;
  return mockSummary({
    variant_id: id,
    asset_id: id,
    file_id: id,
    file_name: `DSC${pad(4100 + id * 3, 5)}.ARW`,
    rating: RATINGS[i % 10] as Any,
    flag: FLAGS[i % 10] as Any,
    color_label: LABELS[i % 10] as Any,
    camera: CAMERAS[i % 3],
    lens: LENSES[i % 3],
    width: 7008,
    height: 4672,
    capture: { raw, offset: "+09:00", tz_source: "exif", tz_assumed: "+09:00", correction_s: 0, utc: utc.toISOString() } as Any,
    thumb_rev: "r1",
  });
});

const catalog = {
  catalog_path: "/Users/me/Pictures/GenzoParis/catalog.db",
  data_dir: "/Users/me/Pictures/GenzoParis",
  startup: { previous_shutdown: "clean", migrated_from: null, recovered_file_ops: 0, backup: null },
  warnings: [],
  stats: { schema_version: 12, assets: N, variants: N, files: N, history_entries: 120, stacks: 0 },
  capabilities: { raw_decoder: "LibRaw 0.21", worker_version: "0.2.0", gpu: "Apple M1 (Metal)" },
  worker_error: null,
  display: { assumed_srgb: false, reason: null, description: "Display P3" },
  tools: { ffmpeg: null, ffprobe: null },
  core_settings: MOCK_CORE_SETTINGS,
};
const status: Any = mockAppStatus();
status.platform = "macos";
status.version = "0.2.0";
status.catalog = catalog;
status.settings.last_catalog = catalog.catalog_path;

const FOLDER_ROOT = "/Users/me/Pictures";
const folders = [
  { folder_id: 1, volume_id: 1, parent_id: null, rel_path: "Users/me/Pictures", path: FOLDER_ROOT, online: true },
  { folder_id: 2, volume_id: 1, parent_id: 1, rel_path: "Users/me/Pictures/2026", path: `${FOLDER_ROOT}/2026`, online: true },
  { folder_id: 3, volume_id: 1, parent_id: 2, rel_path: "Users/me/Pictures/2026/05-北海道", path: `${FOLDER_ROOT}/2026/05-北海道`, online: true },
  { folder_id: 4, volume_id: 1, parent_id: 2, rel_path: "Users/me/Pictures/2026/04-京都", path: `${FOLDER_ROOT}/2026/04-京都`, online: true },
  { folder_id: 5, volume_id: 1, parent_id: 2, rel_path: "Users/me/Pictures/2026/03-沖縄", path: `${FOLDER_ROOT}/2026/03-沖縄`, online: true },
];

// 現像の設定の見本（基本補正・トーンカーブ・カラーグレーディングに値を入れる）。
const developSettings = mockDevelopSettings({
  white_balance: { custom: { temperature_k: 5600, tint: 6 } } as Any,
  exposure_ev: 0.35,
  contrast: 12,
  tone: { highlights: -48, shadows: 36, whites: 8, blacks: -14 },
  presence: { texture: 10, clarity: 18, dehaze: 6 },
});
developSettings.color.vibrance = 22;
developSettings.color.saturation = 4;
developSettings.color.grading.shadows = { hue: 215, saturation: 22, luminance: -4 };
developSettings.color.grading.midtones = { hue: 190, saturation: 6, luminance: 0 };
developSettings.color.grading.highlights = { hue: 38, saturation: 18, luminance: 3 };
developSettings.tone_curve.luma = [
  { x: 0, y: 0 },
  { x: 0.25, y: 0.2 },
  { x: 0.75, y: 0.82 },
  { x: 1, y: 1 },
];

const HISTORY = ["読み込み", "露光量", "ハイライト", "シャドウ", "自然な彩度", "トーンカーブ"];

let developing = 1;

function histogram() {
  const bell = (mid: number) =>
    Array.from({ length: 256 }, (_, i) =>
      Math.round(4000 * Math.exp(-((i - mid) ** 2) / 2600) + 1500 * Math.exp(-((i - 200) ** 2) / 900)),
    );
  const none = { red: 0, green: 0, blue: 0, any: 0 };
  return { red: bell(110), green: bell(125), blue: bell(140), luma: bell(122), pixel_count: 1_500_000, clipped_high: none, clipped_low: none };
}

/** 現像のプレビュー（frames.ts の形: ヘッダーの JSON の長さ・JSON・RGBA）。 */
async function developFrame(id: number): Promise<ArrayBuffer> {
  const img = new Image();
  img.crossOrigin = "anonymous";
  img.src = previewUrl(id, "r1");
  await img.decode();
  const canvas = document.createElement("canvas");
  canvas.width = img.naturalWidth;
  canvas.height = img.naturalHeight;
  const ctx = canvas.getContext("2d")!;
  ctx.drawImage(img, 0, 0);
  const pixels = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
  const header = {
    variant_id: id,
    generation: 1,
    quality: "final",
    width: canvas.width,
    height: canvas.height,
    backend: "gpu",
    histogram: histogram(),
    display_assumed_srgb: false,
    warnings: [],
    uncropped: false,
    output_width: 7008,
    output_height: 4672,
    g_bounds: [0, 0, 1, 1],
  };
  const json = new TextEncoder().encode(JSON.stringify(header));
  const out = new Uint8Array(4 + json.length + pixels.length);
  new DataView(out.buffer).setUint32(0, json.length, true);
  out.set(json, 4);
  out.set(pixels, 4 + json.length);
  return out.buffer;
}

function details(id: number) {
  const summary = items[id - 1];
  return {
    summary,
    camera_serial: "5001234",
    iso: [100, 200, 400, 3200][id % 4],
    aperture: [2.8, 5.6, 8, 4][id % 4],
    shutter_s: [1 / 250, 1 / 1000, 1 / 60, 2][id % 4],
    focal_mm: [24, 70, 35, 200][id % 4],
    gps: { lat: 43.06 + id * 0.01, lon: 141.35 + id * 0.01 },
    gps_source: "camera",
    camera_gps: null,
    caption: null,
    metadata: { ...MOCK_IPTC, title: TITLES[(id - 1) % 10], city: "札幌市", caption: null },
    video: null,
    files: [
      {
        file_id: id,
        role: "primary",
        path: `${FOLDER_ROOT}/2026/05-北海道/${summary.file_name}`,
        file_name: summary.file_name,
        size: 48_000_000,
        status: "ok",
        format: "ARW",
      },
    ],
    variants: [{ variant_id: id, is_master: true, variant_name: null }],
    stack: null,
  };
}

/** コア（core_execute）の応答。 */
function core(c: Any): Any {
  switch (c.type) {
    case "search":
      return { type: "search", generation: 1, count: N };
    case "range":
      return { type: "range", generation: 1, start: c.start, items: items.slice(c.start, c.start + c.len) };
    case "index_of":
      return { type: "index_of", generation: 1, index: c.variant_id - 1 };
    case "shown_variants":
      return { type: "shown_variants", generation: 1, variant_ids: c.variant_ids };
    case "count":
      return { type: "count", count: N };
    case "folders":
      return { type: "folders", folders };
    case "volumes":
      return {
        type: "volumes",
        volumes: [
          { volume_id: 1, uuid: "uuid:1", label: "Macintosh HD", mount_path: "/", online: true, identified: true, network: false, fs_type: "apfs", last_seen_at: "2026-11-01T00:00:00Z" },
        ],
      };
    case "catalog_stats":
      return { type: "catalog_stats", ...catalog.stats };
    case "core_settings":
      return { type: "core_settings", settings: MOCK_CORE_SETTINGS };
    case "facets":
      return {
        type: "facets",
        cameras: CAMERAS.map((value) => ({ value, count: N / 3 })),
        lenses: LENSES.map((value) => ({ value, count: N / 3 })),
        codecs: [],
      };
    case "keyword_tree":
      return { type: "keywords", keywords: MOCK_KEYWORDS };
    case "collections":
      return { type: "collections", collections: MOCK_COLLECTIONS };
    case "variant_details":
      return { type: "details", ...details(c.variant_id) };
    case "variant_keywords":
      return {
        type: "variant_keywords",
        variants: c.variant_ids.length,
        keywords: [{ keyword_id: 2, name: "北海道", path: ["場所", "北海道"], count: c.variant_ids.length }],
      };
    case "variant_collections":
      return { type: "variant_collections", collection_ids: [] };
    case "variant_stacks":
      return { type: "stacks", stacks: [] };
    case "metadata_summary":
      return { type: "metadata", assets: c.variant_ids.length, fields: [] };
    case "open_develop":
      developing = c.variant_id;
      return {
        type: "develop",
        ...mockDevelopState({ variant_id: c.variant_id, settings: developSettings, can_undo: true }),
      };
    case "develop_state":
      return { type: "develop_or_none", state: null };
    case "history":
      return {
        type: "history",
        entries: HISTORY.map((label, i) => ({
          id: i + 1,
          created_at: `2026-05-04T10:0${i}:00Z`,
          label,
          is_current: i === HISTORY.length - 1,
        })).reverse(),
      };
    case "snapshots":
      return { type: "snapshots", snapshots: mockSnapshots(c.variant_id) };
    case "presets":
      return { type: "presets", presets: c.kind === "export" ? mockExportPresets() : mockDevelopPresets(), dir: null };
    case "list_profiles":
      return { type: "profiles", profiles: mockProfileEntries() };
    case "export_name_preview":
      return {
        type: "name_preview",
        examples: [{ source: "DSC04106.ARW", destination: "20260503_北海道-0001.jpg", captured: "2026-05-03 11:07:00" }],
        error: null,
      };
    case "ai_mask_status":
      return {
        type: "ai_mask_status",
        available: true,
        kinds: ["subject", "sky", "person"],
        models: ["u2netp-subject", "sky", "person"],
        backend: "onnxruntime",
        problems: [],
        models_dir: null,
        bundled_models_dir: null,
        onnxruntime: null,
        rasters: 0,
        raster_bytes: 0,
      };
    case "flush_develop":
      return { type: "saved", saved: true };
    case "release_view":
      return { type: "ok" };
  }
  W.__unknown.push(`core_execute:${c.type}`);
  return { type: "ok" };
}

installBridge({
  async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    switch (command) {
      case "app_status":
        return status;
      case "set_display_target":
        return catalog.display as T;
      case "core_execute":
        return core(args?.command);
      case "save_import_preferences":
      case "update_app_settings":
        return status.settings;
    }
    W.__unknown.push(command);
    return undefined as T;
  },
  async invokeBinary(command: string): Promise<ArrayBuffer> {
    if (command === "develop_frame") return developFrame(developing);
    W.__unknown.push(command);
    return new ArrayBuffer(0);
  },
  async listen() {
    return () => undefined;
  },
});

// Bridge を差し替えてから画面を読み込む。
const { App } = await import("../src/App");
await import("../src/styles/base.css");
const root = document.getElementById("root");
if (root) createRoot(root).render(<App />);
