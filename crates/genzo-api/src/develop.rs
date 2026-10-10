//! 現像のセッション（DEV-00〜08・DEV-15・DEV-26・DEV-27・DEV-30、SYS-03。04 の 2.2 節・6.2 節）。
//!
//! - [`Core::open_develop`]: 対話用のワーカーで元ファイルを展開し（RAW は `DecodeRaw`、それ以外は
//!   `DecodeImage`）、本体のメモリに保持する（段階 A0）。
//! - [`Core::set_settings`]: 設定の世代を 1 つ進め、プレビューの描画を P0 のスケジューラに投入する。
//!   **最新の 1 件だけを処理する**（genzo-jobs の `submit_latest`。実行中の描画は最後まで行い、
//!   待っている古い要求は捨てる。6.2 節）。結果は [`crate::Event::PreviewUpdated`] で知らせ、
//!   [`Core::develop_frame`] で取る。世代はコアの中の通しの番号で、写真を開き直しても 1 に戻らない。
//!   前のセッションの描画・今のプレビューより古い世代の描画は、プレビューにしない（4.1 節）。
//! - 描画は GPU（使えれば）→ 失敗したら CPU 版（6.3 節）。WB など段階 A1 の項目をドラッグしている間は
//!   簡易（Draft。2 × 2 の簡易処理）、離したら最終品質（Final）で描き直す（2.2 節・PERF-01b）。
//! - 結果は B3（ステージ 16 の出力）・画面の表示用（17a。モニターのプロファイル、なければ sRGB とみなす。
//!   IQ-05）・ヒストグラムとクリッピングの情報（17c。DEV-26）。
//! - 保存（DATA-03・DEV-27）: ドラッグの終了（[`Core::end_drag`]）で履歴に 1 件記録して保存する。
//!   ドラッグでない変更は、最後の操作から [`DEVELOP_SAVE_DELAY`]（1 秒）後にまとめて保存する（自動保存の
//!   スレッド）。変更が続いても、最初の未保存の変更から [`DEVELOP_SAVE_MAX_DELAY`] がたてば保存する。
//!   [`Core::close_develop`]・[`Core::close`] で保存待ちの変更を保存する。
//! - Undo / Redo（DEV-27）はカタログの履歴で行うので、再起動の後もできる。
//! - 設定のコピーと複数の variant への一括適用（DEV-30。[`Core::paste_settings`]）、仮想コピーの作成
//!   （LIB-13。[`Core::create_virtual_copy`]）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use genzo_jobs::{JobOptions, Priority, SupersedePolicy};
use genzo_model::{DevelopSettings, Phase, RenderQuality, VariantId};
use genzo_pipeline::finish::output::{Dither, quantize_u8};
use genzo_pipeline::{PhotoSource, PreviewRequest};
use genzo_worker::Lane;
use parking_lot::{Condvar, Mutex, MutexGuard};

use crate::config::{DEVELOP_SAVE_DELAY, DEVELOP_SAVE_MAX_DELAY};
use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::events::{CatalogChange, Event, WarningCode};
use crate::previews::{load_source, schedule_regenerate, variant_file};
use crate::render::display_target;
use crate::types::{
    DevelopFrame, DevelopState, FrameInfo, HistogramData, HistoryItem, SettingGroups, SourceInfo,
};

/// 自動保存に失敗したときに、もう一度試すまでの時間（**仮置き**: 10 秒。ディスクの容量不足などで
/// 失敗し続けるときに、警告を出し続けないため）。
pub const SAVE_RETRY_DELAY: Duration = Duration::from_secs(10);

/// 現像のプレビューのスロット（「最新の 1 件だけ」。genzo-jobs の `SlotKey`）。
const PREVIEW_SLOT: &str = "develop-preview";

/// 設定の一括適用の履歴の名前。
pub const HISTORY_LABEL_PASTE: &str = "設定の貼り付け";
/// Undo / Redo の前に保存待ちの変更を保存したときなどの、変更の名前（変わった項目が分からないとき）。
pub const HISTORY_LABEL_GENERIC: &str = "現像設定の変更";

/// 現像のセッション。
pub(crate) struct Session {
    variant_id: VariantId,
    file_revision: u32,
    source: PhotoSource,
    source_info: SourceInfo,
    /// 実際に使う外部データ（RAW のカメラ行列・デコーダ。`deps` の doc）。設定の `render_deps` は常にこの値
    /// にする（利用者の設定では変えない）。
    render_deps: genzo_model::RenderDeps,
    /// 現在の設定（最新の要求）。
    settings: DevelopSettings,
    /// 最後に保存した設定。
    saved: DevelopSettings,
    /// `saved` の写しの番号（[`DevelopShared::next_save_seq`]。古い写しの保存の結果で上書きしない）。
    saved_seq: u64,
    /// 設定の世代（変えるたびに増える。コアの中で通しの番号で、開き直しても 1 に戻らない）。
    generation: u64,
    /// このセッションを開いたときの世代（これより前の世代の描画は、前のセッションのもの）。
    first_generation: u64,
    /// 最後の変更の時刻（自動保存の起点）。
    last_change: Instant,
    /// 最初の未保存の変更の時刻（自動保存の最大の待ち時間の起点。保存したら `None`）。
    first_unsaved_change: Option<Instant>,
    /// 自動保存に失敗したときの、次に試す時刻（新しい変更があれば消す）。
    retry_after: Option<Instant>,
    /// ドラッグ中か。
    dragging: bool,
    /// ドラッグを始めたときの段階 A1 の設定のハッシュ（WB のドラッグ中は簡易処理にする）。
    drag_base_a1: Option<[u8; 32]>,
    /// 最後に要求した描画の品質。
    requested_quality: RenderQuality,
    /// 最新の描画の結果。
    frame: Option<Arc<DevelopFrame>>,
}

impl Session {
    fn dirty(&self) -> bool {
        self.settings != self.saved
    }
}

