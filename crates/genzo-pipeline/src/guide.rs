//! 段階 B: ガイドの計算（docs/04_architecture.md の 2.2 節・2.7 節。レビュー R-03）。
//!
//! ガイドは、ステージ 11（トーン。ハイライト・シャドウ）がタイル処理で参照する **局所の明るさ**。
//! ステージ 11 から分離し、3 段目（エンジン）が段階 B の結果としてキャッシュできる形にする。
//!
//! # 定義（2.7 節）
//!
//! 1. 入力: ステージ 8 の出力（B2、**露光量を適用する前**）の段階 A1 のプレビュー
//!    （[`crate::A1Image`]）。**切り抜く前の全体**を使う。
//! 2. 輝度 `Y = 0.2627·R + 0.6780·G + 0.0593·B`（BT.2020 の係数。[`crate::finish::luminance`]）。
//! 3. G の座標で長辺 [`GUIDE_LONG_EDGE`] px（G の長辺がそれより短ければ G の長辺の画素数）に縮小する。
//!    (a) S の向きのまま、ガイドと同じ密度（S の 1 画素あたりのガイドの画素の数）まで面積平均で
//!    縮小する（[`crate::resample::AreaWeights`]。リニアの値の平均）。入力より大きくはしない。
//!    (b) ガイドの画素ごとに、G の座標 → S → (a) の格子の座標をたどり、双線形で引く
//!    （[`crate::interp::sample_bilinear_gray`]。端の外は鏡映）。回転と向きをここで反映する。
//!    回転のない場合は (a) の画素の中心をそのまま引くので、補間は恒等になる。
//! 4. `log2(max(Y, 2^−16))`（[`crate::sanitize::safe_log2`]）。
//! 5. エッジを保つぼかし: 自身をガイドにした **ガイデッドフィルタ**（He, Sun, Tang, "Guided Image
//!    Filtering", ECCV 2010 の式。[`guided_filter`]）。半径は G の長辺に対する割合
//!    （[`GUIDE_RADIUS_FRACTION`]）をガイドの画素に直したもの。**仮実装**: ハローの出方を
//!    ラプラシアンピラミッドなどと比べて PoC-5 で決める。
//!
//! 露光量は値を定数倍する処理なので、対数の領域では定数を足すだけになる。ガイドは露光量を
//! 含めずに作り、ステージ 11 が使うときに EV を足す（露光量を変えても作り直さない）。
//!
//! # キャッシュのキー（2.7 節の表）
//!
//! | 含める | どこで |
//! |---|---|
//! | 段階 A1 のキー（WB・レンズ補正・カメラプロファイル・ノイズ軽減）、回転、歪曲補正、処理バージョン | `DevelopSettings::hash_for_phase(Phase::B)`（[`GuideSpec::develop_hash`]）と `CacheKey` の `process_version`・`render_deps_hash` |
//! | ガイドのアルゴリズムのパラメータ | 処理バージョンで決まる（[`GuideParams::for_version`]）ので、処理バージョンに含まれる |
//! | 入力（段階 A1 のプレビュー）の寸法・品質、ガイドの寸法 | `CacheKey` の `size`（ガイドの寸法）・`quality`（A1 の品質）。A1 の寸法は長辺 2560 で決まる |
//! | 保存形式 | `CacheFormat { kind: BGuide, color_space: Log2Luminance, format_version: GUIDE_FORMAT_VERSION }` |
//!
//! 露光量・切り抜き・ハイライトやシャドウなどのスライダーは含めない（[`GuideSpec::from_settings`] は
//! 処理バージョンと角度補正だけを読む）。
//!
//! # GPU 版と一致させるための決めごと（2.3 節）
//!
//! - ぼかしの合計は、窓の中を **番号の小さい順に f32 で足し**、窓の大きさで割る（累積和は使わない）。
//!   横方向、縦方向の順。端の外は鏡映。行ごとの計算は分割によらないので、並列化してもスレッド数で
//!   結果は変わらない。
//! - 座標の変換の係数は f64 で合成して f32 に丸め、画素ごとに f32 で計算する。

use genzo_model::{DevelopSettings, Phase};
use rayon::prelude::*;

