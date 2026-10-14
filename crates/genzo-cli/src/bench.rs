//! `bench`: 計測（02 の MAINT-07、05 の 1.8 節のルール、PoC-3 の測定の準備）。
//!
//! genzo-testkit の [`Bench`] で測り、[`BenchRecorder`] で JSON（`<記録先>/<名前>.json`）に追記して、前回の
//! 結果（同じ名前・条件・入力・現像設定・環境）と比べる。報告する値は平均・95 パーセンタイル・最大
//! （[`genzo_testkit::bench::DurationStats`]）、目標を超えた回数。95 パーセンタイルが前回より 10% 以上
//! 悪化したもの（genzo-testkit の既定。仮置き）があれば終了コード 3（[`crate::EXIT_PARTIAL`]）。目標に届かないだけなら 0
//! （このコンテナの llvmpipe など、実機でない環境の数値で失敗にしないため）。
//!
//! # 計測する項目
//!
//! | 名前 | 内容（PoC-3 の測定方法） | 種類・条件 | 目標 |
//! |---|---|---|---|
//! | `cli_preview_stage_c` | 露光量のドラッグ（段階 C。A1・ガイドはキャッシュ）(1) | 応答時間・ウォーム | 33ms（PERF-01） |
//! | `cli_preview_wb_drag_draft` | WB のドラッグ（段階 A1 の簡易処理 → C）(1) | 応答時間・ウォーム | 1/15 秒（PERF-01b の 15fps） |
//! | `cli_preview_wb_release_final` | WB を離した後の最終品質の描画（A1 の最終品質 → ガイド → C）(2) | 応答時間・コールドとウォーム | 0.5 秒（PERF-01b） |
//! | `cli_export_jpeg` | 書き出しの時間の内訳: `decode`（ワーカーでの展開と転送）・`render`（現像）・`encode`・`write` (3) | 一括処理・コールド | 3 秒（PERF-10） |
//! | `cli_preview_brush_stroke` | ブラシで 1 本描いて離したとき（ブラシの部品にストロークを 1 本足して段階 C の最終品質。設定にブラシの部品があるときだけ） | 応答時間・ウォーム | 33ms（PERF-01。仮に同じ目標で比べる） |
//!
//! - プレビューは長辺 [`PREVIEW_LONG_EDGE`]（2560。1.8 節「プレビューは 2560 × 1707 で固定」）。入力の長辺が
//!   それより短いと、プレビューは入力の寸法になり（拡大しない）1.8 節の解像度にならないので、警告を出し、
//!   実際のプレビューの寸法を `notes.preview` に記録する（寸法の違う結果どうしは比べない）。
//! - コールドは、毎回エンジン（と GPU 版）のキャッシュを捨ててから測る。プロセスの起動直後（OS の
//!   ファイルキャッシュを含む）の時間ではない（genzo-testkit の bench の doc）。
//! - **測るのは呼び出しから結果が戻るまで** で、1.8 節の応答時間の定義（入力イベントから画面への表示まで）
//!   ではない。UI での計測は PoC-1・PoC-3 で行う（記録の `notes.scope = call_to_result`）。
//! - 展開と転送は、ワーカーの側の時間を受け取る仕組みがないため、1 つの内訳（`decode`）にまとめる。
//! - 入力: `--file`（ワーカーで展開）、または `--synthetic WxH`（合成の RAW。ワーカーが LibRaw を使える
//!   なら合成 DNG をワーカーで展開し、使えなければメモリの上の RAW を使う。`notes.input` で区別する）。
//!   どちらもなければ α7 IV の寸法（[`DEFAULT_SYNTHETIC_SIZE`]）の合成の RAW。
//! - GPU は `--gpu auto` で使えれば使う（GPU の失敗は CPU 版に切り替えず、エラーにする。処理した側の
//!   混ざった結果を記録しないため）。書き出しは genzo-gpu の [`genzo_gpu::export_with_fallback`]（失敗したら
//!   CPU 版。処理した側は `notes.backend`）。GPU の API（Vulkan・Metal・DX12 など）は `notes.gpu_backend`
//!   （API が違う結果どうしは比べない）、GPU の名前とドライバーは環境の情報に記録する。
//! - 計測の設定（[`bench_settings`]）には、`develop set` と同じオプション（ハイライト復元・ノイズ軽減・色・外観・
//!   シャープ・効果）、`--dcp`・`--cube`・`--look-amount`（DEV-17。取り込まずにファイルを使う。`render` と同じ）、
//!   `--masks FILE`（現像設定の `masks` と同じ形の JSON）または `--sample-masks`（[`sample_masks`]。DEV-18・DEV-19）を
//!   当てられる。設定は記録に入る（設定の違う結果どうしは比べない）。コールドの回は、ブラシのラスターの使い回し
//!   （genzo-pipeline の `local::brush`）も捨てる。
//! - `cli_preview_brush_stroke` は、設定の最初のブラシの部品に、毎回 1 本（画像の幅をなぞる、大きさ
//!   [`SAMPLE_BRUSH_SIZE`] の 161 点。画面と同じく点は大きさの 1/8 の間隔）を足して描く（足したストロークだけを
//!   塗り足す、ブラシで描いたときの経路）。足したストロークが [`MAX_ADDED_STROKES`] 本になったら元に戻す。
//! - 終了コード 3（悪化）にするのは、**今回の回数が 1.8 節の規則を満たす** 計測が前回より悪化した場合
//!   だけ（[`regression_fails`]）。`--runs` で回数を減らした試しの計測は、比べた結果を出すが終了コードには
//!   しない（少ない回数の 95 パーセンタイルは最大値とほぼ同じで、ばらつきで悪化と判定されやすいため）。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use genzo_gpu::{ExportBackend, GpuPreviewOptions, GpuRenderer};
use genzo_media::{DynRgbImage, ExifData, ProtectedFiles, RgbImage8, RgbImage16};
use genzo_model::{
    BrushStroke, ConflictPolicy, CurvePoint, DevelopSettings, ExportSettings, LocalAdjustment,
    LocalDelta, MaskComponent, MaskMode, MaskShape, RenderQuality, WhiteBalance,
};
use genzo_pipeline::{
    Engine, EngineConfig, ExportOptions, ExportPixels, NeverCancel, OutputTarget, PhotoSource,
    PreviewRequest, SourceImage,
};
use genzo_raw::{CfaPattern, RawImage};
use genzo_testkit::bayer::{BayerParams, mosaic, write_dng};
use genzo_testkit::bench::{Bench, BenchRecorder, BenchReport, BenchResult, PhaseTimer};
use genzo_testkit::image::FloatImage;
use genzo_testkit::{EnvironmentInfo, InputRef};
use serde_json::json;

