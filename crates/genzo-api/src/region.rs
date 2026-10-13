//! 表示範囲の描画（ルーペの等倍表示 PRV-03・拡大とパン LIB-02。implementation_status の No.52 の (a)）。
//!
//! - [`Core::render_region`] は、現像中の写真（[`Core::open_develop`] で開いたもの）の表示範囲を、今の
//!   現像設定（保存待ちの変更を含む）で描き、画面用の 8bit の画素（ステージ 17a の出力）を返す。
//! - 範囲は C（出力座標。切り抜いた後の画像のフル解像度の画素）か G（ジオメトリ補正後の全体。長辺を 1
//!   とした値）で指定する（04 の 2.7 節。[`RegionSpace`]）。G の範囲は C の格子に写し、外側へ丸めて
//!   出力の画像の内側に収める。
//! - 切り抜く前の全体の表示（[`Core::set_develop_view`]）の間は、現像のプレビューと同じく、C の格子を
//!   G の外接矩形の全体にする（出力の寸法は [`crate::FrameInfo::output_width`] と同じ。回転で画像の外に
//!   なった四隅は黒で塗る）。
//! - 描画は genzo-pipeline の `Engine::full_resolution` で作るフル解像度の処理（`Engine::render_region`
//!   と同じ）の `render_region` で行う（座標の変換に処理のジオメトリが要るため、`Engine::render_region`
//!   が内部で作る処理を自分で作って使う）。GPU 版はまだない（CPU 版だけ）。
//! - **P0 の「最新の 1 件だけ」**（04 の 6.1 節）: 対話用のスケジューラ（1 本のスレッド）に、決まった枠で
//!   投入する。新しい要求が来ると、実行中・待っている古い要求は取り消され、その呼び出しは
//!   [`ApiError::Cancelled`] で戻る（[`ApiError::is_cancelled`] が真。UI は誤りとして表示しない）。
//!   [`Core::cancel_region`] で明示的に取り消せる（ルーペを閉じたときなど）。要求の番号の割り当てから、
//!   スケジューラへの投入・取り消しトークンの公開までは 1 つのロックの下で行うので、呼び出しが重なっても
//!   番号の順と投入の順は一致する（古い番号の要求が、後から投入して新しい要求を取り消すことはない）。
//! - 倍率（[`RegionRequest::scale`]）: 1 は等倍。1 より小さければ B3（リニア）を面積平均で縮小してから
//!   17a を適用する。1 より大きければ、等倍で描いた画面用の値を最近傍で拡大する（画素の境界をそのまま
//!   見せるため）。
//! - 返す画素は呼び出し側（ビューポート）に直接渡す（04 の 1.5 節。JSON のコマンドの応答には情報だけを
//!   入れ、画素は [`Core::region_frame`] で取る）。
//! - **ビフォー**（DEV-31。[`Core::render_before_region`]）: ビフォー / アフターの表示で、ビフォーの設定
//!   （`before` の `effective_settings`。切り抜きはアフターと同じなので、同じ範囲の要求でそろう）で同じように
//!   描く。アフターと同時に描けるよう、状態（最後の結果・取り消し）と「最新の 1 件だけ」の枠はアフターと別。
//!   結果は [`Core::before_region_frame`] で取る（[`RegionInfo::before`] が真）。
//!
//! メモリ: フル解像度の範囲の B3（1 画素 12 バイト）と、等倍以上では 17a の出力（同じく 12 バイト）を
//! 一時的に持つ。そのため範囲と結果の画素数に上限（[`MAX_REGION_SOURCE_PIXELS`]・
//! [`MAX_REGION_OUTPUT_PIXELS`]）を置く。上限を超える範囲（縮小した全体の表示など）は、現像の
//! プレビュー（[`Core::develop_frame`]）を使う。

use std::sync::Arc;

use genzo_jobs::{CancellationToken, JobError, JobOptions, Priority, SupersedePolicy};
use genzo_pipeline::finish::output::{Dither, quantize_u8};
use genzo_pipeline::{FullResolutionJob, ImageTile, Roi, downscale_area};

use crate::core::{Core, Inner, RegionRequestStage};
use crate::error::ApiError;
use crate::render::display_target;
use crate::types::{RegionFrame, RegionInfo, RegionRequest, RegionSpace};