use crate::border::mirror_index;
use crate::contract::ColorContract;
use crate::coords::{CoordinateFrames, SensorGrid};
use crate::error::{PipelineError, Result};
use crate::finish::geometry::GeometryPlan;
use crate::finish::luminance;
use crate::image::{GrayImage, RgbImage};
use crate::interp::sample_bilinear_gray;
use crate::resample::AreaWeights;
use crate::sanitize::safe_log2;
use crate::sensor::A1Image;
use crate::stage::{Guide, StageContext};
use crate::version::ProcessVersion;

/// ガイドの長辺（px。2.2 節・2.7 節の「長辺 512px 程度」）。仮置き: 解像度を上げるとハローは
/// 減るが段階 B が重くなる。PoC-5 で決める。
pub const GUIDE_LONG_EDGE: u32 = 512;

/// ガイデッドフィルタの半径（G の長辺に対する割合）。仮置き: 長辺 512px のガイドで 15px。
/// 局所の明るさとして「物の大きさ」程度の範囲（画像の数 % ）を見る値として置いた。PoC-5 で決める。
pub const GUIDE_RADIUS_FRACTION: f64 = 0.03;

/// ガイデッドフィルタの ε（log2 の値の分散。段²）。仮置き: 標準偏差 0.5 段より小さい変化は
/// ぼかし（質感として残す）、それより大きい明暗の境目は保つ（ハローを抑える）目安。PoC-5 で決める。
pub const GUIDE_EPSILON: f32 = 0.25;

/// ガイドのキャッシュの保存形式のバージョン（`CacheFormat::format_version`）。
pub const GUIDE_FORMAT_VERSION: u32 = 1;

/// ガイドのアルゴリズムのパラメータ（処理バージョンで決まる）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuideParams {
    /// 長辺（px）。
    pub long_edge: u32,
    /// ガイデッドフィルタの半径（G の長辺に対する割合）。
    pub radius_fraction: f64,
    /// ガイデッドフィルタの ε。
    pub epsilon: f32,
}

impl GuideParams {
    /// 処理バージョンのパラメータ。
    pub const fn for_version(version: ProcessVersion) -> Self {
        match version {
            ProcessVersion::V1 => Self {
                long_edge: GUIDE_LONG_EDGE,
                radius_fraction: GUIDE_RADIUS_FRACTION,
                epsilon: GUIDE_EPSILON,
            },
        }
    }

    /// 長辺 `long_side` px のガイドでの半径（px。1 以上）。
    pub fn radius_px(&self, long_side: u32) -> u32 {
        ((self.radius_fraction * f64::from(long_side)).round() as u32).max(1)
    }
}

/// ガイドを作るのに必要な設定（2.7 節の表の「含める」項目のうち、現像設定から読むもの）。
///
/// 露光量・切り抜き・トーンのスライダーは持たない（変えてもガイドは同じ）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuideSpec {
    /// 処理バージョン。
    pub process_version: ProcessVersion,
    /// 角度補正（度）。G を決める。
    pub rotation_deg: f64,
    /// アルゴリズムのパラメータ。
    pub params: GuideParams,
}

impl GuideSpec {
    /// 現像設定から作る。処理バージョンは丸める前の値で確かめる（未知の版はエラー）。
    pub fn from_settings(settings: &DevelopSettings) -> Result<Self> {
        let process_version = ProcessVersion::from_u32(settings.process_version)?;
        let s = settings.normalized();
        let rotation_deg = f64::from(s.geometry.angle_deg);
        Ok(Self {
            process_version,
            rotation_deg: if rotation_deg == 0.0 {
                0.0
            } else {
                rotation_deg
            },
            params: GuideParams::for_version(process_version),
        })
    }

    /// キャッシュのキーの `develop_hash`（段階 B のハッシュ。モジュールの doc の表）。
    pub fn develop_hash(settings: &DevelopSettings) -> [u8; 32] {
        settings.hash_for_phase(Phase::B)
    }
}

/// G の外接矩形（`frames` の回転を含む）に対するガイドの寸法。
///
/// 長辺は `min(long_edge, round(G の長辺))`、短辺は `max(1, round(短辺 × 長辺の倍率))`。
pub fn guide_size(frames: &CoordinateFrames, long_edge: u32) -> (u32, u32) {
    let (gw, gh) = frames.g_size_px();
    let long = gw.max(gh);
    let target = (long.round() as u32).clamp(1, long_edge.max(1));
    let k = f64::from(target) / long;
    let len = |v: f64| ((v * k).round() as u32).clamp(1, target);
    (len(gw), len(gh))
}