use crate::Status;
use crate::args::{BenchArgs, BenchTarget, ColorArgs, DetailArgs, GlobalArgs, SensorArgs};
use crate::develop::read_json_input;
use crate::error::{CliError, CliResult};
use crate::output::{Output, json_value};
use crate::standalone::{
    InputKind, STANDALONE_SOURCE_ID, Worker, ensure_file, gpu_renderer, input_kind, load,
    profile_files,
};

/// プレビューの長辺（05 の 1.8 節「プレビューは 2560 × 1707 で固定」。02 の PERF-01）。
pub const PREVIEW_LONG_EDGE: u32 = 2560;

/// 合成の RAW の既定の寸法（α7 IV。04 の 2.2 節）。
pub const DEFAULT_SYNTHETIC_SIZE: (u32, u32) = (7008, 4672);

/// 段階 C（露光量のドラッグ）の目標（PERF-01: 1 フレーム 33ms 以内）。
pub const STAGE_C_TARGET: Duration = Duration::from_millis(33);

/// WB のドラッグ（簡易処理）の目標（PERF-01b: 15fps 以上 = 1 フレーム 1/15 秒以内）。
pub const WB_DRAG_TARGET: Duration = Duration::from_nanos(1_000_000_000 / 15);

/// WB を離した後の最終品質の目標（PERF-01b: 0.5 秒以内）。
pub const WB_RELEASE_TARGET: Duration = Duration::from_millis(500);

/// 書き出しの目標（PERF-10: α7 IV のフル解像度の JPEG で 1 枚あたり 3 秒以内）。
pub const EXPORT_TARGET: Duration = Duration::from_secs(3);

/// ドラッグを模す WB の色温度の範囲（K）と刻み（**仮置き**: 4000〜7000 K を 150 K ずつ。毎回 A1 の
/// キャッシュに当たらない値にするため。値そのものに意味はない）。
const WB_SWEEP: (f32, f32, f32) = (4000.0, 7000.0, 150.0);

/// 見本のブラシの大きさ（半径。G 座標。画面の既定と同じ）。
pub const SAMPLE_BRUSH_SIZE: f32 = 0.05;

/// `cli_preview_brush_stroke` で足すストロークの数の上限（超えたら足したものを除いて元に戻す。設定の上限を超えない
/// ため）。
pub const MAX_ADDED_STROKES: usize = 200;

/// 画像の幅（G 座標の 0〜1）を高さ `y` から `passes` 回なぞる（2 回目・4 回目… は右から左。なぞるごとに `step` だけ
/// 下へずらす）ストローク（大きさ [`SAMPLE_BRUSH_SIZE`]。点は大きさの 1/8 の間隔。1 回で 161 点）。
pub fn sample_stroke(y: f32, passes: usize, step: f32) -> BrushStroke {
    let spacing = SAMPLE_BRUSH_SIZE / 8.0;
    let n = (1.0 / spacing).round() as usize;
    let mut points = Vec::with_capacity(passes * (n + 1));
    for k in 0..passes {
        let yk = y + step * k as f32;
        for i in 0..=n {
            let x = i as f32 * spacing;
            points.push([if k % 2 == 0 { x } else { 1.0 - x }, yk]);
        }
    }
    BrushStroke {
        size: SAMPLE_BRUSH_SIZE,
        points,
        ..BrushStroke::default()
    }
}

/// `--sample-masks` の見本のマスク（ローカル補正の計測。DEV-18・DEV-19）: 線形グラデーション（上から）・円形
/// グラデーション（中央。ぼかし 50）・ブラシ（[`sample_stroke`] で画像の幅を 6 回なぞって塗りつぶす 1 本。966 点）に
/// 輝度範囲（20〜90）を交差させたもの。調整は色温度・露光量・コントラスト・ハイライト・明瞭度・彩度・シャープ
/// （ステージ 10〜14 のすべてで局所の値の面を使う）。
pub fn sample_masks() -> Vec<LocalAdjustment> {
    let delta = |f: &dyn Fn(&mut LocalDelta)| {
        let mut d = LocalDelta::default();
        f(&mut d);
        d
    };
    let linear = LocalAdjustment {
        delta: delta(&|d| {
            d.exposure_ev = -0.7;
            d.temperature = -15.0;
            d.dehaze = 20.0;
        }),
        ..LocalAdjustment::new(MaskComponent::add(MaskShape::Linear {
            x0: 0.5,
            y0: 0.05,
            x1: 0.5,
            y1: 0.35,
        }))
    };
    let radial = LocalAdjustment {
        delta: delta(&|d| {
            d.exposure_ev = 0.4;
            d.clarity = 25.0;
            d.saturation = 15.0;
        }),
        ..LocalAdjustment::new(MaskComponent::add(MaskShape::Radial {
            cx: 0.5,
            cy: 0.33,
            rx: 0.25,
            ry: 0.18,
            angle_deg: 0.0,
            feather: 50.0,
        }))
    };
    let mut brush = LocalAdjustment {
        delta: delta(&|d| {
            d.contrast = 20.0;
            d.highlights = -30.0;
            d.sharpness = 30.0;
        }),
        ..LocalAdjustment::new(MaskComponent::add(MaskShape::Brush {
            strokes: vec![sample_stroke(0.05, 6, 0.1)],
        }))
    };
    brush.components.push(MaskComponent {
        mode: MaskMode::Intersect,
        invert: false,
        shape: MaskShape::LuminanceRange {
            min: 20.0,
            max: 90.0,
            smoothness: 50.0,
        },
    });
    vec![linear, radial, brush]
}