/// 1 回の描画で描く範囲（フル解像度）の画素数の上限。
///
/// **仮置き**: 16 Mi 画素（4096 × 4096 相当）。範囲の B3 と画面の出力（1 画素あわせて 24 バイト）で、
/// 上限のとき約 384 MiB を一時的に使う。5K の画面の等倍表示（5120 × 2880 ≈ 14.7 M 画素）が収まる大きさ
/// として置いた。PoC-1（UI の方式）の実機の計測で見直す。
pub const MAX_REGION_SOURCE_PIXELS: u64 = 16 * 1024 * 1024;

/// 1 回の描画で返す画素数の上限（**仮置き**: [`MAX_REGION_SOURCE_PIXELS`] と同じ。拡大して返す場合の
/// 大きさの上限）。
pub const MAX_REGION_OUTPUT_PIXELS: u64 = 16 * 1024 * 1024;

/// 倍率の上限（**仮置き**: 16 倍。1 画素を 16 × 16 で見せれば画素の確認に十分なため）。
pub const MAX_REGION_SCALE: f64 = 16.0;

/// 対話用のスケジューラの「最新の 1 件だけ」の枠。
const REGION_SLOT: &str = "develop-region";

/// ビフォーの表示範囲の描画の枠（アフターと別。DEV-31）。
const BEFORE_REGION_SLOT: &str = "develop-region-before";

/// 表示範囲を描く対象（アフター = 今の設定、ビフォー = ビフォーの設定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegionTarget {
    After,
    Before,
}

/// 表示範囲の描画の状態（[`crate::develop::DevelopShared`] が持つ）。
#[derive(Default)]
pub(crate) struct RegionState {
    /// 最後に割り当てた要求の番号。
    last_request: u64,
    /// 実行中・待っている要求の取り消しトークン（[`Core::cancel_region`]）。
    pending: Option<(u64, CancellationToken)>,
    /// 最後に描けた結果（[`Core::region_frame`]）。
    last: Option<RegionFrame>,
}

impl Core {
    /// 現像中の写真の表示範囲を描く（ルーペの等倍表示 PRV-03・拡大とパン LIB-02。`region` の doc）。
    ///
    /// 描き終わるまで待つ（Tauri からは `spawn_blocking` で呼ぶ）。新しい要求か [`Core::cancel_region`]
    /// で取り消されたら [`ApiError::Cancelled`]。写真を開いていなければ [`ApiError::NoDevelopSession`]、
    /// 範囲・倍率が不正なら [`ApiError::InvalidArgument`]。
    pub fn render_region(&self, request: &RegionRequest) -> Result<RegionFrame, ApiError> {
        self.render_region_for(request, RegionTarget::After)
    }

    /// ビフォー / アフターの表示（DEV-31）の、ビフォーの表示範囲を描く（`region` の doc）。
    ///
    /// [`Core::render_region`] と同じだが、ビフォーの設定で描き、状態と「最新の 1 件だけ」の枠はアフターと別
    /// （ビフォーとアフターの要求は互いに取り消さない）。結果は [`Core::before_region_frame`]。ビフォーを
    /// 設定していなければ [`ApiError::InvalidArgument`]。
    pub fn render_before_region(&self, request: &RegionRequest) -> Result<RegionFrame, ApiError> {
        self.render_region_for(request, RegionTarget::Before)
    }