/// 段階 A1 のプレビューからガイドを作る（モジュールの doc の定義）。
pub fn compute_guide(a1: &A1Image, spec: &GuideSpec) -> Result<Guide> {
    compute_guide_from(&a1.image, a1.sensor_grid(), a1.frames(), spec)
}

/// B2 の画像（S の向き。`grid` は画像が覆う S の範囲）からガイドを作る。`frames` の回転は
/// 使わず、`spec.rotation_deg` にする。
pub fn compute_guide_from(
    image: &RgbImage,
    grid: SensorGrid,
    frames: CoordinateFrames,
    spec: &GuideSpec,
) -> Result<Guide> {
    if image.contract() != ColorContract::B2Working {
        return Err(PipelineError::ContractMismatch {
            stage: "guide",
            expected: ColorContract::B2Working,
            found: image.contract(),
        });
    }
    let grid = SensorGrid::new(grid.width, grid.height, grid.extent)?;
    if image.dimensions() != (grid.width, grid.height) {
        return Err(PipelineError::InvalidArgument(format!(
            "ガイドの入力の寸法 {:?} が格子 {}×{} と違います",
            image.dimensions(),
            grid.width,
            grid.height
        )));
    }
    let frames = frames.with_rotation(spec.rotation_deg)?;
    match spec.process_version {
        ProcessVersion::V1 => {}
    }
    let (out_w, out_h) = guide_size(&frames, spec.params.long_edge);
    let (g_w, g_h) = frames.g_size_px();

    // 2. 輝度。
    let y_img = GrayImage::from_vec(
        grid.width,
        grid.height,
        image.data().par_iter().map(|&c| luminance(c)).collect(),
    )?;

    // 3a. ガイドと同じ密度まで面積平均で縮小する（入力より大きくはしない）。
    let dg = f64::from(out_w.max(out_h)) / g_w.max(g_h);
    let iw = ((grid.extent.0 * dg).round() as u32).clamp(1, grid.width);
    let ih = ((grid.extent.1 * dg).round() as u32).clamp(1, grid.height);
    let small = downscale_gray(&y_img, iw, ih)?;

    // 3b. G の座標 → S → 縮小した格子。係数は f64 で合成して f32 に丸める。
    let g_to_s = frames.g_to_sensor_affine();
    let (kx, ky) = (g_w / f64::from(out_w), g_h / f64::from(out_h));
    let (sx, sy) = (f64::from(iw) / grid.extent.0, f64::from(ih) / grid.extent.1);
    let a: [[f32; 3]; 2] = [
        [
            (g_to_s[0][0] * kx * sx) as f32,
            (g_to_s[0][1] * ky * sx) as f32,
            (g_to_s[0][2] * sx) as f32,
        ],
        [
            (g_to_s[1][0] * kx * sy) as f32,
            (g_to_s[1][1] * ky * sy) as f32,
            (g_to_s[1][2] * sy) as f32,
        ],
    ];
    let mut values = vec![0.0f32; out_w as usize * out_h as usize];
    values
        .par_chunks_mut(out_w as usize)
        .enumerate()
        .for_each(|(q, row)| {
            let v = q as f32 + 0.5;
            for (p, out) in row.iter_mut().enumerate() {
                let u = p as f32 + 0.5;
                let x = a[0][0] * u + a[0][1] * v + a[0][2];
                let y = a[1][0] * u + a[1][1] * v + a[1][2];
                // 4. 対数（下限 2^−16）。
                *out = safe_log2(sample_bilinear_gray(&small, x, y));
            }
        });
    let log_img = GrayImage::from_vec(out_w, out_h, values)?;

    // 5. エッジを保つぼかし。
    let radius = spec.params.radius_px(out_w.max(out_h));
    let mut blurred = guided_filter(&log_img, radius, spec.params.epsilon)?;
    blurred.sanitize();
    Ok(Guide::new(blurred, frames, spec.process_version))
}