/// `--masks` の JSON（マスクの配列、または `masks` を持つ現像設定）からマスクを読む。
fn read_masks(source: &str) -> CliResult<Vec<LocalAdjustment>> {
    let v = read_json_input(source)?;
    let list = match v {
        serde_json::Value::Array(_) => v,
        serde_json::Value::Object(mut o) => o.remove("masks").ok_or_else(|| {
            CliError::Usage(
                "--masks の JSON には、マスクの配列か masks を持つ現像設定を指定してください"
                    .to_owned(),
            )
        })?,
        _ => {
            return Err(CliError::Usage(
                "--masks の JSON には、マスクの配列か masks を持つ現像設定を指定してください"
                    .to_owned(),
            ));
        }
    };
    serde_json::from_value(list)
        .map_err(|e| CliError::Usage(format!("--masks のマスクを読めません: {e}")))
}

/// 計測の設定に、`--dcp`・`--cube`・`--look-amount`・`--masks`・`--sample-masks` を当てる（検証する）。カメラ
/// プロファイル・ルックのファイルがあれば、エンジンに渡すものを返す。
fn apply_profile_and_masks(
    a: &BenchArgs,
    settings: &mut DevelopSettings,
) -> CliResult<Option<Arc<genzo_pipeline::MemoryProfiles>>> {
    if a.sample_masks {
        settings.masks = sample_masks();
    } else if let Some(src) = &a.masks {
        settings.masks = read_masks(src)?;
    }
    let profiles = profile_files(a.dcp.as_ref(), a.cube.as_ref(), a.look_amount, settings)?;
    Ok(profiles.map(Arc::new))
}

/// 計測で使う現像設定（いろいろなステージが有効になる設定。genzo-pipeline の計測と同じ考え方）。
pub fn bench_settings() -> DevelopSettings {
    let mut s = DevelopSettings {
        exposure_ev: 0.5,
        contrast: 30.0,
        ..Default::default()
    };
    s.tone.highlights = -70.0;
    s.tone.shadows = 70.0;
    s.tone.whites = 20.0;
    s.tone.blacks = -20.0;
    s.color.saturation = 30.0;
    s.color.vibrance = 30.0;
    s.tone_curve.luma = vec![
        CurvePoint::new(0.0, 0.0),
        CurvePoint::new(0.25, 0.2),
        CurvePoint::new(0.75, 0.82),
        CurvePoint::new(1.0, 1.0),
    ];
    s
}

/// 計測用の合成のシーン（カメラ RGB。整数の演算と定数の表だけで作る。OS の数学ライブラリによらない）。
///
/// 横に 4 つの区画: グレーの段階 / 細い線 / 色のパッチ / 明暗差。
pub fn synthetic_scene(w: u32, h: u32) -> CliResult<FloatImage> {
    const GRAYS: [f32; 8] = [0.004, 0.01, 0.025, 0.06, 0.12, 0.25, 0.5, 0.9];
    const PATCHES: [[f32; 3]; 6] = [
        [0.60, 0.08, 0.05],
        [0.05, 0.50, 0.07],
        [0.04, 0.06, 0.55],
        [0.70, 0.62, 0.05],
        [0.30, 0.22, 0.16],
        [0.18, 0.18, 0.18],
    ];
    FloatImage::from_rgb_fn(w, h, |x, y| match (x * 4 / w).min(3) {
        0 => [GRAYS[(y * 8 / h).min(7) as usize]; 3],
        1 => {
            if x % 7 == 0 || y % 9 == 0 || (x + y) % 13 == 0 {
                [0.8, 0.75, 0.7]
            } else {
                [0.03, 0.035, 0.04]
            }
        }
        2 => PATCHES[((x / 11 + y / 11 * 5) % 6) as usize],
        _ => {
            let (cx, cy) = (x % 40, y % 40);
            if (10..30).contains(&cx) && (10..30).contains(&cy) {
                [0.9, 0.88, 0.86]
            } else {
                [0.006, 0.005, 0.004]
            }
        }
    })
    .map_err(CliError::other)
}

/// 計測用の合成の RAW（RGGB・典型的な形のカメラ行列。実在の機種の値ではない）。
pub fn synthetic_raw(w: u32, h: u32) -> CliResult<RawImage> {
    let scene = synthetic_scene(w, h)?;
    mosaic(
        &scene,
        &BayerParams {
            cfa: CfaPattern::RGGB,
            cam_xyz: Some([
                [0.70, -0.20, -0.06],
                [-0.50, 1.30, 0.23],
                [-0.07, 0.13, 0.65],
            ]),
            ..Default::default()
        },
    )
    .map_err(CliError::other)
}

/// 計測の入力。
struct BenchInput {
    /// ワーカーで展開するファイル（なければメモリの上の RAW）。
    file: Option<(PathBuf, InputKind)>,
    /// メモリの上の入力（ファイルがない場合）。
    memory: Option<PhotoSource>,
    /// 記録する入力の ID とハッシュ。
    input: InputRef,
    /// 入力の種類（`file` / `worker_dng` / `in_memory`）。
    how: &'static str,
    /// 合成の DNG を置く一時フォルダ（drop で消す）。
    _temp: Option<tempfile::TempDir>,
}