    fn render_region_for(
        &self,
        request: &RegionRequest,
        target: RegionTarget,
    ) -> Result<RegionFrame, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        validate(request)?;
        let crate::develop::SessionSnapshot {
            source,
            settings,
            generation,
            uncropped,
            before_settings,
            ..
        } = crate::develop::session_snapshot(inner, request.variant_id)?;
        let settings = match target {
            RegionTarget::After => settings,
            RegionTarget::Before => before_settings.ok_or_else(crate::before::no_before)?,
        };
        let before = target == RegionTarget::Before;
        // テスト用の口（`CoreHooks::on_region_request`）はアフターの要求だけで呼ぶ。
        let hook = inner.on_region_request.as_ref().filter(|_| !before);
        let region_state = if before {
            &inner.develop.region_before
        } else {
            &inner.develop.region
        };
        let slot = if before {
            BEFORE_REGION_SLOT
        } else {
            REGION_SLOT
        };
        let req = *request;
        // 要求の番号の割り当て・スケジューラへの投入・取り消しトークンの公開を、1 つの区間（`region` の
        // ロックを持ったまま）にする。別々にすると、先に番号を取った古い要求が、止まっている間に追い越した
        // 新しい要求の後で投入して、新しい要求（実行中・待ち）を取り消す（最後に描くのが古い範囲になる）。
        // `pending` の公開も、投入の順と食い違わない。ほかのロックは、スケジューラの内部のロックだけ（順序は
        // `region` → スケジューラ。スケジューラは、持っている間にここへ戻らない）。
        let (seq, handle) = {
            let mut st = match region_state.try_lock() {
                Some(guard) => guard,
                None => {
                    // テスト用の口（`CoreHooks::on_region_request`）。待つ前に、ロックを持たずに呼ぶ。
                    if let Some(hook) = hook {
                        hook(RegionRequestStage::Waiting);
                    }
                    region_state.lock()
                }
            };
            st.last_request += 1;
            let seq = st.last_request;
            // テスト用の口。区間の中（ロックを持ったまま）で呼ぶ。
            if let Some(hook) = hook {
                hook(RegionRequestStage::Numbered(seq));
            }
            let weak = Arc::downgrade(inner);
            let handle = inner.interactive.submit_latest_with(
                slot,
                JobOptions::new(Priority::P0)
                    .label("表示範囲の描画")
                    .supersede_policy(SupersedePolicy::CancelRunning),
                move |ctx| {
                    let Some(inner) = weak.upgrade() else {
                        return Err(JobError::Cancelled);
                    };
                    // テスト用の口（スケジューラのスレッドで呼ぶ。`region` のロックは持たない）。
                    if let Some(hook) = inner.on_region_request.as_ref().filter(|_| !before) {
                        hook(RegionRequestStage::Running(seq));
                    }
                    let token = ctx.token().clone();
                    let control = move || token.is_cancelled();
                    // 切り抜く前の全体の表示（Core::set_develop_view）なら、出力（C の格子）は G の外接矩形の
                    // 全体（現像のプレビューと同じ。FrameInfo::output_width と同じ格子）。
                    let job = match inner
                        .engine
                        .full_resolution_with(&source, &settings, uncropped)
                    {
                        Ok(j) => j,
                        Err(e) => return Ok(Err(ApiError::from(e))),
                    };
                    Ok(render(
                        &inner, &job, &req, generation, seq, before, &control,
                    ))
                },
            );
            if let Some(hook) = hook {
                hook(RegionRequestStage::Submitted(seq));
            }
            st.pending = Some((seq, handle.token().clone()));
            (seq, handle)
        };
        let outcome = handle.wait();
        let frame = {
            let mut st = region_state.lock();
            if st.pending.as_ref().is_some_and(|(s, _)| *s == seq) {
                st.pending = None;
            }
            let frame = match outcome {
                Ok(Ok(frame)) => frame,
                Ok(Err(e)) if e.is_cancelled() => return Err(ApiError::Cancelled),
                Ok(Err(e)) => return Err(e),
                Err(e) if e.is_cancellation() => return Err(ApiError::Cancelled),
                Err(e) => return Err(e.into()),
            };
            if st
                .last
                .as_ref()
                .is_none_or(|l| l.info.request < frame.info.request)
            {
                st.last = Some(frame.clone());
            }
            frame
        };
        Ok(frame)
    }

    /// 実行中・待っている表示範囲の描画を取り消す（ビフォーの表示範囲の描画も。その
    /// [`Core::render_region`]・[`Core::render_before_region`] は [`ApiError::Cancelled`] で戻る）。取り消した
    /// ものがあれば `true`。
    pub fn cancel_region(&self) -> Result<bool, ApiError> {
        self.inner.check_open()?;
        let mut cancelled = false;
        for state in [
            &self.inner.develop.region,
            &self.inner.develop.region_before,
        ] {
            let pending = state.lock().pending.take();
            if let Some((_, token)) = pending {
                token.cancel();
                cancelled = true;
            }
        }
        Ok(cancelled)
    }

    /// 最後に描けた表示範囲（なければ `None`）。
    pub fn region_frame(&self) -> Option<RegionFrame> {
        self.inner.develop.region.lock().last.clone()
    }

    /// 最後に描けたビフォーの表示範囲（[`Core::render_before_region`]。なければ `None`）。
    pub fn before_region_frame(&self) -> Option<RegionFrame> {
        self.inner.develop.region_before.lock().last.clone()
    }
}