/// 1 チャンネルの画像を面積平均で縮小する（[`crate::resample::downscale_area`] と同じ定義・同じ
/// 足し方。横、縦の順）。同じ寸法なら複製する。
fn downscale_gray(image: &GrayImage, width: u32, height: u32) -> Result<GrayImage> {
    if (width, height) == (image.width(), image.height()) {
        return Ok(image.clone());
    }
    let wx = AreaWeights::new(image.width(), width)?;
    let wy = AreaWeights::new(image.height(), height)?;
    let (sw, dw) = (image.width() as usize, width as usize);
    let mut tmp = vec![0.0f32; image.height() as usize * dw];
    tmp.par_chunks_mut(dw)
        .zip(image.data().par_chunks(sw))
        .for_each(|(out, src)| {
            for (o, dst) in out.iter_mut().enumerate() {
                let mut acc = 0.0f32;
                for &(i, w) in wx.taps(o as u32) {
                    acc += w * src[i as usize];
                }
                *dst = acc;
            }
        });
    let mut data = vec![0.0f32; height as usize * dw];
    data.par_chunks_mut(dw).enumerate().for_each(|(oy, out)| {
        let taps = wy.taps(oy as u32);
        for (x, dst) in out.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            for &(j, w) in taps {
                acc += w * tmp[j as usize * dw + x];
            }
            *dst = acc;
        }
    });
    GrayImage::from_vec(width, height, data)
}

/// 窓の平均（一辺 `2r + 1` の正方形。横方向、縦方向の順に、窓の中を番号の小さい順に足して
/// `2r + 1` で割る。端の外は鏡映）。
pub fn box_mean(image: &GrayImage, radius: u32) -> Result<GrayImage> {
    let (w, h) = (image.width(), image.height());
    let r = i64::from(radius);
    let n = (2 * radius + 1) as f32;
    let src = image.data();
    let mut tmp = vec![0.0f32; src.len()];
    tmp.par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(y, out)| {
            let row = &src[y * w as usize..(y + 1) * w as usize];
            for (x, dst) in out.iter_mut().enumerate() {
                let mut acc = 0.0f32;
                for k in -r..=r {
                    acc += row[mirror_index(x as i64 + k, w) as usize];
                }
                *dst = acc / n;
            }
        });
    let mut data = vec![0.0f32; src.len()];
    data.par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(y, out)| {
            for (x, dst) in out.iter_mut().enumerate() {
                let mut acc = 0.0f32;
                for k in -r..=r {
                    let yy = mirror_index(y as i64 + k, h) as usize;
                    acc += tmp[yy * w as usize + x];
                }
                *dst = acc / n;
            }
        });
    GrayImage::from_vec(w, h, data)
}

/// 自身をガイドにしたガイデッドフィルタ（He ら 2010 の式。エッジを保つぼかし）。
///
/// ```text
/// m  = box(I)、 mm = box(I·I)
/// var = max(mm − m·m, 0)、 a = var / (var + ε)、 b = m − a·m
/// q  = box(a)·I + box(b)
/// ```
///
/// 平らなところ（分散が ε より十分小さい）では `a ≒ 0` で窓の平均になり、明暗の境目（分散が
/// ε より十分大きい）では `a ≒ 1` で元の値を保つ。
pub fn guided_filter(image: &GrayImage, radius: u32, epsilon: f32) -> Result<GrayImage> {
    if !(epsilon.is_finite() && epsilon > 0.0) {
        return Err(PipelineError::InvalidArgument(format!(
            "ガイデッドフィルタの ε は正の有限の値が必要です（{epsilon}）"
        )));
    }
    let (w, h) = (image.width(), image.height());
    let i = image.data();
    let mean = box_mean(image, radius)?;
    let sq = GrayImage::from_vec(w, h, i.par_iter().map(|&v| v * v).collect())?;
    let mean_sq = box_mean(&sq, radius)?;
    let (a, b): (Vec<f32>, Vec<f32>) = mean
        .data()
        .par_iter()
        .zip(mean_sq.data().par_iter())
        .map(|(&m, &mm)| {
            let var = (mm - m * m).max(0.0);
            let a = var / (var + epsilon);
            (a, m - a * m)
        })
        .unzip();
    let mean_a = box_mean(&GrayImage::from_vec(w, h, a)?, radius)?;
    let mean_b = box_mean(&GrayImage::from_vec(w, h, b)?, radius)?;
    let q = i
        .par_iter()
        .zip(mean_a.data().par_iter().zip(mean_b.data().par_iter()))
        .map(|(&v, (&ma, &mb))| ma * v + mb)
        .collect();
    GrayImage::from_vec(w, h, q)
}