impl BenchInput {
    fn prepare(a: &BenchArgs, worker: &mut Worker) -> CliResult<Self> {
        if let Some(file) = &a.file {
            ensure_file(file)?;
            let kind = input_kind(file)?;
            if kind == InputKind::Video {
                return Err(CliError::Usage(
                    "動画は計測の入力にできません（写真のファイルを指定してください）".to_owned(),
                ));
            }
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let input = InputRef::from_file(format!("file:{name}"), file)
                .map_err(|e| CliError::io(file, e))?;
            return Ok(Self {
                file: Some((file.clone(), kind)),
                memory: None,
                input,
                how: "file",
                _temp: None,
            });
        }
        let (w, h) = a.synthetic.unwrap_or(DEFAULT_SYNTHETIC_SIZE);
        let raw = synthetic_raw(w, h)?;
        let params = format!("cli_synthetic_scene v1 {w}x{h} cfa=rggb");
        let input = InputRef::from_bytes("synthetic:cli_scene", params.as_bytes());
        // ワーカーが LibRaw を使えるなら、合成 DNG をワーカーで展開する（展開と転送も測る）。
        let raw_supported = worker.client().ping()?.raw_decoder.is_some();
        if raw_supported {
            let temp = tempfile::tempdir().map_err(|e| CliError::io(std::env::temp_dir(), e))?;
            let path = temp.path().join("synthetic.dng");
            write_dng(&path, &raw).map_err(CliError::other)?;
            return Ok(Self {
                file: Some((path, InputKind::Raw)),
                memory: None,
                input,
                how: "worker_dng",
                _temp: Some(temp),
            });
        }
        let source = PhotoSource::new(STANDALONE_SOURCE_ID, SourceImage::Raw(Arc::new(raw)))?;
        Ok(Self {
            file: None,
            memory: Some(source),
            input,
            how: "in_memory",
            _temp: None,
        })
    }

    /// 入力を展開する（ファイルならワーカーで。RAW のデコーダの識別子も返す）。
    fn load(&self, worker: &mut Worker) -> CliResult<(PhotoSource, Option<String>)> {
        match (&self.file, &self.memory) {
            (Some((path, kind)), _) => {
                let l = load(worker.client(), path, *kind)?;
                Ok((l.source, l.decoder))
            }
            (None, Some(s)) => Ok((s.clone(), None)),
            (None, None) => Err(CliError::other("計測の入力がありません")),
        }
    }
}

/// 計測の共通の設定（環境・入力・条件）。
fn base(name: &str, kind_latency: bool, ctx: &Ctx) -> Bench {
    let b = if kind_latency {
        Bench::latency(name)
    } else {
        Bench::batch(name)
    };
    let b = match ctx.runs {
        Some(n) => b.iterations(n),
        None => b,
    };
    let b = b
        .environment(ctx.env.clone())
        .input(ctx.input.clone())
        .note("input", ctx.how)
        .note("resolution", format!("{}x{}", ctx.size.0, ctx.size.1))
        .note("backend", if ctx.gpu.is_some() { "gpu" } else { "cpu" })
        .note("source", if ctx.is_raw { "raw" } else { "image" })
        .note("scope", "call_to_result");
    match &ctx.gpu {
        Some(g) => b.note("gpu_backend", g.context().summary().backend.clone()),
        None => b,
    }
}

/// プレビューの寸法（genzo-pipeline の [`genzo_pipeline::preview_size`] で長辺を [`PREVIEW_LONG_EDGE`] に
/// 縮める。拡大はしない）。向きと切り抜きは考えない（記録と、1.8 節の解像度との比較にだけ使う）。
pub fn preview_size((w, h): (u32, u32)) -> (u32, u32) {
    genzo_pipeline::preview_size(w, h, PREVIEW_LONG_EDGE)
}

/// 終了コードを 3（悪化）にするか: 前回より悪化し、今回の回数が 1.8 節の規則を満たす計測があるか。
pub fn regression_fails(reports: &[BenchReport]) -> bool {
    reports
        .iter()
        .any(|r| r.regressed() && r.result.meets_iteration_rule)
}

/// 計測の文脈。
struct Ctx {
    runs: Option<u32>,
    env: EnvironmentInfo,
    input: InputRef,
    how: &'static str,
    size: (u32, u32),
    /// RAW か（RAW 以外は簡易処理がなく、WB のドラッグも最終品質で描く）。
    is_raw: bool,
    gpu: Option<GpuRenderer>,
    /// 計測で使う現像設定（[`bench_settings_with`]。オプションの指定を当てて検証したもの）。
    settings: DevelopSettings,
    /// `--dcp`・`--cube` のカメラプロファイル・ルック（エンジンに渡す）。
    profiles: Option<Arc<genzo_pipeline::MemoryProfiles>>,
}

impl Ctx {
    /// 計測で使う現像設定。
    fn settings(&self) -> DevelopSettings {
        self.settings.clone()
    }

    /// 計測で使うエンジン（カメラプロファイル・ルックのファイルがあれば引けるようにする）。
    fn engine(&self) -> CliResult<Engine> {
        let engine = Engine::new(EngineConfig::default())?;
        Ok(match &self.profiles {
            Some(p) => engine.with_profile_resolver(p.clone()),
            None => engine,
        })
    }
}

/// 設定の最初のブラシの部品（マスクと部品の番号）。
fn first_brush(s: &DevelopSettings) -> Option<(usize, usize)> {
    s.masks.iter().enumerate().find_map(|(i, m)| {
        m.components
            .iter()
            .position(|c| matches!(c.shape, MaskShape::Brush { .. }))
            .map(|j| (i, j))
    })
}