/// 要求の値を確かめる（範囲の幅・高さは正、値は有限、倍率は 0 より大きく [`MAX_REGION_SCALE`] 以下）。
fn validate(r: &RegionRequest) -> Result<(), ApiError> {
    let finite = [r.x, r.y, r.width, r.height, r.scale]
        .iter()
        .all(|v| v.is_finite());
    if !finite {
        return Err(ApiError::InvalidArgument(
            "表示範囲の値に数でないもの（NaN・無限大）があります".to_owned(),
        ));
    }
    if r.width <= 0.0 || r.height <= 0.0 {
        return Err(ApiError::InvalidArgument(format!(
            "表示範囲の幅と高さは正の値にしてください（{} × {}）",
            r.width, r.height
        )));
    }
    if !(r.scale > 0.0 && r.scale <= MAX_REGION_SCALE) {
        return Err(ApiError::InvalidArgument(format!(
            "倍率は 0 より大きく {MAX_REGION_SCALE} 以下にしてください（{}）",
            r.scale
        )));
    }
    Ok(())
}

/// 要求の範囲を、C の格子（フル解像度）の画素の範囲にする（外側へ丸め、出力の画像の内側に収める）。
pub(crate) fn region_roi(job: &FullResolutionJob, r: &RegionRequest) -> Result<Roi, ApiError> {
    let (x0, y0, x1, y1) = match r.space {
        RegionSpace::Output => (r.x, r.y, r.x + r.width, r.y + r.height),
        RegionSpace::Geometry => {
            let geometry = job.geometry();
            let long = geometry.frames().g_long_side_px();
            let (ax, ay) = geometry.g_px_to_c(r.x * long, r.y * long);
            let (bx, by) = geometry.g_px_to_c((r.x + r.width) * long, (r.y + r.height) * long);
            (ax.min(bx), ay.min(by), ax.max(bx), ay.max(by))
        }
    };
    let (fw, fh) = job.output_size();
    let clamp = |v: f64, max: u32| v.clamp(0.0, f64::from(max));
    let (x0, x1) = (clamp(x0.floor(), fw), clamp(x1.ceil(), fw));
    let (y0, y1) = (clamp(y0.floor(), fh), clamp(y1.ceil(), fh));
    if x1 <= x0 || y1 <= y0 {
        return Err(ApiError::InvalidArgument(format!(
            "表示範囲が画像（{fw} × {fh}）と重なりません"
        )));
    }
    // 0〜出力の寸法（u32）に収めたので、変換で値は失われない。
    let roi = Roi::new(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32);
    if roi.pixel_count() > MAX_REGION_SOURCE_PIXELS {
        return Err(ApiError::InvalidArgument(format!(
            "表示範囲が大きすぎます（{} × {}。上限 {MAX_REGION_SOURCE_PIXELS} 画素。縮小した全体の表示には現像のプレビューを使ってください）",
            roi.width, roi.height
        )));
    }
    Ok(roi)
}

/// 返す画像の寸法（範囲の寸法 × 倍率を四捨五入。1 以上）。
fn scaled_size(roi: Roi, scale: f64) -> Result<(u32, u32), ApiError> {
    let side = |n: u32| (f64::from(n) * scale).round().max(1.0);
    let (w, h) = (side(roi.width), side(roi.height));
    if w * h > MAX_REGION_OUTPUT_PIXELS as f64 {
        return Err(ApiError::InvalidArgument(format!(
            "返す画像が大きすぎます（{w} × {h}。上限 {MAX_REGION_OUTPUT_PIXELS} 画素）"
        )));
    }
    Ok((w as u32, h as u32))
}

/// 範囲を描いて、画面用の 8bit の画素にする。
fn render(
    inner: &Inner,
    job: &FullResolutionJob,
    r: &RegionRequest,
    generation: u64,
    request: u64,
    before: bool,
    control: &(dyn Fn() -> bool + Sync),
) -> Result<RegionFrame, ApiError> {
    let roi = region_roi(job, r)?;
    let (width, height) = scaled_size(roi, r.scale)?;
    let (target, assumed_srgb) = display_target(inner);
    let tile_size = job.tile_size();
    let (rgb8, tiles) = if (width, height) == (roi.width, roi.height) {
        // 等倍（または丸めると同じ寸法）。
        let out = job.render_region(roi, tile_size, Some(&target), &control)?;
        let o = out
            .output
            .ok_or_else(|| ApiError::Internal("画面の出力がない".to_owned()))?;
        (quantize_u8(&o.image, (0, 0), Dither::None)?, out.tiles)
    } else if width < roi.width || height < roi.height {
        // 縮小: B3（リニア）を面積平均で縮小してから 17a を適用する。
        let out = job.render_region(roi, tile_size, None, &control)?;
        let small = downscale_area(&out.b3.image, width.min(roi.width), height.min(roi.height))?;
        let tile = ImageTile::from_image(Roi::full(small.width(), small.height()), small)?;
        let (o, _) = job.apply_output(&target, &tile)?;
        let q = quantize_u8(&o.image, (0, 0), Dither::None)?;
        let (qw, qh) = o.image.dimensions();
        (nearest(&q, (qw, qh), (width, height)), out.tiles)
    } else {
        // 拡大: 等倍で描いた画面用の値を最近傍で拡大する。
        let out = job.render_region(roi, tile_size, Some(&target), &control)?;
        let o = out
            .output
            .ok_or_else(|| ApiError::Internal("画面の出力がない".to_owned()))?;
        let q = quantize_u8(&o.image, (0, 0), Dither::None)?;
        (
            nearest(&q, (roi.width, roi.height), (width, height)),
            out.tiles,
        )
    };
    let (full_width, full_height) = job.output_size();
    let warnings = crate::render::warning_texts(job.warnings(), job.unimplemented());
    Ok(RegionFrame {
        info: RegionInfo {
            variant_id: r.variant_id,
            generation,
            request,
            x: roi.x as u32,
            y: roi.y as u32,
            region_width: roi.width,
            region_height: roi.height,
            full_width,
            full_height,
            scale: r.scale,
            width,
            height,
            tiles,
            display_assumed_srgb: assumed_srgb,
            warnings,
            before,
        },
        display_rgb8: Arc::new(rgb8),
    })
}