/// C の画素 → ガイドの連続座標の対応（ステージ 11 が使う。GPU 版にも同じ係数を渡す）。
///
/// C の画素 (i, j) は G の格子の画素 `(x0 + i, y0 + j)`（[`GeometryPlan::crop_lattice`]）。ガイドの
/// 連続座標は `u = ((x0 + i) as f32 + 0.5) × sx`、`v = ((y0 + j) as f32 + 0.5) × sy`
/// （`sx = ガイドの幅 / (G の外接矩形の幅 × G の格子の密度)`）。ガイドは [`sample_bilinear_gray`]
/// （端の外は鏡映）で引く。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuideLookup {
    /// G の格子での C の原点（切り抜きの左上）。
    pub origin: (i64, i64),
    /// G の格子の 1 画素あたりのガイドの画素の数（f32）。
    pub scale: (f32, f32),
}

impl GuideLookup {
    /// ジオメトリと、ガイドの寸法から作る。
    pub fn new(plan: &GeometryPlan, guide_width: u32, guide_height: u32) -> Self {
        let (gw, gh) = plan.frames().g_size_px();
        let (dx, dy) = plan.density();
        let c = plan.crop_lattice();
        Self {
            origin: (i64::from(c.x), i64::from(c.y)),
            scale: (
                (f64::from(guide_width) / (gw * dx)) as f32,
                (f64::from(guide_height) / (gh * dy)) as f32,
            ),
        }
    }

    /// C の画素 (x, y)（格子の番号）のガイドの連続座標。
    #[inline]
    pub fn coords(&self, x: i64, y: i64) -> (f32, f32) {
        (
            ((self.origin.0 + x) as f32 + 0.5) * self.scale.0,
            ((self.origin.1 + y) as f32 + 0.5) * self.scale.1,
        )
    }
}

/// 文脈のガイドが使えるか確かめて返す（ガイドがない、回転・寸法・向き・処理バージョンが文脈と
/// 違う場合は [`PipelineError::GuideMismatch`]）。
pub fn checked_guide<'a>(ctx: &StageContext<'a>, stage: &'static str) -> Result<&'a Guide> {
    let guide = ctx.guide.ok_or(PipelineError::GuideMismatch {
        stage,
        reason: "文脈にガイドがない（段階 B を先に計算する）",
    })?;
    if guide.frames != ctx.frames {
        return Err(PipelineError::GuideMismatch {
            stage,
            reason: "ガイドを作ったときの座標系（寸法・向き・回転）が文脈と違う",
        });
    }
    if guide.process_version != ctx.process_version {
        return Err(PipelineError::GuideMismatch {
            stage,
            reason: "ガイドを作った処理バージョンが文脈と違う",
        });
    }
    Ok(guide)
}

#[cfg(test)]
mod tests {
    use genzo_model::Orientation;

    use super::*;

    fn frames(w: u32, h: u32, o: Orientation) -> CoordinateFrames {
        CoordinateFrames::new(w, h, o).unwrap()
    }

    fn spec(deg: f64) -> GuideSpec {
        GuideSpec {
            process_version: ProcessVersion::V1,
            rotation_deg: deg,
            params: GuideParams::for_version(ProcessVersion::V1),
        }
    }

    #[test]
    fn guide_sizes() {
        assert_eq!(
            guide_size(&frames(7008, 4672, Orientation::Normal), 512),
            (512, 341)
        );
        assert_eq!(
            guide_size(&frames(7008, 4672, Orientation::Rotate90Cw), 512),
            (341, 512)
        );
        // G の長辺が 512 より短ければ、その画素数。
        assert_eq!(
            guide_size(&frames(64, 48, Orientation::Normal), 512),
            (64, 48)
        );
        // 回転すると外接矩形が大きくなる。
        let r = frames(600, 400, Orientation::Normal)
            .with_rotation(10.0)
            .unwrap();
        let (w, h) = guide_size(&r, 512);
        assert_eq!(w, 512);
        let (gw, gh) = r.g_size_px();
        assert_eq!(h, (gh / gw * 512.0).round() as u32);
        assert_eq!(
            GuideParams::for_version(ProcessVersion::V1).radius_px(512),
            15
        );
        assert_eq!(
            GuideParams::for_version(ProcessVersion::V1).radius_px(10),
            1
        );
    }