/// エンジン・GPU 版・ブラシのラスターの使い回しを捨てる（コールドの回）。
fn clear_all(engine: &Engine, gpu: Option<&GpuRenderer>) {
    engine.clear_caches();
    if let Some(g) = gpu {
        g.clear_caches();
    }
    genzo_pipeline::local::brush::clear_cache();
}

/// 計測で使う現像設定: [`bench_settings`] に、`develop set` と同じオプション（ハイライト復元・ノイズ軽減・白黒・
/// カラーミキサー・カラーグレーディング・テクスチャ・明瞭度・かすみの除去・シャープ・効果）の指定を当てたもの。
/// 範囲の外の値は、計測を始める前にエラーにする（genzo-model の検証）。
pub fn bench_settings_with(
    sensor: &SensorArgs,
    color: &ColorArgs,
    detail: &DetailArgs,
) -> CliResult<DevelopSettings> {
    let mut s = bench_settings();
    sensor.apply(&mut s);
    color.apply(&mut s.color);
    detail.apply(&mut s);
    s.validate().map_err(genzo_model::DevelopError::from)?;
    Ok(s)
}

/// プレビューを 1 回描く（GPU があれば GPU 版。結果は読み出さず、ヒストグラムだけ求める）。
fn render_preview(
    engine: &Engine,
    gpu: Option<&GpuRenderer>,
    source: &PhotoSource,
    settings: &DevelopSettings,
    quality: RenderQuality,
) -> CliResult<()> {
    let request = PreviewRequest::new(quality, PREVIEW_LONG_EDGE)
        .with_output(OutputTarget::assumed_srgb_display()?)
        .with_histogram();
    match gpu {
        Some(g) => {
            let options = GpuPreviewOptions {
                download_b3: false,
                download_output: false,
                gpu_draft_a1: true,
            };
            g.render_preview(engine, source, settings, &request, &options)?;
        }
        None => {
            engine.render_preview_with(source, settings, &request)?;
        }
    }
    Ok(())
}

/// 計測の中で起きたエラーを覚えておく（計測の関数はエラーを返せないため）。
fn keep_err(slot: &mut Option<CliError>, r: CliResult<()>) {
    if let Err(e) = r
        && slot.is_none()
    {
        *slot = Some(e);
    }
}

fn next_wb(k: &mut f32) -> WhiteBalance {
    *k = if *k + WB_SWEEP.2 > WB_SWEEP.1 {
        WB_SWEEP.0
    } else {
        *k + WB_SWEEP.2
    };
    WhiteBalance::Custom {
        temperature_k: *k,
        tint: 0.0,
    }
}

fn bench_preview(ctx: &Ctx, source: &PhotoSource) -> CliResult<Vec<BenchResult>> {
    let engine = ctx.engine()?;
    let gpu = ctx.gpu.as_ref();
    let clear = || clear_all(&engine, gpu);
    let mut results = Vec::new();
    let mut err: Option<CliError> = None;

    let preview = preview_size(ctx.size);
    let preview_note = format!("{}x{}", preview.0, preview.1);
    let preview_bench = |name: &str| base(name, true, ctx).note("preview", preview_note.clone());

    // (1) 露光量のドラッグ（段階 C）。最初に 1 回描いて A1・ガイドをキャッシュに入れる。
    let mut s = ctx.settings();
    render_preview(&engine, gpu, source, &s, RenderQuality::Final)?;
    let mut ev = 0.0f32;
    let r = preview_bench("cli_preview_stage_c")
        .target(STAGE_C_TARGET)
        .develop_settings(&s)
        .run_warm(|| {
            ev = if ev > 1.0 { 0.0 } else { ev + 0.1 };
            s.exposure_ev = ev;
            keep_err(
                &mut err,
                render_preview(&engine, gpu, source, &s, RenderQuality::Final),
            );
        })?;
    results.push(r);

    // (1) WB のドラッグ（簡易処理）。
    let mut s = ctx.settings();
    let mut k = WB_SWEEP.0;
    let r = preview_bench("cli_preview_wb_drag_draft")
        .target(WB_DRAG_TARGET)
        .develop_settings(&s)
        .run_warm(|| {
            s.white_balance = next_wb(&mut k);
            keep_err(
                &mut err,
                render_preview(&engine, gpu, source, &s, RenderQuality::Draft),
            );
        })?;
    results.push(r);

    // (2) WB を離した後の最終品質（コールド: 毎回キャッシュを捨てる / ウォーム: WB を変えながら）。
    let mut s = ctx.settings();
    let r = preview_bench("cli_preview_wb_release_final")
        .target(WB_RELEASE_TARGET)
        .develop_settings(&s)
        .run_cold(clear, || {
            s.white_balance = next_wb(&mut k);
            keep_err(
                &mut err,
                render_preview(&engine, gpu, source, &s, RenderQuality::Final),
            );
        })?;
    results.push(r);
    let mut s = ctx.settings();
    let r = preview_bench("cli_preview_wb_release_final")
        .target(WB_RELEASE_TARGET)
        .develop_settings(&s)
        .run_warm(|| {
            s.white_balance = next_wb(&mut k);
            keep_err(
                &mut err,
                render_preview(&engine, gpu, source, &s, RenderQuality::Final),
            );
        })?;
    results.push(r);

    // ブラシで 1 本描いて離したとき（設定にブラシの部品があるときだけ）。足したストロークだけを塗り足す。
    if let Some((i, j)) = first_brush(&ctx.settings) {
        let mut s = ctx.settings();
        render_preview(&engine, gpu, source, &s, RenderQuality::Final)?;
        let original = match &s.masks[i].components[j].shape {
            MaskShape::Brush { strokes } => strokes.len(),
            _ => 0,
        };
        let mut added = 0usize;
        let r = preview_bench("cli_preview_brush_stroke")
            .target(STAGE_C_TARGET)
            .develop_settings(&s)
            .note(
                "stroke_points",
                sample_stroke(0.0, 1, 0.0).points.len().to_string(),
            )
            .run_warm(|| {
                if let MaskShape::Brush { strokes } = &mut s.masks[i].components[j].shape {
                    if added >= MAX_ADDED_STROKES {
                        strokes.truncate(original);
                        added = 0;
                    }
                    let y = 0.05 + 0.6 * (added % 13) as f32 / 12.0;
                    strokes.push(sample_stroke(y, 1, 0.0));
                    added += 1;
                }
                keep_err(
                    &mut err,
                    render_preview(&engine, gpu, source, &s, RenderQuality::Final),
                );
            })?;
        results.push(r);
    }
    match err {
        Some(e) => Err(e),
        None => Ok(results),
    }
}