/// 8bit の RGB を最近傍で `to` の寸法にする（同じ寸法ならそのまま）。
fn nearest(src: &[u8], from: (u32, u32), to: (u32, u32)) -> Vec<u8> {
    if from == to {
        return src.to_vec();
    }
    let (fw, fh) = (from.0 as usize, from.1 as usize);
    let (tw, th) = (to.0 as usize, to.1 as usize);
    let mut out = vec![0u8; tw * th * 3];
    for y in 0..th {
        let sy = (y * fh / th).min(fh - 1);
        for x in 0..tw {
            let sx = (x * fw / tw).min(fw - 1);
            let s = (sy * fw + sx) * 3;
            let d = (y * tw + x) * 3;
            out[d..d + 3].copy_from_slice(&src[s..s + 3]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use genzo_model::VariantId;

    fn req(x: f64, y: f64, w: f64, h: f64, scale: f64) -> RegionRequest {
        RegionRequest {
            variant_id: VariantId::new(1),
            space: RegionSpace::Output,
            x,
            y,
            width: w,
            height: h,
            scale,
        }
    }

    #[test]
    fn requests_are_validated() {
        assert!(validate(&req(0.0, 0.0, 10.0, 10.0, 1.0)).is_ok());
        assert!(validate(&req(0.0, 0.0, 10.0, 10.0, MAX_REGION_SCALE)).is_ok());
        for bad in [
            req(f64::NAN, 0.0, 10.0, 10.0, 1.0),
            req(0.0, 0.0, 0.0, 10.0, 1.0),
            req(0.0, 0.0, 10.0, -1.0, 1.0),
            req(0.0, 0.0, 10.0, 10.0, 0.0),
            req(0.0, 0.0, 10.0, 10.0, MAX_REGION_SCALE * 2.0),
            req(0.0, 0.0, 10.0, f64::INFINITY, 1.0),
        ] {
            assert!(
                matches!(validate(&bad), Err(ApiError::InvalidArgument(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn scaled_sizes_round_and_are_at_least_one() {
        let roi = Roi::new(0, 0, 100, 51);
        assert_eq!(scaled_size(roi, 1.0).unwrap(), (100, 51));
        assert_eq!(scaled_size(roi, 0.5).unwrap(), (50, 26));
        assert_eq!(scaled_size(roi, 0.001).unwrap(), (1, 1));
        assert_eq!(scaled_size(roi, 2.0).unwrap(), (200, 102));
        let huge = Roi::new(0, 0, 4096, 4096);
        assert!(scaled_size(huge, 1.0).is_ok());
        assert!(scaled_size(huge, 1.5).is_err());
    }

    #[test]
    fn nearest_neighbour_repeats_pixels() {
        // 2 × 1 → 4 × 2。
        let src = [1, 2, 3, 4, 5, 6];
        let out = nearest(&src, (2, 1), (4, 2));
        assert_eq!(
            out,
            vec![
                1, 2, 3, 1, 2, 3, 4, 5, 6, 4, 5, 6, //
                1, 2, 3, 1, 2, 3, 4, 5, 6, 4, 5, 6,
            ]
        );
        assert_eq!(nearest(&src, (2, 1), (2, 1)), src.to_vec());
    }
}