/// 現像のセッションの共有の状態（自動保存のスレッドと共有する）。
#[derive(Default)]
pub(crate) struct DevelopShared {
    pub state: Mutex<Option<Session>>,
    pub cv: Condvar,
    pub stop: AtomicBool,
    /// 写真を開く操作を 1 つずつにする（展開の間は state のロックを持たないため）。
    open_lock: Mutex<()>,
    /// 写真を開く要求の通しの番号（新しい要求が来たら、古い要求は展開をやめる。6.1 節の P0）。
    open_seq: AtomicU64,
    /// 展開中の要求の取り消しトークン（新しい要求が取り消す）。
    opening: Mutex<Option<genzo_jobs::CancellationToken>>,
    /// 最後に割り当てた設定の世代（セッションをまたいだ通しの番号。[`DevelopShared::next_generation`]）。
    last_generation: AtomicU64,
    /// 現像中の写真の現像設定をカタログに書く操作を 1 つずつにし、最後に書いた写しを覚える
    /// （[`save_unlocked`] は state のロックの外で書くため。ロックの順序は state → save → カタログ）。
    save: Mutex<SaveRecord>,
    /// 保存する設定の写しの通しの番号（大きいほど新しい）。
    save_seq: AtomicU64,
}

/// 現像中の写真の現像設定を最後に書いた写し（[`DevelopShared::save`]）。
#[derive(Default)]
struct SaveRecord {
    /// 書いた variant。
    variant: Option<VariantId>,
    /// 書いた写しの番号。
    seq: u64,
}

impl SaveRecord {
    /// `seq` より新しい写しを `variant` に書いた後か。
    fn has_newer(&self, variant: VariantId, seq: u64) -> bool {
        self.variant == Some(variant) && self.seq > seq
    }

    fn record(&mut self, variant: VariantId, seq: u64) {
        self.variant = Some(variant);
        self.seq = seq;
    }
}