/// 書き出しを 1 回行う（内訳を記録する）。使った側（GPU / CPU）を返す。
#[allow(clippy::too_many_arguments)]
fn export_once(
    t: &mut PhaseTimer,
    input: &BenchInput,
    worker: &mut Worker,
    engine: &Engine,
    gpu: Option<&GpuRenderer>,
    settings: &DevelopSettings,
    export: &ExportSettings,
    dest: &Path,
    protected: &ProtectedFiles,
) -> CliResult<&'static str> {
    let start = Instant::now();
    let (source, _) = input.load(worker)?;
    if input.file.is_some() {
        t.record("decode", start.elapsed());
    }
    let start = Instant::now();
    let outcome = genzo_gpu::export_with_fallback(
        gpu,
        engine,
        &source,
        settings,
        export,
        &ExportOptions::default(),
        &NeverCancel,
    )?;
    let backend = match outcome.backend {
        ExportBackend::Gpu => "gpu",
        ExportBackend::Cpu { .. } => "cpu",
    };
    let image = outcome.image;
    let (w, h) = (image.width, image.height);
    let pixels = match image.pixels {
        ExportPixels::Rgb8(v) => DynRgbImage::Rgb8(RgbImage8::from_raw(w, h, v)?),
        ExportPixels::Rgb16(v) => DynRgbImage::Rgb16(RgbImage16::from_raw(w, h, v)?),
    };
    t.record("render", start.elapsed());
    let start = Instant::now();
    let exif = ExifData::from_photo_metadata(&source_metadata(&source));
    let bytes = genzo_media::encode_export(&pixels, export, Some(&exif))?;
    t.record("encode", start.elapsed());
    let start = Instant::now();
    let path = dest.join(format!("bench.{}", export.format.extension()));
    genzo_media::write_atomically(&path, ConflictPolicy::Overwrite, protected, |f| {
        f.write_all(&bytes)
            .map_err(|e| genzo_media::MediaError::Io {
                path: Some(path.clone()),
                source: e,
            })
    })?;
    t.record("write", start.elapsed());
    Ok(backend)
}

/// 入力の撮影情報（RAW 以外は既定の値。計測では Exif の中身は時間にほとんど影響しない）。
fn source_metadata(source: &PhotoSource) -> genzo_model::PhotoMetadata {
    match &source.image {
        SourceImage::Raw(r) => r.metadata.clone(),
        SourceImage::Working { .. } => genzo_model::PhotoMetadata::default(),
    }
}

fn bench_export(ctx: &Ctx, input: &BenchInput, worker: &mut Worker) -> CliResult<Vec<BenchResult>> {
    let engine = ctx.engine()?;
    let gpu = ctx.gpu.as_ref();
    let settings = ctx.settings();
    let export = ExportSettings {
        on_conflict: ConflictPolicy::Overwrite,
        ..ExportSettings::default()
    };
    let dest = tempfile::tempdir().map_err(|e| CliError::io(std::env::temp_dir(), e))?;
    let mut protected = ProtectedFiles::new();
    if let Some((p, _)) = &input.file {
        protected.insert(p);
    }
    let mut err: Option<CliError> = None;
    let mut backends: Vec<&'static str> = Vec::new();
    let r = base("cli_export_jpeg", false, ctx)
        .target(EXPORT_TARGET)
        .develop_settings(&settings)
        .note("format", "jpeg_q90_srgb")
        .run_cold_phased(
            || clear_all(&engine, gpu),
            |t| match export_once(
                t,
                input,
                worker,
                &engine,
                gpu,
                &settings,
                &export,
                dest.path(),
                &protected,
            ) {
                Ok(b) => backends.push(b),
                Err(e) => keep_err(&mut err, Err(e)),
            },
        )?;
    if let Some(e) = err {
        return Err(e);
    }
    let mut r = r;
    // GPU から CPU 版に切り替えた回があれば、処理した側を記録に残す（比べる相手を分けるため）。
    if backends.contains(&"cpu") && ctx.gpu.is_some() {
        r.notes
            .insert("backend".to_owned(), "gpu_with_cpu_fallback".to_owned());
    }
    Ok(vec![r])
}

