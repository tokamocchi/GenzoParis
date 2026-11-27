//! 複数の写真への一括の自動補正（DEV-32 の拡張。implementation_status の「v1 の残り」No.27）。
//!
//! 現像の画面の「自動」（[`Core::auto_tone`]）は開いている 1 枚の値を返すだけで、適用は UI・CLI が行う。
//! ここでは、選んだ写真を 1 枚ずつ展開して同じ方式（genzo-pipeline の `engine::auto_tone`）で値を求め、設定に
//! 入れて保存する（P3 のジョブ。[`Core::auto_tone_variants`]）。
//!
//! - **1 枚ごとに保存する**（1 枚 1 トランザクション。履歴に「自動補正」を 1 件）。途中で取り消しても、
//!   それまでに保存した写真はそのまま残る（1 枚ずつ Undo で戻せる）。まとめて 1 つのトランザクションにしないのは、
//!   展開に時間がかかり、その間カタログのロックを持ち続けられないため。
//! - 変えるのは 1 枚のときと同じ 6 つの値だけ（露光量・コントラスト・ハイライト・シャドウ・白レベル・黒レベル）。
//!   求めた値が今の値と同じ写真は保存しない（履歴を増やさない。`unchanged` に数える）。
//! - 動画は飛ばす（`skipped`）。見つからない・読めない写真は `failed` に理由を入れて、次の写真に進む。
//! - 現像中の写真が含まれていれば、展開済みの入力と今の設定（保存待ちの変更を含む）から求め、保存した設定で
//!   描き直す（`develop` の `apply_to_variants_in`）。
//! - 保存した写真のサムネイル・プレビューは、設定の保存と同じ経路で作り直しが予約される。

use std::sync::Arc;

use genzo_model::{AssetKind, VariantId};
use genzo_worker::Lane;

use crate::core::{Core, Inner};
use crate::develop::{HISTORY_LABEL_AUTO_TONE, apply_to_variants_in, session_source};
use crate::error::ApiError;
use crate::jobs::{JobCtx, spawn_job};
use crate::previews::{load_source, variant_file};
use crate::types::{AutoTone, AutoToneBatchReport, JobKind, JobResult, VariantIssue};

/// 1 度に指定できる写真の数の上限（**仮置き**: 10,000。ジョブの記録と結果の大きさを抑えるため）。
pub const MAX_AUTO_TONE_VARIANTS: usize = 10_000;

/// 1 枚の結果。
enum Outcome {
    /// 値を入れて保存した。
    Applied,
    /// 求めた値が今の値と同じだった（保存していない）。
    Unchanged,
    /// 動画（対象外）。
    Skipped,
}

/// 1 枚の自動補正の値を求める（保存はしない）。動画なら `None`。
fn pick_one(
    inner: &Arc<Inner>,
    variant_id: VariantId,
    ctx: &JobCtx<'_>,
) -> Result<Option<AutoTone>, ApiError> {
    let file = inner.with_catalog_api(|c| variant_file(c, variant_id))?;
    if file.kind == AssetKind::Video {
        return Ok(None);
    }
    // 現像中の写真なら、展開済みの入力と今の設定（保存待ちの変更を含む）を使う。
    let session = {
        let state = inner.develop.state.lock();
        state
            .as_ref()
            .filter(|s| s.variant_id == variant_id)
            .map(|s| s.settings.clone())
    };
    let (source, deps) = match session_source(inner, variant_id, file.revision) {
        Some(found) => found,
        None => {
            let loaded = load_source(inner, &file, Lane::Batch, ctx.token())?;
            (loaded.source, loaded.render_deps)
        }
    };
    if ctx.is_cancelled() {
        return Err(ApiError::Cancelled);
    }
    let settings = match session {
        Some(s) => s,
        None => inner.with_catalog(|c| c.develop_settings(variant_id))?,
    };
    // 保存された設定の外部データを、実際に使う値に置き換えて求める（`deps` の doc。プレビューの作り直しと同じ）。
    let effective = crate::deps::with_render_deps(&inner.engine, &settings, &deps);
    let pick = inner
        .engine
        .auto_tone(&source, &effective, inner.engine.config().preview_long_edge)
        .map_err(|e| match e {
            genzo_pipeline::PipelineError::InvalidArgument(m) => ApiError::InvalidArgument(m),
            e => e.into(),
        })?;
    Ok(Some(AutoTone::from_pick(&pick)))
}

/// 1 枚に自動補正を当てて保存する。
fn apply_one(
    inner: &Arc<Inner>,
    variant_id: VariantId,
    ctx: &JobCtx<'_>,
) -> Result<Outcome, ApiError> {
    let Some(auto) = pick_one(inner, variant_id, ctx)? else {
        return Ok(Outcome::Skipped);
    };
    let saved = apply_to_variants_in(inner, &[variant_id], true, |_, current| {
        let mut new = current.clone();
        auto.apply(&mut new);
        Ok((new != *current).then(|| (new, HISTORY_LABEL_AUTO_TONE.to_owned())))
    })?;
    Ok(if saved.is_empty() {
        Outcome::Unchanged
    } else {
        Outcome::Applied
    })
}

fn run(ctx: &JobCtx<'_>, variant_ids: &[VariantId]) -> Result<AutoToneBatchReport, ApiError> {
    let mut report = AutoToneBatchReport::default();
    let total = variant_ids.len() as u64;
    ctx.progress(0, total);
    for (i, &v) in variant_ids.iter().enumerate() {
        if ctx.is_cancelled() {
            report.cancelled = true;
            break;
        }
        match apply_one(ctx.inner, v, ctx) {
            Ok(Outcome::Applied) => report.applied += 1,
            Ok(Outcome::Unchanged) => report.unchanged += 1,
            Ok(Outcome::Skipped) => report.skipped += 1,
            Err(e) if e.is_cancelled() => {
                report.cancelled = true;
                break;
            }
            // カタログを閉じた後は続けられない。
            Err(ApiError::Closed) => return Err(ApiError::Closed),
            Err(e) => report.failed.push(VariantIssue {
                variant_id: v,
                reason: e.to_string(),
            }),
        }
        ctx.progress(i as u64 + 1, total);
    }
    Ok(report)
}

impl Core {
    /// 選んだ写真に、まとめて自動補正（DEV-32）を当てる（P3 のジョブ。ジョブの ID を返す。結果は
    /// [`JobResult::AutoTone`]。モジュールの doc）。`variant_ids` の重なりは 1 つにする（順序は保つ）。
    ///
    /// エラー: 空・[`MAX_AUTO_TONE_VARIANTS`] を超えるなら [`ApiError::InvalidArgument`]。
    pub fn auto_tone_variants(&self, variant_ids: &[VariantId]) -> Result<u64, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let mut seen = std::collections::HashSet::new();
        let ids: Vec<VariantId> = variant_ids
            .iter()
            .copied()
            .filter(|v| seen.insert(*v))
            .collect();
        if ids.is_empty() {
            return Err(ApiError::InvalidArgument(
                "自動補正する写真を選んでください".to_owned(),
            ));
        }
        if ids.len() > MAX_AUTO_TONE_VARIANTS {
            return Err(ApiError::InvalidArgument(format!(
                "一度に自動補正できるのは {MAX_AUTO_TONE_VARIANTS} 枚までです（{} 枚が選ばれています）",
                ids.len()
            )));
        }
        Ok(spawn_job(
            inner,
            JobKind::AutoTone,
            "自動補正",
            move |ctx| run(ctx, &ids).map(JobResult::AutoTone),
        ))
    }
}