impl DevelopShared {
    /// 新しい設定の世代。
    ///
    /// 世代はセッションごとに 1 から数え直さない。数え直すと、同じ写真を開き直したときに、前の
    /// セッションで遅れて終わった描画（大きい世代）が新しいセッションのプレビューになり、その後の
    /// 新しい描画が「古い」として捨てられる（6.2 節・4.1 節）。UI がイベントの世代で新旧を比べるときも
    /// 取り違えない。
    fn next_generation(&self) -> u64 {
        self.last_generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// 保存する設定の写しの新しい番号。
    fn next_save_seq(&self) -> u64 {
        self.save_seq.fetch_add(1, Ordering::AcqRel) + 1
    }
}

/// 描画の結果を、開いているセッションのプレビューにしてよいか（4.1 節「古い世代の結果で上書きしない」）。
///
/// - `session`: 開いているセッションの（variant、開いたときの世代、今のプレビューの世代）。
/// - 別の写真の結果、前のセッションの結果（開いたときの世代より前）、今のプレビューより古い世代の結果は
///   使わない。最新の要求より古い世代でも、今のプレビューより新しければ使う（6.2 節: 途中の結果も表示する）。
fn accepts_frame(
    session: (VariantId, u64, Option<u64>),
    frame_variant: VariantId,
    frame_generation: u64,
) -> bool {
    let (variant, first_generation, shown) = session;
    variant == frame_variant
        && frame_generation >= first_generation
        && shown.is_none_or(|g| g <= frame_generation)
}

/// 変わった項目から履歴の名前を作る（DEV-27。「露光量 +0.30」など）。CLI が設定を保存するときの
/// 履歴の名前にも使う（[`Core::paste_settings_with_label`]）。
pub fn describe_change(old: &DevelopSettings, new: &DevelopSettings) -> String {
    let mut parts: Vec<String> = Vec::new();
    if old.white_balance != new.white_balance {
        parts.push("ホワイトバランス".to_owned());
    }
    if old.exposure_ev != new.exposure_ev {
        parts.push(format!("露光量 {:+.2}", new.exposure_ev));
    }
    if old.contrast != new.contrast {
        parts.push(format!("コントラスト {:+.0}", new.contrast));
    }
    let t = (&old.tone, &new.tone);
    if t.0.highlights != t.1.highlights {
        parts.push(format!("ハイライト {:+.0}", t.1.highlights));
    }
    if t.0.shadows != t.1.shadows {
        parts.push(format!("シャドウ {:+.0}", t.1.shadows));
    }
    if t.0.whites != t.1.whites {
        parts.push(format!("白レベル {:+.0}", t.1.whites));
    }
    if t.0.blacks != t.1.blacks {
        parts.push(format!("黒レベル {:+.0}", t.1.blacks));
    }
    if old.tone_curve != new.tone_curve {
        parts.push("トーンカーブ".to_owned());
    }
    if old.color.saturation != new.color.saturation {
        parts.push(format!("彩度 {:+.0}", new.color.saturation));
    }
    if old.color.vibrance != new.color.vibrance {
        parts.push(format!("自然な彩度 {:+.0}", new.color.vibrance));
    }
    if old.color != new.color && !parts.iter().any(|p| p.contains("彩度")) {
        parts.push("色".to_owned());
    }
    if old.geometry != new.geometry {
        parts.push("切り抜き・角度補正".to_owned());
    }
    if old.lens != new.lens {
        parts.push("レンズ補正".to_owned());
    }
    if old.detail != new.detail {
        parts.push("ディテール".to_owned());
    }
    if old.masks != new.masks {
        parts.push("ローカル補正".to_owned());
    }
    if old.process_version != new.process_version || old.render_deps != new.render_deps {
        parts.push("処理バージョン".to_owned());
    }
    match parts.len() {
        0 => HISTORY_LABEL_GENERIC.to_owned(),
        1..=3 => parts.join("・"),
        _ => format!("{}ほか", parts[..3].join("・")),
    }
}

/// 保存待ちの変更を保存する（呼び出し側が state のロックを持つ。写真を開く・一括適用・Undo / Redo・
/// 閉じるなど、カタログの読み書きとまとめて行う操作用）。保存したら `true`。
fn save_locked(inner: &Arc<Inner>, s: &mut Session, label: Option<&str>) -> Result<bool, ApiError> {
    if !s.dirty() {
        return Ok(false);
    }
    let label = label
        .map(str::to_owned)
        .unwrap_or_else(|| describe_change(&s.saved, &s.settings));
    let v = s.variant_id;
    let settings = s.settings.clone();
    let seq = inner.develop.next_save_seq();
    {
        let mut record = inner.develop.save.lock();
        inner.with_catalog(|c| c.save_develop(v, &settings, &label))?;
        record.record(v, seq);
    }
    s.saved = settings;
    s.saved_seq = seq;
    s.first_unsaved_change = None;
    after_develop_saved(inner, &[v]);
    Ok(true)
}

/// 保存待ちの変更を、state のロックを外してから保存する（自動保存・ドラッグの終了・書き出しの前など）。
/// 保存したら `true`。
///
/// 現像のロックを持ったままカタログのロックを待つと、カタログを長く使う処理の間、スライダーの操作・
/// プレビューの描画の受け取り（P0）・UI のプレビューの取得まで止まるため（PERF-13。指摘 F24）。写しを
/// 取ってからロックを外して書き、ロックを取り直して `saved` を更新する。書く前に、より新しい写しが
/// 書かれていれば（ロックを外している間に、一括適用・Undo などが書いた）書かない（保存待ちのまま残り、
/// 次の保存で書く）。
fn save_unlocked(
    inner: &Arc<Inner>,
    state: &mut MutexGuard<'_, Option<Session>>,
    label: Option<&str>,
) -> Result<bool, ApiError> {
    let Some(s) = state.as_mut() else {
        return Ok(false);
    };
    if !s.dirty() {
        return Ok(false);
    }
    let label = label
        .map(str::to_owned)
        .unwrap_or_else(|| describe_change(&s.saved, &s.settings));
    let (v, first) = (s.variant_id, s.first_generation);
    let settings = s.settings.clone();
    let seq = inner.develop.next_save_seq();
    let written = MutexGuard::unlocked(state, || -> Result<bool, ApiError> {
        let mut record = inner.develop.save.lock();
        if record.has_newer(v, seq) {
            return Ok(false);
        }
        inner.with_catalog(|c| c.save_develop(v, &settings, &label))?;
        record.record(v, seq);
        drop(record);
        after_develop_saved(inner, &[v]);
        Ok(true)
    })?;
    if written
        && let Some(s) = state
            .as_mut()
            .filter(|s| s.variant_id == v && s.first_generation == first)
        && seq > s.saved_seq
    {
        s.saved = settings;
        s.saved_seq = seq;
        s.first_unsaved_change = if s.dirty() {
            // 書いている間に変わった分は、これから保存する。
            Some(Instant::now())
        } else {
            None
        };
    }
    Ok(written)
}

/// 現像設定を保存した後: カタログの変更を知らせ、L0 / L1 の作り直しを予約する（PRV-02）。
fn after_develop_saved(inner: &Arc<Inner>, variants: &[VariantId]) {
    inner.events.emit(Event::CatalogChanged {
        change: CatalogChange::Develop,
        variant_ids: variants.to_vec(),
        all: false,
    });
    for &v in variants {
        schedule_regenerate(inner, v);
    }
}

/// 書き出しの前に、保存待ちの変更を保存する（ドラッグ中の値は保存しない）。
pub(crate) fn flush_for_export(inner: &Arc<Inner>) -> Result<(), ApiError> {
    let mut state = inner.develop.state.lock();
    if state.as_ref().is_some_and(|s| !s.dragging) {
        save_unlocked(inner, &mut state, None)?;
    }
    Ok(())
}

/// 削除した variant を開いていれば、保存せずに閉じる。
pub(crate) fn close_if_removed(inner: &Inner, removed: &[VariantId]) {
    let mut state = inner.develop.state.lock();
    if state
        .as_ref()
        .is_some_and(|s| removed.contains(&s.variant_id))
    {
        *state = None;
        inner.develop.cv.notify_all();
    }
}

/// 現像中の写真の展開済みの入力と、実際に使う外部データ（`variant_id` の写真を開いていて、リビジョンが
/// 同じとき）。
pub(crate) fn session_source(
    inner: &Inner,
    variant_id: VariantId,
    revision: u32,
) -> Option<(PhotoSource, genzo_model::RenderDeps)> {
    let state = inner.develop.state.lock();
    state
        .as_ref()
        .filter(|s| s.variant_id == variant_id && s.file_revision == revision)
        .map(|s| (s.source.clone(), s.render_deps.clone()))
}

/// 描画を投入する（「最新の 1 件だけ」）。
fn submit_render(inner: &Arc<Inner>, s: &mut Session, quality: RenderQuality) {
    s.requested_quality = quality;
    let job = RenderJob {
        variant_id: s.variant_id,
        source: s.source.clone(),
        settings: s.settings.clone(),
        generation: s.generation,
        quality,
    };
    let weak = Arc::downgrade(inner);
    let _ = inner.interactive.submit_latest_with(
        PREVIEW_SLOT,
        JobOptions::new(Priority::P0)
            .label("現像のプレビュー")
            .supersede_policy(SupersedePolicy::KeepRunning),
        move |_ctx| {
            let Some(inner) = weak.upgrade() else {
                return Ok(());
            };
            match render_frame(&inner, &job) {
                Ok(frame) => publish_frame(&inner, frame),
                Err(e) if e.is_cancelled() => {}
                Err(e) => inner.events.warn(
                    WarningCode::Render,
                    format!("プレビューを描けませんでした: {e}"),
                    Some(job.variant_id),
                    None,
                ),
            }
            Ok(())
        },
    );
}

/// 描画の要求（セッションの状態の写し）。
struct RenderJob {
    variant_id: VariantId,
    source: PhotoSource,
    settings: DevelopSettings,
    generation: u64,
    quality: RenderQuality,
}

/// プレビューを描く（17a・17c まで）。
fn render_frame(inner: &Inner, job: &RenderJob) -> Result<DevelopFrame, ApiError> {
    let (target, assumed_srgb) = display_target(inner);
    let request = PreviewRequest::new(job.quality, inner.engine.config().preview_long_edge)
        .with_output(target)
        .with_histogram();
    let r = inner.render_preview(&job.source, &job.settings, &request, Some(job.variant_id))?;
    let output = r
        .output
        .ok_or_else(|| ApiError::Internal("画面の出力がない".to_owned()))?;
    let (width, height) = output.dimensions();
    let display_rgb8 = quantize_u8(&output, (0, 0), Dither::None)?;
    let histogram = r
        .histogram
        .as_ref()
        .map(HistogramData::from)
        .unwrap_or_default();
    Ok(DevelopFrame {
        info: FrameInfo {
            variant_id: job.variant_id,
            generation: job.generation,
            quality: r.quality,
            width,
            height,
            backend: r.backend,
            histogram,
            display_assumed_srgb: assumed_srgb,
            warnings: r.warnings,
        },
        b3: Arc::new(r.b3),
        display_rgb8: Arc::new(display_rgb8),
    })
}

/// 描画の結果をセッションに入れて知らせる（古い世代の結果で、新しい結果を置き換えない）。
///
/// イベント（[`Event::PreviewUpdated`]）は、結果を入れたのと同じ錠の中で、待っている側
/// （[`Core::wait_for_frame`]）を起こす前に送る。結果を受け取った側が、そのイベントがまだ届いていない
/// 状態を見ないようにするため（錠の外で送ると、負荷が高いときに順序が入れ替わる）。送り先は無制限の
/// チャンネルで、送るときに develop の錠を取る処理はないので、錠の中で送ってもデッドロックしない。
fn publish_frame(inner: &Inner, frame: DevelopFrame) {
    let info = frame.info.clone();
    let mut state = inner.develop.state.lock();
    let Some(s) = state.as_mut() else {
        return;
    };
    let shown = s.frame.as_ref().map(|f| f.info.generation);
    if !accepts_frame(
        (s.variant_id, s.first_generation, shown),
        info.variant_id,
        info.generation,
    ) {
        return;
    }
    s.frame = Some(Arc::new(frame));
    inner.events.emit(Event::PreviewUpdated {
        variant_id: info.variant_id,
        generation: info.generation,
        quality: info.quality,
        width: info.width,
        height: info.height,
        backend: info.backend,
    });
    inner.develop.cv.notify_all();
}

/// 画面のプロファイルを変えたときなど、現在の設定で描き直す。
pub(crate) fn rerender(inner: &Arc<Inner>) {
    let mut state = inner.develop.state.lock();
    if let Some(s) = state.as_mut() {
        s.generation = inner.develop.next_generation();
        let q = if s.dragging {
            s.requested_quality
        } else {
            RenderQuality::Final
        };
        submit_render(inner, s, q);
    }
}

/// 自動保存のスレッド（DATA-03: ドラッグでない変更は、最後の操作から 1 秒後に保存する）。
pub(crate) fn spawn_saver(inner: &Arc<Inner>) -> Result<JoinHandle<()>, ApiError> {
    let shared = Arc::clone(&inner.develop);
    let weak = Arc::downgrade(inner);
    let delay = inner
        .config
        .develop_save_delay
        .unwrap_or(DEVELOP_SAVE_DELAY);
    let max_delay = inner
        .config
        .develop_save_max_delay
        .unwrap_or(DEVELOP_SAVE_MAX_DELAY)
        .max(delay);
    std::thread::Builder::new()
        .name("genzo-develop-saver".to_owned())
        .spawn(move || saver_loop(&shared, &weak, delay, max_delay))
        .map_err(|e| ApiError::Internal(format!("自動保存のスレッドを作れません: {e}")))
}

/// 自動保存の期限: 最後の変更から `delay` 後か、最初の未保存の変更から `max_delay` 後の早い方（保存に
/// 失敗していれば、次に試す時刻）。
fn save_deadline(s: &Session, delay: Duration, max_delay: Duration) -> Instant {
    s.retry_after.unwrap_or_else(|| {
        let after_pause = s.last_change + delay;
        match s.first_unsaved_change {
            Some(first) => after_pause.min(first + max_delay),
            None => after_pause,
        }
    })
}

fn saver_loop(
    shared: &DevelopShared,
    weak: &std::sync::Weak<Inner>,
    delay: Duration,
    max_delay: Duration,
) {
    let mut state = shared.state.lock();
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        let deadline = state
            .as_ref()
            .filter(|s| s.dirty() && !s.dragging)
            .map(|s| save_deadline(s, delay, max_delay));
        match deadline {
            None => shared.cv.wait(&mut state),
            Some(d) if Instant::now() < d => {
                shared.cv.wait_until(&mut state, d);
            }
            Some(_) => {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                // 書く間は state のロックを外す（save_unlocked）。
                let saved = save_unlocked(&inner, &mut state, None);
                if let Some(s) = state.as_mut() {
                    match saved {
                        Ok(_) => s.retry_after = None,
                        Err(e) => {
                            inner.events.warn(
                                WarningCode::SaveFailed,
                                format!(
                                    "現像設定を保存できませんでした（{SAVE_RETRY_DELAY:?} 後にもう一度試します）: {e}"
                                ),
                                Some(s.variant_id),
                                None,
                            );
                            // 保存待ちのまま残し（close でも保存を試す）、間隔を空けて試し直す。
                            s.retry_after = Some(Instant::now() + SAVE_RETRY_DELAY);
                        }
                    }
                }
            }
        }
    }
}