    #[test]
    fn box_mean_is_exact_on_constants_and_mirrors_edges() {
        let c = GrayImage::filled(7, 5, -3.25).unwrap();
        let m = box_mean(&c, 2).unwrap();
        assert!(m.data().iter().all(|&v| (v + 3.25).abs() < 1e-6));
        // 1 行の 0, 1, 2, 3（半径 1）: 端は鏡映（−1 → 1、4 → 2）。
        let r = GrayImage::from_vec(4, 1, vec![0.0, 1.0, 2.0, 3.0]).unwrap();
        let m = box_mean(&r, 1).unwrap();
        let expect = [2.0 / 3.0, 1.0, 2.0, 7.0 / 3.0];
        for (a, b) in m.data().iter().zip(expect) {
            assert!((a - b).abs() < 1e-6, "{:?}", m.data());
        }
    }

    #[test]
    fn guided_filter_keeps_strong_edges_and_smooths_small_detail() {
        // 左半分 −8 段、右半分 0 段の境目と、右半分に ±0.1 段の細かい模様。
        let img = GrayImage::from_fn(40, 20, |x, y| {
            if x < 20 {
                -8.0
            } else {
                0.1 * if (x + y) % 2 == 0 { 1.0 } else { -1.0 }
            }
        })
        .unwrap();
        let q = guided_filter(&img, 3, 0.25).unwrap();
        // 境目のすぐ両側は元の値に近い（ハローが小さい）。
        assert!((q.pixel(18, 10).unwrap() + 8.0).abs() < 0.5);
        assert!(q.pixel(21, 10).unwrap().abs() < 0.5);
        // 細かい模様はならされる。
        let v = q.pixel(30, 10).unwrap();
        assert!(v.abs() < 0.05, "{v}");
        assert!(guided_filter(&img, 3, 0.0).is_err());
    }

    #[test]
    fn guide_of_a_flat_image_is_flat() {
        let img = RgbImage::filled(64, 48, ColorContract::B2Working, [0.18; 3]).unwrap();
        let f = frames(64, 48, Orientation::Normal);
        let g = compute_guide_from(&img, SensorGrid::full_resolution(&f), f, &spec(0.0)).unwrap();
        assert_eq!((g.image.width(), g.image.height()), (64, 48));
        let l = 0.18f32.log2();
        assert!(g.image.data().iter().all(|v| (v - l).abs() < 1e-4));
        // 回転しても平らなまま（外接矩形の外は鏡映で埋まる）。
        let g = compute_guide_from(&img, SensorGrid::full_resolution(&f), f, &spec(12.0)).unwrap();
        assert!(g.image.data().iter().all(|v| (v - l).abs() < 1e-4));
        assert_eq!(g.frames.rotation_deg, 12.0);
        // 契約が違う入力はエラー。
        let wrong = RgbImage::filled(64, 48, ColorContract::B3DisplayReferred, [0.18; 3]).unwrap();
        assert!(
            compute_guide_from(&wrong, SensorGrid::full_resolution(&f), f, &spec(0.0)).is_err()
        );
    }

    #[test]
    fn guide_follows_orientation() {
        // 左が暗く右が明るい S の画像を、時計回りに 90 度回転して表示すると、上が暗く下が明るい。
        let img = RgbImage::from_fn(40, 20, ColorContract::B2Working, |x, _| {
            if x < 20 { [0.01; 3] } else { [1.0; 3] }
        })
        .unwrap();
        let f = frames(40, 20, Orientation::Rotate90Cw);
        let g = compute_guide_from(&img, SensorGrid::full_resolution(&f), f, &spec(0.0)).unwrap();
        assert_eq!((g.image.width(), g.image.height()), (20, 40));
        assert!(g.image.pixel(10, 2).unwrap() < -4.0);
        assert!(g.image.pixel(10, 37).unwrap() > -1.0);
    }

    #[test]
    fn spec_reads_only_version_and_rotation() {
        let mut s = DevelopSettings::default();
        let base = GuideSpec::from_settings(&s).unwrap();
        s.exposure_ev = 2.0;
        s.tone.shadows = 50.0;
        s.geometry.crop.left = 0.3;
        assert_eq!(GuideSpec::from_settings(&s).unwrap(), base);
        assert_eq!(
            GuideSpec::develop_hash(&s),
            GuideSpec::develop_hash(&DevelopSettings::default())
        );
        s.geometry.angle_deg = -0.0;
        assert_eq!(GuideSpec::from_settings(&s).unwrap(), base);
        s.geometry.angle_deg = 3.0;
        assert_ne!(GuideSpec::from_settings(&s).unwrap(), base);
        s.process_version = 9;
        assert!(GuideSpec::from_settings(&s).is_err());
    }
}