/// `bench`。
pub fn run(g: &GlobalArgs, out: Output, a: BenchArgs) -> CliResult<Status> {
    // 設定の誤りは、ワーカーを起こして入力を展開する前に知らせる。
    let mut settings = bench_settings_with(&a.sensor, &a.color, &a.detail)?;
    let profiles = apply_profile_and_masks(&a, &mut settings)?;
    let mut worker = Worker::spawn()?;
    let input = BenchInput::prepare(&a, &mut worker)?;
    out.progress("入力を展開しています…");
    let (source, decoder) = input.load(&mut worker)?;
    let size = source.image.sensor_size();
    let gpu = gpu_renderer(g.gpu, out);
    if let Some(r) = &gpu {
        r.compile_all()?;
    }
    let mut env = EnvironmentInfo::detect().with_library("genzo", env!("CARGO_PKG_VERSION"));
    if let Some(r) = &gpu {
        let s = r.context().summary();
        env = env.with_gpu(
            s.name.clone(),
            Some(format!("{} {}", s.driver, s.driver_info).trim().to_owned()),
        );
        if s.is_software {
            out.warn("GPU はソフトウェアの実装です。時間は性能の判断に使えません");
        }
    }
    if let Some(d) = &decoder {
        env = env.with_library("raw_decoder", d.clone());
    }
    if a.target == BenchTarget::Preview {
        let p = preview_size(size);
        if p.0.max(p.1) < PREVIEW_LONG_EDGE {
            out.warn(format!(
                "入力（{} × {}）の長辺が {PREVIEW_LONG_EDGE} 未満のため、プレビューは {} × {} になり、1.8 節の解像度（2560 × 1707）ではありません",
                size.0, size.1, p.0, p.1
            ));
        }
    }
    let ctx = Ctx {
        runs: a.runs,
        env,
        input: input.input.clone(),
        how: input.how,
        size,
        is_raw: matches!(source.image, SourceImage::Raw(_)),
        gpu,
        settings,
        profiles,
    };
    out.progress(format!(
        "計測しています（{}、{} × {}、{}）…",
        match a.target {
            BenchTarget::Preview => "プレビュー",
            BenchTarget::Export => "書き出し",
        },
        size.0,
        size.1,
        if ctx.gpu.is_some() { "GPU" } else { "CPU" }
    ));
    let results = match a.target {
        BenchTarget::Preview => bench_preview(&ctx, &source)?,
        BenchTarget::Export => bench_export(&ctx, &input, &mut worker)?,
    };
    worker.shutdown(out);
    let recorder = match &a.out {
        Some(d) => BenchRecorder::new(d.clone()),
        None => BenchRecorder::from_env(),
    };
    let mut reports: Vec<BenchReport> = Vec::new();
    for r in &results {
        reports.push(recorder.record(r)?);
    }
    let regressed = regression_fails(&reports);
    if reports
        .iter()
        .any(|r| r.regressed() && !r.result.meets_iteration_rule)
    {
        out.warn(
            "回数が 1.8 節の規則に足りない計測で悪化を検出しましたが、終了コードには反映しません（--runs を省くと規則の回数で測ります）",
        );
    }
    if out.json {
        let items: Vec<serde_json::Value> = reports
            .iter()
            .map(|r| {
                json!({
                    "name": r.result.name,
                    "path": r.path,
                    "passed": r.passed(),
                    "target_met": r.result.target_met(),
                    "regressed": r.regressed(),
                    "result": json_value(&r.result),
                    "comparison": r.comparison,
                })
            })
            .collect();
        out.print_json(&json!({ "reports": items, "regressed": regressed }));
    } else {
        for r in &reports {
            out.line(r.to_string());
            out.line("");
        }
        out.line(
            "※ 時間は呼び出しから結果が戻るまでで、入力から画面への表示までではありません（05 の 1.8 節）",
        );
    }
    Ok(if regressed {
        Status::Partial
    } else {
        Status::Success
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_follow_the_requirements() {
        assert_eq!(STAGE_C_TARGET.as_millis(), 33);
        // 15fps = 66.67ms。
        assert_eq!(WB_DRAG_TARGET.as_micros(), 66_666);
        assert_eq!(WB_RELEASE_TARGET.as_millis(), 500);
        assert_eq!(EXPORT_TARGET.as_secs(), 3);
        assert_eq!(PREVIEW_LONG_EDGE, genzo_pipeline::DEFAULT_PREVIEW_LONG_EDGE);
    }

    /// 計測の設定に、`develop set` と同じオプションの指定を当てる。範囲の外の値はエラー。
    #[test]
    fn bench_settings_take_the_develop_options() {
        let none = bench_settings_with(
            &SensorArgs::default(),
            &ColorArgs::default(),
            &DetailArgs::default(),
        )
        .unwrap();
        assert_eq!(none, bench_settings());
        let sensor = SensorArgs {
            nr_luminance: Some(20.0),
            ..SensorArgs::default()
        };
        let color = ColorArgs {
            grading_balance: Some(-40.0),
            hsl_saturation: vec![(genzo_model::HueBand::Blue, -50.0)],
            ..ColorArgs::default()
        };
        let detail = DetailArgs {
            clarity: Some(30.0),
            sharpen_amount: Some(60.0),
            grain_amount: Some(25.0),
            ..DetailArgs::default()
        };
        let s = bench_settings_with(&sensor, &color, &detail).unwrap();
        assert_eq!(s.detail.noise_reduction.luminance, 20.0);
        assert_eq!(s.color.grading.balance, -40.0);
        assert_eq!(s.color.mixer.blue.saturation, -50.0);
        assert_eq!(s.presence.clarity, 30.0);
        assert_eq!(s.detail.sharpening.amount, 60.0);
        assert_eq!(s.effects.grain.amount, 25.0);
        // 基準の設定（露光量など）は保つ。
        assert_eq!(s.exposure_ev, bench_settings().exposure_ev);
        assert_eq!(s.tone_curve, bench_settings().tone_curve);
        let bad = DetailArgs {
            dehaze: Some(150.0),
            ..DetailArgs::default()
        };
        assert!(bench_settings_with(&SensorArgs::default(), &ColorArgs::default(), &bad).is_err());
    }

    /// `--sample-masks`・`--masks`・`--dcp`・`--cube` を計測の設定に当てる。
    #[test]
    fn bench_settings_take_masks_and_profiles() {
        use clap::Parser;
        let parse = |args: &[&str]| match crate::args::Cli::try_parse_from(args).unwrap().command {
            crate::args::Command::Bench(a) => a,
            _ => panic!(),
        };
        // 見本のマスク: 3 つ（ブラシは画像の幅を 6 回なぞる 966 点）。検証を通る。
        let a = parse(&["genzo", "bench", "preview", "--sample-masks"]);
        let mut s = bench_settings();
        assert!(apply_profile_and_masks(&a, &mut s).unwrap().is_none());
        assert_eq!(s.masks.len(), 3);
        assert_eq!(first_brush(&s), Some((2, 0)));
        let MaskShape::Brush { strokes } = &s.masks[2].components[0].shape else {
            panic!()
        };
        assert_eq!(strokes[0].points.len(), 6 * 161);
        assert_eq!(sample_stroke(0.3, 1, 0.0).points.len(), 161);
        s.validate().unwrap();
        // --masks: マスクの配列、または masks を持つ現像設定（develop get の出力）。
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("masks.json");
        let settings = DevelopSettings {
            masks: sample_masks()[..1].to_vec(),
            ..DevelopSettings::default()
        };
        std::fs::write(&file, serde_json::to_vec(&settings).unwrap()).unwrap();
        let a = parse(&[
            "genzo",
            "bench",
            "preview",
            "--masks",
            file.to_str().unwrap(),
        ]);
        let mut s = bench_settings();
        apply_profile_and_masks(&a, &mut s).unwrap();
        assert_eq!(s.masks, settings.masks);
        assert_eq!(first_brush(&s), None);
        std::fs::write(&file, serde_json::to_vec(&settings.masks).unwrap()).unwrap();
        let mut s = bench_settings();
        apply_profile_and_masks(&a, &mut s).unwrap();
        assert_eq!(s.masks.len(), 1);
        std::fs::write(&file, b"42").unwrap();
        assert!(apply_profile_and_masks(&a, &mut bench_settings()).is_err());
        assert!(
            crate::args::Cli::try_parse_from([
                "genzo",
                "bench",
                "preview",
                "--masks",
                "x.json",
                "--sample-masks"
            ])
            .is_err()
        );
        // --cube（取り込まずに使う）: 設定の profile.look がファイルの ID を指し、エンジンに渡すものができる。
        let cube = dir.path().join("film.cube");
        std::fs::write(
            &cube,
            "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
        )
        .unwrap();
        let a = parse(&[
            "genzo",
            "bench",
            "export",
            "--cube",
            cube.to_str().unwrap(),
            "--look-amount",
            "40",
        ]);
        let mut s = bench_settings();
        assert!(apply_profile_and_masks(&a, &mut s).unwrap().is_some());
        assert_eq!(s.profile.look.as_deref(), Some("film"));
        assert_eq!(s.profile.look_amount, 40.0);
        // 種類の違うファイルは使い方の誤り。
        let a = parse(&["genzo", "bench", "export", "--dcp", cube.to_str().unwrap()]);
        assert!(apply_profile_and_masks(&a, &mut bench_settings()).is_err());
    }

    #[test]
    fn synthetic_raw_has_the_requested_size() {
        let raw = synthetic_raw(64, 48).unwrap();
        assert_eq!((raw.width, raw.height), (64, 48));
        raw.validate().unwrap();
    }

    #[test]
    fn preview_size_follows_the_long_edge_rule() {
        // α7 IV（7008 × 4672）は 1.8 節の 2560 × 1707（3:2）になる。
        assert_eq!(preview_size(DEFAULT_SYNTHETIC_SIZE), (2560, 1707));
        assert_eq!(preview_size((4672, 7008)), (1707, 2560), "縦位置");
        assert_eq!(preview_size((640, 426)), (640, 426), "拡大しない");
        assert_eq!(preview_size((2560, 1707)), (2560, 1707));
    }

    fn report(samples_ms: &[u64], previous_p95: Option<f64>) -> BenchReport {
        let samples: Vec<Duration> = samples_ms
            .iter()
            .map(|&v| Duration::from_millis(v))
            .collect();
        let result = Bench::batch("x")
            .environment(EnvironmentInfo::detect())
            .from_samples(genzo_testkit::bench::Condition::Cold, &samples, &[])
            .unwrap();
        let comparison = previous_p95.map(|p| {
            let mut previous = result.clone();
            previous.stats.p95_ms = p;
            genzo_testkit::bench::compare_results(
                &previous,
                &result,
                &genzo_testkit::bench::RegressionPolicy::default(),
            )
        });
        BenchReport {
            path: PathBuf::from("x.json"),
            result,
            comparison,
        }
    }

    #[test]
    fn only_regressions_with_enough_runs_fail() {
        // 5 回（一括処理の規則）で前回の 95 パーセンタイル 10 ms → 今回 50 ms は悪化で失敗。
        let enough = report(&[50, 50, 50, 50, 50], Some(10.0));
        assert!(enough.regressed() && enough.result.meets_iteration_rule);
        assert!(regression_fails(std::slice::from_ref(&enough)));
        // 2 回（規則に足りない）の悪化は終了コードにしない。
        let few = report(&[50, 50], Some(10.0));
        assert!(few.regressed() && !few.result.meets_iteration_rule);
        assert!(!regression_fails(std::slice::from_ref(&few)));
        // 悪化していなければ失敗にしない。比べる相手がない場合も同じ。
        assert!(!regression_fails(&[report(&[10; 5], Some(10.0))]));
        assert!(!regression_fails(&[report(&[10; 5], None)]));
        assert!(regression_fails(&[few, enough]));
    }

    #[test]
    fn wb_sweep_wraps_around() {
        let mut k = WB_SWEEP.1 - 10.0;
        let WhiteBalance::Custom { temperature_k, .. } = next_wb(&mut k) else {
            panic!()
        };
        assert_eq!(temperature_k, WB_SWEEP.0);
        let WhiteBalance::Custom { temperature_k, .. } = next_wb(&mut k) else {
            panic!()
        };
        assert_eq!(temperature_k, WB_SWEEP.0 + WB_SWEEP.2);
    }
}