/// 終了の処理: 保存待ちの変更を保存し、自動保存のスレッドを止める。
pub(crate) fn shutdown(inner: &Arc<Inner>) -> Result<(), ApiError> {
    let mut state = inner.develop.state.lock();
    let r = match state.as_mut() {
        Some(s) => {
            s.dragging = false;
            save_locked(inner, s, None).map(|_| ())
        }
        None => Ok(()),
    };
    *state = None;
    inner.develop.stop.store(true, Ordering::Release);
    inner.develop.cv.notify_all();
    r
}

fn state_of(s: &Session, can_undo: bool, can_redo: bool) -> DevelopState {
    DevelopState {
        variant_id: s.variant_id,
        settings: s.settings.clone(),
        generation: s.generation,
        can_undo: can_undo || s.dirty(),
        can_redo: can_redo && !s.dirty(),
        dirty: s.dirty(),
        dragging: s.dragging,
        source: s.source_info.clone(),
    }
}

impl Core {
    /// 写真を現像のために開く（段階 A0: 対話用のワーカーで展開して、本体のメモリに保持する）。
    ///
    /// 別の写真を開いていれば、保存待ちの変更を保存してから閉じる。最終品質のプレビューの描画を投入する。
    /// 動画は開けない（[`ApiError::InvalidArgument`]）。
    ///
    /// **最新の要求だけを処理する**（04 の 6.1 節の P0）: 写真を次々に開くと、途中の要求は展開を取り消して
    /// [`ApiError::Cancelled`]（[`ApiError::is_cancelled`] が真）で戻る。UI はこの取り消しを誤りとして
    /// 表示しないこと（最後に要求した写真が開く）。
    pub fn open_develop(&self, variant_id: VariantId) -> Result<DevelopState, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let shared = &inner.develop;
        // 最新の要求だけを処理する（6.1 節の P0「古い要求は捨てる」）: 自分の番号を取り、展開中の古い要求を
        // 取り消す（対話用のワーカーは強制終了・再起動される）。
        let my_seq = shared.open_seq.fetch_add(1, Ordering::AcqRel) + 1;
        if let Some(previous) = shared.opening.lock().take() {
            previous.cancel();
        }
        let is_stale = || shared.open_seq.load(Ordering::Acquire) != my_seq;
        let _open = shared.open_lock.lock();
        // 待っている間に新しい要求が来ていれば、展開せずにやめる（新しい要求が処理する）。
        if is_stale() {
            return Err(ApiError::Cancelled);
        }
        // 開いている写真を閉じる（同じ写真なら、そのまま状態を返す）。
        {
            let mut state = inner.develop.state.lock();
            if let Some(s) = state.as_mut() {
                if s.variant_id == variant_id {
                    let (u, r) = self.undo_redo_flags(variant_id)?;
                    return Ok(state_of(s, u, r));
                }
                s.dragging = false;
                save_locked(inner, s, None)?;
                *state = None;
            }
        }
        let file = inner.with_catalog_api(|c| variant_file(c, variant_id))?;
        if file.kind == genzo_model::AssetKind::Video {
            return Err(ApiError::InvalidArgument(
                "動画は現像できません（カタログの管理だけ）".to_owned(),
            ));
        }
        let token = genzo_jobs::CancellationToken::new();
        *shared.opening.lock() = Some(token.clone());
        if is_stale() {
            // 番号を取ってから展開のトークンを置くまでの間に、新しい要求が来た。
            token.cancel();
        }
        let loaded = load_source(inner, &file, Lane::Interactive, &token);
        {
            let mut opening = shared.opening.lock();
            if opening.as_ref().is_some_and(|t| t.same_as(&token)) {
                *opening = None;
            }
        }
        let loaded = loaded?;
        let mut state = inner.develop.state.lock();
        // 展開の間に新しい要求が来ていれば、セッションを置かない。
        if is_stale() {
            return Err(ApiError::Cancelled);
        }
        // 現像設定は、展開の後に state のロックを持ってから読む（展開の間に一括適用（[`Core::paste_settings`]。
        // state のロックを持ってカタログに書く）や削除があっても、古い設定で開かないため）。
        let dstate = inner.with_catalog(|c| c.develop_state(variant_id))?;
        // 外部データ（render_deps）は実際に使う値にする。保存された値が空なら、ここで埋める（メモリの中だけ。
        // 次に保存したときに記録する）。違えば警告する（`deps` の doc）。
        let deps = loaded.render_deps;
        if crate::deps::recorded_deps_differ(&dstate.settings.render_deps, &deps) {
            inner.events.warn(
                WarningCode::RenderDepsChanged,
                crate::deps::deps_changed_message(&dstate.settings.render_deps, &deps),
                Some(variant_id),
                None,
            );
        }
        let settings = crate::deps::with_render_deps(&dstate.settings, &deps);
        let generation = inner.develop.next_generation();
        let mut session = Session {
            variant_id,
            file_revision: loaded.file.revision,
            source: loaded.source,
            source_info: loaded.info,
            render_deps: deps,
            settings: settings.clone(),
            saved: settings,
            saved_seq: inner.develop.next_save_seq(),
            generation,
            first_generation: generation,
            last_change: Instant::now(),
            first_unsaved_change: None,
            retry_after: None,
            dragging: false,
            drag_base_a1: None,
            requested_quality: RenderQuality::Final,
            frame: None,
        };
        submit_render(inner, &mut session, RenderQuality::Final);
        let out = state_of(&session, dstate.can_undo, dstate.can_redo);
        *state = Some(session);
        inner.develop.cv.notify_all();
        Ok(out)
    }

    fn undo_redo_flags(&self, variant_id: VariantId) -> Result<(bool, bool), ApiError> {
        let s = self.inner.with_catalog(|c| c.develop_state(variant_id))?;
        Ok((s.can_undo, s.can_redo))
    }

    /// 現像中の写真の状態（開いていなければ `None`）。
    pub fn develop_state(&self) -> Result<Option<DevelopState>, ApiError> {
        let inner = &self.inner;
        let mut state = inner.develop.state.lock();
        let Some(v) = state.as_ref().map(|s| s.variant_id) else {
            return Ok(None);
        };
        // カタログ（履歴の有無）は、state のロックを外して読む。
        let d = MutexGuard::unlocked(&mut state, || inner.with_catalog(|c| c.develop_state(v)))?;
        Ok(state
            .as_ref()
            .filter(|s| s.variant_id == v)
            .map(|s| state_of(s, d.can_undo, d.can_redo)))
    }

    /// 現像設定を変える（6.2 節）。描画を投入して、すぐに設定の世代を返す（応答を待たずに次の値を
    /// 送ってよい。古い要求は捨てる）。
    ///
    /// - `dragging = true`: スライダーのドラッグ中。保存も履歴の記録もしない（[`Core::end_drag`] で記録）。
    ///   WB など段階 A1 の項目が変わっていれば簡易処理（Draft）で描く。
    /// - `dragging = false`: ドラッグでない変更（数値の入力など）。最後の操作から 1 秒後にまとめて保存する。
    ///   ドラッグ中にこれが来たら、ドラッグを終えたものとして扱う。
    pub fn set_settings(
        &self,
        settings: &DevelopSettings,
        dragging: bool,
    ) -> Result<u64, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        settings
            .validate()
            .map_err(genzo_model::DevelopError::from)?;
        let mut state = inner.develop.state.lock();
        let s = state.as_mut().ok_or(ApiError::NoDevelopSession)?;
        if dragging && !s.dragging {
            s.dragging = true;
            s.drag_base_a1 = Some(s.settings.hash_for_phase(Phase::A1));
        }
        let was_dirty = s.dirty();
        // 外部データ（render_deps）はセッションの値のまま（利用者の設定では変えない。`deps` の doc）。
        s.settings = crate::deps::with_render_deps(settings, &s.render_deps);
        s.generation = inner.develop.next_generation();
        s.last_change = Instant::now();
        s.retry_after = None;
        if !s.dirty() {
            s.first_unsaved_change = None;
        } else if !was_dirty || s.first_unsaved_change.is_none() {
            s.first_unsaved_change = Some(s.last_change);
        }
        // WB など段階 A1 の項目をドラッグしている間は簡易処理（RAW 以外は常に最終品質）。
        let requested = if dragging && s.drag_base_a1 != Some(s.settings.hash_for_phase(Phase::A1))
        {
            RenderQuality::Draft
        } else {
            RenderQuality::Final
        };
        let quality = s.source.effective_quality(requested);
        // ドラッグの終了の知らせがないまま、ドラッグでない変更が来た: ドラッグを終えたものとして保存する。
        let ends_drag = !dragging && s.dragging;
        if ends_drag {
            s.dragging = false;
            s.drag_base_a1 = None;
        }
        submit_render(inner, s, quality);
        let generation = s.generation;
        inner.develop.cv.notify_all();
        if ends_drag {
            save_unlocked(inner, &mut state, None)?;
        }
        Ok(generation)
    }

    /// スライダーのドラッグを終える: 履歴に 1 件記録して保存し（DEV-27・DATA-03）、簡易処理で描いて
    /// いたら最終品質で描き直す（PERF-01b）。
    pub fn end_drag(&self) -> Result<DevelopState, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let mut state = inner.develop.state.lock();
        let s = state.as_mut().ok_or(ApiError::NoDevelopSession)?;
        let was_dragging = std::mem::replace(&mut s.dragging, false);
        s.drag_base_a1 = None;
        let v = s.variant_id;
        if was_dragging {
            if s.requested_quality == RenderQuality::Draft {
                s.generation = inner.develop.next_generation();
                submit_render(inner, s, RenderQuality::Final);
            }
            // 書く間は state のロックを外す（save_unlocked）。
            save_unlocked(inner, &mut state, None)?;
        }
        // カタログ（履歴の有無）も、state のロックを外して読む。
        let d = MutexGuard::unlocked(&mut state, || inner.with_catalog(|c| c.develop_state(v)))?;
        inner.develop.cv.notify_all();
        let s = state
            .as_ref()
            .filter(|s| s.variant_id == v)
            .ok_or(ApiError::NoDevelopSession)?;
        Ok(state_of(s, d.can_undo, d.can_redo))
    }

    /// 保存待ちの変更をすぐに保存する（保存したら `true`）。
    pub fn flush_develop(&self) -> Result<bool, ApiError> {
        let inner = &self.inner;
        let mut state = inner.develop.state.lock();
        if state.as_ref().is_some_and(|s| !s.dragging) {
            save_unlocked(inner, &mut state, None)
        } else {
            Ok(false)
        }
    }

    /// 現像中の写真を閉じる（保存待ちの変更を保存する。ドラッグ中なら終えたものとして記録する）。
    pub fn close_develop(&self) -> Result<(), ApiError> {
        let inner = &self.inner;
        let mut state = inner.develop.state.lock();
        if let Some(s) = state.as_mut() {
            s.dragging = false;
            save_locked(inner, s, None)?;
        }
        *state = None;
        inner.develop.cv.notify_all();
        Ok(())
    }

    /// 現在の設定で、呼び出したスレッドで描く（同期。CLI・テスト用）。結果は
    /// [`Core::develop_frame`] にも入る。
    pub fn render_develop(&self) -> Result<FrameInfo, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let job = {
            let state = inner.develop.state.lock();
            let s = state.as_ref().ok_or(ApiError::NoDevelopSession)?;
            RenderJob {
                variant_id: s.variant_id,
                source: s.source.clone(),
                settings: s.settings.clone(),
                generation: s.generation,
                quality: RenderQuality::Final,
            }
        };
        let frame = render_frame(inner, &job)?;
        let info = frame.info.clone();
        publish_frame(inner, frame);
        Ok(info)
    }

    /// 最新のプレビュー（ピクセルを含む）。まだ描けていなければ `None`。
    pub fn develop_frame(&self) -> Option<DevelopFrame> {
        let state = self.inner.develop.state.lock();
        state.as_ref().and_then(|s| s.frame.as_deref().cloned())
    }

    /// 世代 `generation` 以降のプレビューが描けるまで待つ（`timeout` を過ぎたら、その時点の最新の情報）。
    pub fn wait_for_frame(
        &self,
        generation: u64,
        timeout: Duration,
    ) -> Result<Option<FrameInfo>, ApiError> {
        let shared = &self.inner.develop;
        let deadline = Instant::now() + timeout;
        let mut state = shared.state.lock();
        loop {
            let s = state.as_ref().ok_or(ApiError::NoDevelopSession)?;
            if let Some(f) = s.frame.as_ref()
                && f.info.generation >= generation
            {
                return Ok(Some(f.info.clone()));
            }
            if shared.cv.wait_until(&mut state, deadline).timed_out() {
                return Ok(state
                    .as_ref()
                    .and_then(|s| s.frame.as_ref().map(|f| f.info.clone())));
            }
        }
    }

    /// 1 つ前の履歴に戻す（DEV-27。再起動の後もできる）。保存待ちの変更があれば、先に保存してから戻す。
    /// 戻せなければ `None`。
    pub fn undo(&self, variant_id: VariantId) -> Result<Option<DevelopSettings>, ApiError> {
        self.step_history(variant_id, false)
    }

    /// 1 つ先の履歴に進める（DEV-27）。進められなければ `None`。
    pub fn redo(&self, variant_id: VariantId) -> Result<Option<DevelopSettings>, ApiError> {
        self.step_history(variant_id, true)
    }

    fn step_history(
        &self,
        variant_id: VariantId,
        forward: bool,
    ) -> Result<Option<DevelopSettings>, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let mut state = inner.develop.state.lock();
        let session = state.as_mut().filter(|s| s.variant_id == variant_id);
        if let Some(s) = session {
            s.dragging = false;
            s.drag_base_a1 = None;
            save_locked(inner, s, None)?;
        }
        let seq = inner.develop.next_save_seq();
        let result = {
            let mut record = inner.develop.save.lock();
            let result = inner.with_catalog(|c| {
                if forward {
                    c.redo_develop(variant_id)
                } else {
                    c.undo_develop(variant_id)
                }
            })?;
            if result.is_some() {
                record.record(variant_id, seq);
            }
            result
        };
        if let Some(settings) = &result {
            if let Some(s) = state.as_mut().filter(|s| s.variant_id == variant_id) {
                let settings = crate::deps::with_render_deps(settings, &s.render_deps);
                s.settings = settings.clone();
                s.saved = settings;
                s.saved_seq = seq;
                s.generation = inner.develop.next_generation();
                submit_render(inner, s, RenderQuality::Final);
            }
            drop(state);
            after_develop_saved(inner, &[variant_id]);
        }
        Ok(result)
    }

    /// 履歴（古い順。DEV-27）。
    pub fn history(&self, variant_id: VariantId) -> Result<Vec<HistoryItem>, ApiError> {
        let entries = self.inner.with_catalog(|c| c.history(variant_id))?;
        Ok(entries
            .into_iter()
            .map(|h| HistoryItem {
                id: h.id,
                created_at: h.created_at,
                label: h.label,
                is_current: h.is_current,
            })
            .collect())
    }

    /// variant の現像設定（設定のコピー。DEV-30）。現像中の写真なら、保存待ちの変更を含む現在の設定。
    pub fn develop_settings(&self, variant_id: VariantId) -> Result<DevelopSettings, ApiError> {
        {
            let state = self.inner.develop.state.lock();
            if let Some(s) = state.as_ref().filter(|s| s.variant_id == variant_id) {
                return Ok(s.settings.clone());
            }
        }
        self.inner.with_catalog(|c| c.develop_settings(variant_id))
    }

    /// 設定を複数の variant にまとめて適用する（DEV-30。1 つのトランザクションで、それぞれに履歴を
    /// 1 件記録する）。`groups` で写す項目を選べる（既定はすべて）。適用した variant の数を返す。
    ///
    /// 現像中の写真が含まれていれば、保存待ちの変更を先に保存し、適用した設定で描き直す。
    pub fn paste_settings(
        &self,
        source: &DevelopSettings,
        targets: &[VariantId],
        groups: SettingGroups,
    ) -> Result<usize, ApiError> {
        self.paste_settings_with_label(source, targets, groups, HISTORY_LABEL_PASTE)
    }

    /// [`Core::paste_settings`] と同じ処理で、履歴の名前を `label` にする（CLI の `develop set` /
    /// `develop reset` が、写真を開かずに設定を保存するときに使う。名前は [`describe_change`] で作れる）。
    pub fn paste_settings_with_label(
        &self,
        source: &DevelopSettings,
        targets: &[VariantId],
        groups: SettingGroups,
        label: &str,
    ) -> Result<usize, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        source.validate().map_err(genzo_model::DevelopError::from)?;
        if targets.is_empty() {
            return Ok(0);
        }
        let mut state = inner.develop.state.lock();
        if let Some(s) = state.as_mut().filter(|s| targets.contains(&s.variant_id)) {
            s.dragging = false;
            save_locked(inner, s, None)?;
        }
        let seq = inner.develop.next_save_seq();
        let mut record = inner.develop.save.lock();
        // 外部データ（render_deps）は貼り付けない（写真ごとに違う。`deps` の doc）: 貼り付け先の値のままにし、
        // 現像中の写真なら、実際に使っている値にする。
        let session_deps = state
            .as_ref()
            .map(|s| (s.variant_id, s.render_deps.clone()));
        let applied: Vec<(VariantId, DevelopSettings)> = inner.with_catalog_api(|c| {
            let mut items = Vec::with_capacity(targets.len());
            for &v in targets {
                let current = c.develop_settings(v)?;
                if !groups.is_all() && current.process_version != source.process_version {
                    return Err(ApiError::InvalidArgument(format!(
                        "処理バージョンが違う variant {v} には、一部の項目だけを貼り付けられません（{} と {}）",
                        current.process_version, source.process_version
                    )));
                }
                let deps = match &session_deps {
                    Some((sv, d)) if *sv == v => d.clone(),
                    _ => current.render_deps.clone(),
                };
                items.push((
                    v,
                    crate::deps::with_render_deps(&groups.merge(source, &current), &deps),
                ));
            }
            c.save_develop_batch(&items, label)?;
            Ok(items)
        })?;
        if let Some(s) = state.as_ref()
            && targets.contains(&s.variant_id)
        {
            record.record(s.variant_id, seq);
        }
        drop(record);
        if let Some(s) = state.as_mut()
            && let Some((_, settings)) = applied.iter().find(|(v, _)| *v == s.variant_id)
        {
            s.settings = settings.clone();
            s.saved = settings.clone();
            s.saved_seq = seq;
            s.generation = inner.develop.next_generation();
            submit_render(inner, s, RenderQuality::Final);
        }
        drop(state);
        let mut ids: Vec<VariantId> = applied.iter().map(|(v, _)| *v).collect();
        ids.sort();
        ids.dedup();
        after_develop_saved(inner, &ids);
        Ok(ids.len())
    }

    /// 仮想コピーを作る（LIB-13。元の variant の現在の現像設定をコピーする）。
    pub fn create_virtual_copy(
        &self,
        variant_id: VariantId,
        name: Option<&str>,
    ) -> Result<VariantId, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        // 現像中の写真の保存待ちの変更を含めてコピーする。
        self.flush_develop()?;
        let new_id = inner.with_catalog(|c| c.create_virtual_copy(variant_id, name))?;
        // サムネイルは元の variant のものを写しておき、現像結果から作り直す。
        if let Ok(Some(t)) = inner.with_cache(|c| c.thumbs.get(variant_id)) {
            let _ = inner.with_cache(|c| c.thumbs.put(new_id, &t.cache_key, &t.jpeg));
        }
        inner.events.emit(Event::CatalogChanged {
            change: CatalogChange::VariantAdded,
            variant_ids: vec![new_id],
            all: false,
        });
        crate::search::refresh(inner, &[]);
        schedule_regenerate(inner, new_id);
        Ok(new_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_from_other_sessions_or_older_generations_are_not_shown() {
        let a = VariantId::new(1);
        let b = VariantId::new(2);
        // 開いたときの世代 10、まだプレビューがない。
        assert!(accepts_frame((a, 10, None), a, 10));
        assert!(accepts_frame((a, 10, None), a, 12));
        // 同じ写真の前のセッションの描画（世代 9 以前）は使わない。
        assert!(!accepts_frame((a, 10, None), a, 9));
        // 別の写真の描画は使わない。
        assert!(!accepts_frame((a, 10, None), b, 15));
        // 今のプレビューより古い世代は使わない（同じ世代・新しい世代は使う）。
        assert!(!accepts_frame((a, 10, Some(14)), a, 13));
        assert!(accepts_frame((a, 10, Some(14)), a, 14));
        assert!(accepts_frame((a, 10, Some(14)), a, 15));
    }

    /// 自動保存の期限は、最後の変更からの待ち時間と、最初の未保存の変更からの最大の待ち時間の早い方
    /// （指摘 F35）。
    #[test]
    fn save_deadline_is_capped_by_the_first_unsaved_change() {
        let t0 = Instant::now();
        let session = |last: Duration, first: Option<Duration>, retry: Option<Duration>| Session {
            variant_id: VariantId::new(1),
            file_revision: 1,
            source: PhotoSource::new(
                genzo_pipeline::SourceId::new(genzo_model::FileId::new(1), 1),
                genzo_pipeline::SourceImage::Working {
                    image: Arc::new(
                        genzo_pipeline::RgbImage::from_vec(
                            1,
                            1,
                            genzo_pipeline::ColorContract::B2Working,
                            vec![[0.0; 3]],
                        )
                        .unwrap(),
                    ),
                    orientation: genzo_model::Orientation::Normal,
                },
            )
            .unwrap(),
            source_info: SourceInfo {
                is_raw: false,
                width: 1,
                height: 1,
                decoder: None,
                as_shot_white_balance: None,
            },
            render_deps: genzo_model::RenderDeps::default(),
            settings: DevelopSettings::default(),
            saved: DevelopSettings::default(),
            saved_seq: 0,
            generation: 1,
            first_generation: 1,
            last_change: t0 + last,
            first_unsaved_change: first.map(|f| t0 + f),
            retry_after: retry.map(|r| t0 + r),
            dragging: false,
            drag_base_a1: None,
            requested_quality: RenderQuality::Final,
            frame: None,
        };
        let (delay, max) = (Duration::from_secs(1), Duration::from_secs(3));
        let ms = Duration::from_millis;
        // 休みがあれば、最後の変更から 1 秒後。
        assert_eq!(
            save_deadline(&session(ms(500), Some(ms(0)), None), delay, max),
            t0 + ms(1500)
        );
        // 変更が続いていても、最初の未保存の変更から 3 秒後には保存する。
        assert_eq!(
            save_deadline(&session(ms(2900), Some(ms(0)), None), delay, max),
            t0 + ms(3000)
        );
        // 保存に失敗した後は、次に試す時刻。
        assert_eq!(
            save_deadline(
                &session(ms(2900), Some(ms(0)), Some(ms(10_000))),
                delay,
                max
            ),
            t0 + ms(10_000)
        );
    }

    /// state のロックを外して書く保存は、より新しい写しが書かれた後なら書かない（古い設定で上書きしない）。
    #[test]
    fn older_snapshots_are_not_written_after_newer_ones() {
        let (a, b) = (VariantId::new(1), VariantId::new(2));
        let mut r = SaveRecord::default();
        assert!(!r.has_newer(a, 1));
        r.record(a, 5);
        assert!(r.has_newer(a, 4), "a に 5 を書いた後の 4 は古い");
        assert!(!r.has_newer(a, 6));
        assert!(!r.has_newer(b, 1), "別の写真には関係しない");
    }

    #[test]
    fn generations_increase_across_sessions() {
        let shared = DevelopShared::default();
        let g: Vec<u64> = (0..5).map(|_| shared.next_generation()).collect();
        assert_eq!(g, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn change_labels_name_the_changed_items() {
        let a = DevelopSettings::default();
        let mut b = a.clone();
        b.exposure_ev = 0.3;
        assert_eq!(describe_change(&a, &b), "露光量 +0.30");
        b.tone.shadows = 20.0;
        b.color.saturation = -10.0;
        assert_eq!(
            describe_change(&a, &b),
            "露光量 +0.30・シャドウ +20・彩度 -10"
        );
        b.contrast = 5.0;
        assert!(describe_change(&a, &b).ends_with("ほか"));
        assert_eq!(describe_change(&a, &a), HISTORY_LABEL_GENERIC);
    }
}
