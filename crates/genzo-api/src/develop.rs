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
//!   [`Core::close_develop`]・[`Core::close`] で保存待ちの変更を保存する。**保存に失敗したら、セッションを
//!   捨てない**（未保存の設定の唯一のメモリ上の写しなので）: 写真を閉じる・別の写真を開く・Undo・一括適用は
//!   エラーで戻ってセッションを残し、[`Core::close`] は何も閉じずにエラーで戻る（もう一度閉じられる）。
//!   閉じ直せない最後の機会（drop など）に保存できなければ、警告して設定を捨て、カタログに「正常に終了した」
//!   印を付けずに閉じる（[`shutdown`]）。
//! - Undo / Redo（DEV-27）はカタログの履歴で行うので、再起動の後もできる。
//! - 設定のコピーと複数の variant への一括適用（DEV-30。[`Core::paste_settings`]・[`Core::paste_settings_with`]）、
//!   仮想コピーの作成（LIB-13。[`Core::create_virtual_copy`]）。貼り付けは、貼り付け先の処理バージョンを
//!   変えない（利用者が選んだときだけ上げる。[`PasteVersion`]）。版 1 と 2 の写真の間でも、選んだ項目だけを
//!   写せる。
//! - 処理バージョンの更新（DATA-09。現像中の写真は [`Core::upgrade_process_version`]、選んだ写真をまとめて
//!   [`Core::upgrade_process_versions`]。v1 の機能は処理バージョン 2 から）。
//! - 切り抜きの編集の支え（DEV-15）: 切り抜く前の全体の表示（[`Core::set_develop_view`]）と、描画が実際に
//!   使う切り抜き（[`Core::constrain_crop`]）。WB のスポイト（DEV-03。[`Core::white_balance_at`]）。
//! - ビフォー / アフターの表示のビフォー（DEV-31。[`Core::set_before`]）はセッションに持つ（`before` の doc）。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use genzo_jobs::{JobOptions, Priority, SupersedePolicy};
use genzo_model::{CropRect, DevelopSettings, HistoryEntryId, Phase, RenderQuality, VariantId};
use genzo_pipeline::finish::output::{Dither, quantize_u8};
use genzo_pipeline::{PhotoSource, PreviewRequest, SourceImage};
use genzo_worker::Lane;
use parking_lot::{Condvar, Mutex, MutexGuard};

use crate::config::{DEVELOP_SAVE_DELAY, DEVELOP_SAVE_MAX_DELAY};
use crate::core::{Core, DevelopSaveStage, Inner};
use crate::error::ApiError;
use crate::events::{CatalogChange, Event, WarningCode};
use crate::previews::{load_source, schedule_regenerate, variant_file};
use crate::render::display_target;
use crate::types::{
    DevelopFrame, DevelopState, FrameInfo, HistogramData, HistoryItem, PasteReport, PasteVersion,
    PickedWhiteBalance, SettingGroups, SourceInfo,
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
/// 白黒にしたときの履歴の名前（DEV-25。画面の V キー）。
pub const HISTORY_LABEL_BLACK_WHITE: &str = "白黒に変換";
/// 白黒からカラーに戻したときの履歴の名前（DEV-25）。
pub const HISTORY_LABEL_COLOR: &str = "カラーに変換";
/// 処理バージョンの更新（[`Core::upgrade_process_version`]）の履歴の名前の前半（後に「2 に更新」が付く。
/// [`upgrade_process_label`]）。
pub const HISTORY_LABEL_UPGRADE_PROCESS: &str = "処理バージョンを";

/// 処理バージョンを `version` に更新したときの履歴の名前（「処理バージョンを 2 に更新」。CLI も使う）。
pub fn upgrade_process_label(version: u32) -> String {
    format!("{HISTORY_LABEL_UPGRADE_PROCESS} {version} に更新")
}

/// 現像のセッション。
pub(crate) struct Session {
    pub(crate) variant_id: VariantId,
    file_revision: u32,
    pub(crate) source: PhotoSource,
    source_info: SourceInfo,
    /// 実際に使う外部データ（RAW のカメラ行列・デコーダ。`deps` の doc）。設定の `render_deps` は常にこの値
    /// にする（利用者の設定では変えない）。
    pub(crate) render_deps: genzo_model::RenderDeps,
    /// 現在の設定（最新の要求）。
    pub(crate) settings: DevelopSettings,
    /// 最後に保存した設定。
    saved: DevelopSettings,
    /// `saved` の写しの番号（[`DevelopShared::next_save_seq`]。古い写しの保存の結果で上書きしない）。
    saved_seq: u64,
    /// 設定の世代（変えるたびに増える。コアの中で通しの番号で、開き直しても 1 に戻らない）。
    generation: u64,
    /// このセッションを開いたときの世代（これより前の世代の描画は、前のセッションのもの）。
    pub(crate) first_generation: u64,
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
    /// 切り抜く前の全体の表示か（[`Core::set_develop_view`]。写真を開き直すと `false`）。
    pub(crate) uncropped: bool,
    /// 最新の描画の結果。
    frame: Option<Arc<DevelopFrame>>,
    /// ビフォー / アフターの表示のビフォー（DEV-31。[`Core::set_before`]。写真を開き直すと `None`）。
    pub(crate) before: Option<crate::before::BeforeState>,
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
    /// 現像中の写真の現像設定をカタログに書く操作を 1 つずつにする（[`save_unlocked`] は state のロックの
    /// 外で書くため。ロックの順序は state → save → カタログ）。書く操作は、書いた後に
    /// [`DevelopShared::record_written`] で記録する。
    save: Mutex<()>,
    /// state のロックを外して書く保存の追い越しの記録（[`SaveRecord`]）。**葉のロック**: 持っている間に
    /// ほかのロックを取らず、待たない（state のロックを持ったまま取るので、`save` とは別にしている。
    /// `save` はカタログのロックを待つ間も持たれる）。
    save_record: Mutex<SaveRecord>,
    /// 保存する設定の写しの通しの番号（大きいほど新しい）。
    save_seq: AtomicU64,
    /// 表示範囲の描画（[`Core::render_region`]。`region` の doc）。
    pub region: Mutex<crate::region::RegionState>,
    /// ビフォーの表示範囲の描画（[`Core::render_before_region`]。DEV-31）。
    pub region_before: Mutex<crate::region::RegionState>,
}

/// state のロックを外して書く保存（[`save_unlocked`]）の追い越しの記録（[`DevelopShared::save_record`]）。
///
/// 写しの番号は、設定を変えてカタログに書く操作がどれも state のロックの中で割り当てるので、同じ variant
/// では、番号が大きいほど新しい設定。state のロックを外してから書くまでの間に、同じ variant のもっと新しい
/// 写しが書かれたら、古い写しは書かない（新しい設定と履歴を上書きしない）。
///
/// 覚えるのは variant ごと。以前のように全体で 1 件だけ（最後に書いた variant と番号）にすると、別の
/// variant を書いた時点で、止まっている保存が見るべき記録が消えた。
///
/// 覚えるのは、**書くか書かないかを決める前の保存（[`SaveRecord::begin`] から [`SaveRecord::end`] まで）が
/// ある variant だけ**で、その保存がなくなれば記録も捨てる。書いた variant の数に応じて増え続けないので、
/// セッションを閉じるときや variant を消すときの掃除は要らない（掃除を入れると、止まっている保存が
/// 見るべき記録まで消しうる）。
#[derive(Default)]
struct SaveRecord {
    /// variant ごとの、決める前の保存と、その間に書かれた写し。
    pending: HashMap<VariantId, PendingSaves>,
}

/// 1 つの variant について、決める前の保存の状態（[`SaveRecord::pending`]）。
#[derive(Default)]
struct PendingSaves {
    /// 決める前の保存の数。
    count: usize,
    /// その間に書かれた、最も新しい写しの番号（戻らない）。
    newest: u64,
}

impl SaveRecord {
    /// `variant` の保存が、state のロックを外して、書くか書かないかを決めるまで待ち始める。**写しの番号を
    /// 割り当てたのと同じ state のロックの中**で呼ぶ（これより後に割り当てた番号の書き込みを、すべて覚える
    /// ため）。
    fn begin(&mut self, variant: VariantId) {
        self.pending.entry(variant).or_default().count += 1;
    }

    /// [`SaveRecord::begin`] の保存が、書くか書かないかを決め終えた。待っている保存がなくなれば記録を捨てる。
    fn end(&mut self, variant: VariantId) {
        if let Some(p) = self.pending.get_mut(&variant) {
            p.count = p.count.saturating_sub(1);
            if p.count == 0 {
                self.pending.remove(&variant);
            }
        }
    }

    /// `variants` に番号 `seq` の写しを書いた。待っている保存がある variant だけ覚える。
    fn written(&mut self, variants: &[VariantId], seq: u64) {
        for v in variants {
            if let Some(p) = self.pending.get_mut(v) {
                p.newest = p.newest.max(seq);
            }
        }
    }

    /// 待っている保存の番号 `seq` より新しい写しを、`variant` に書いた後か。
    fn has_newer(&self, variant: VariantId, seq: u64) -> bool {
        self.pending.get(&variant).is_some_and(|p| p.newest > seq)
    }
}

/// state のロックを外して書く保存が、書くか書かないかを決めるまでの印（drop で [`SaveRecord::end`]）。
struct UnlockedSave<'a> {
    shared: &'a DevelopShared,
    variant: VariantId,
    seq: u64,
}

impl UnlockedSave<'_> {
    /// 書く前に、同じ variant にもっと新しい写しが書かれていないか（`save` のロックを持って呼ぶ）。
    fn is_superseded(&self) -> bool {
        self.shared
            .save_record
            .lock()
            .has_newer(self.variant, self.seq)
    }
}

impl Drop for UnlockedSave<'_> {
    fn drop(&mut self) {
        self.shared.save_record.lock().end(self.variant);
    }
}

impl DevelopShared {
    /// 新しい設定の世代。
    ///
    /// 世代はセッションごとに 1 から数え直さない。数え直すと、同じ写真を開き直したときに、前の
    /// セッションで遅れて終わった描画（大きい世代）が新しいセッションのプレビューになり、その後の
    /// 新しい描画が「古い」として捨てられる（6.2 節・4.1 節）。UI がイベントの世代で新旧を比べるときも
    /// 取り違えない。
    pub(crate) fn next_generation(&self) -> u64 {
        self.last_generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// 保存する設定の写しの新しい番号。
    fn next_save_seq(&self) -> u64 {
        self.save_seq.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// `variants` に番号 `seq` の現像設定を書いた（`save` のロックを持ったまま、カタログに書いた直後に呼ぶ。
    /// 現像設定を書くのは、写す元の variant に限らず、すべてここに記録する）。
    fn record_written(&self, variants: &[VariantId], seq: u64) {
        self.save_record.lock().written(variants, seq);
    }

    /// state のロックを外して書く保存（番号 `seq`）を、追い越しの記録に登録する。**番号を割り当てたのと同じ
    /// state のロックの中**で呼び、書くか書かないかを決めるまで（返す印を drop するまで）持つ。
    fn begin_unlocked_save(&self, variant: VariantId, seq: u64) -> UnlockedSave<'_> {
        self.save_record.lock().begin(variant);
        UnlockedSave {
            shared: self,
            variant,
            seq,
        }
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
    if old.highlight_recovery != new.highlight_recovery {
        parts.push("ハイライト復元".to_owned());
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
    let pr = (&old.presence, &new.presence);
    if pr.0.texture != pr.1.texture {
        parts.push(format!("テクスチャ {:+.0}", pr.1.texture));
    }
    if pr.0.clarity != pr.1.clarity {
        parts.push(format!("明瞭度 {:+.0}", pr.1.clarity));
    }
    if pr.0.dehaze != pr.1.dehaze {
        parts.push(format!("かすみの除去 {:+.0}", pr.1.dehaze));
    }
    if old.tone_curve != new.tone_curve {
        parts.push("トーンカーブ".to_owned());
    }
    let (c_old, c_new) = (&old.color, &new.color);
    let before_color = parts.len();
    if c_old.black_white.enabled != c_new.black_white.enabled {
        parts.push(
            if c_new.black_white.enabled {
                HISTORY_LABEL_BLACK_WHITE
            } else {
                HISTORY_LABEL_COLOR
            }
            .to_owned(),
        );
    }
    if c_old.saturation != c_new.saturation {
        parts.push(format!("彩度 {:+.0}", c_new.saturation));
    }
    if c_old.vibrance != c_new.vibrance {
        parts.push(format!("自然な彩度 {:+.0}", c_new.vibrance));
    }
    if c_old.mixer != c_new.mixer {
        parts.push("カラーミキサー".to_owned());
    }
    if c_old.black_white.mix != c_new.black_white.mix {
        parts.push("白黒ミキサー".to_owned());
    }
    if c_old.grading != c_new.grading {
        parts.push("カラーグレーディング".to_owned());
    }
    if c_old != c_new && parts.len() == before_color {
        parts.push("色".to_owned());
    }
    if old.geometry != new.geometry {
        parts.push("切り抜き・角度補正".to_owned());
    }
    if old.lens != new.lens {
        parts.push("レンズ補正".to_owned());
    }
    let (nr_old, nr_new) = (&old.detail.noise_reduction, &new.detail.noise_reduction);
    if nr_old.luminance != nr_new.luminance {
        parts.push(format!("輝度ノイズの軽減 {:.0}", nr_new.luminance));
    }
    if nr_old.color != nr_new.color {
        parts.push(format!("カラーノイズの軽減 {:.0}", nr_new.color));
    }
    if nr_old != nr_new && nr_old.luminance == nr_new.luminance && nr_old.color == nr_new.color {
        parts.push("ノイズ軽減".to_owned());
    }
    let (sh_old, sh_new) = (&old.detail.sharpening, &new.detail.sharpening);
    if sh_old.amount != sh_new.amount {
        parts.push(format!("シャープ {:.0}", sh_new.amount));
    } else if sh_old != sh_new {
        parts.push("シャープ".to_owned());
    }
    let (fx_old, fx_new) = (&old.effects, &new.effects);
    if fx_old.vignette.amount != fx_new.vignette.amount {
        parts.push(format!("周辺光量 {:+.0}", fx_new.vignette.amount));
    } else if fx_old.vignette != fx_new.vignette {
        parts.push("周辺光量".to_owned());
    }
    if fx_old.grain.amount != fx_new.grain.amount {
        parts.push(format!("粒子 {:.0}", fx_new.grain.amount));
    } else if fx_old.grain != fx_new.grain {
        parts.push("粒子".to_owned());
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
        let _write = inner.develop.save.lock();
        inner.with_catalog(|c| c.save_develop(v, &settings, &label))?;
        inner.develop.record_written(&[v], seq);
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
/// 取ってからロックを外して書き、ロックを取り直して `saved` を更新する。書く前に、同じ variant の
/// より新しい写しが書かれていれば（ロックを外している間に、一括適用・Undo などが書いた。間に別の variant
/// が書かれていても同じ。[`SaveRecord`]）書かない（保存待ちのまま残り、次の保存で書く）。
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
    // 番号を割り当てたのと同じ state のロックの中で登録する（これより後の、同じ variant への書き込みを
    // 取りこぼさないため）。
    let pending = inner.develop.begin_unlocked_save(v, seq);
    let written = MutexGuard::unlocked(state, || -> Result<bool, ApiError> {
        // テスト用の口（`CoreHooks::on_develop_save`）。state のロックを外した後、保存のロックを取る前に、
        // ロックを持たずに呼ぶ。
        if let Some(hook) = &inner.on_develop_save {
            hook(DevelopSaveStage::Unlocked(v));
        }
        let write = inner.develop.save.lock();
        if pending.is_superseded() {
            return Ok(false);
        }
        inner.with_catalog(|c| c.save_develop(v, &settings, &label))?;
        inner.develop.record_written(&[v], seq);
        drop(write);
        after_develop_saved(inner, &[v]);
        Ok(true)
    })?;
    drop(pending);
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

/// 現像中の写真の入力が `id`（ファイルとそのリビジョン）なら、その入力と外部データ（比較表示が、現像中の写真と
/// 同じファイル（仮想コピー）を展開し直さないため。`view` の doc）。
pub(crate) fn session_source_of_file(
    inner: &Inner,
    id: genzo_pipeline::SourceId,
) -> Option<(PhotoSource, genzo_model::RenderDeps)> {
    let state = inner.develop.state.lock();
    state
        .as_ref()
        .filter(|s| s.source.id == id)
        .map(|s| (s.source.clone(), s.render_deps.clone()))
}

/// 描画を投入する（「最新の 1 件だけ」）。ビフォーを設定していれば、必要ならビフォーも描き直す
/// （アフターの後に投入する。`before` の doc）。
fn submit_render(inner: &Arc<Inner>, s: &mut Session, quality: RenderQuality) {
    s.requested_quality = quality;
    let job = RenderJob {
        variant_id: s.variant_id,
        source: s.source.clone(),
        settings: s.settings.clone(),
        generation: s.generation,
        quality,
        uncropped: s.uncropped,
        histogram: true,
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
    crate::before::maybe_submit(inner, s);
}

/// 描画の要求（セッションの状態の写し）。
pub(crate) struct RenderJob {
    pub(crate) variant_id: VariantId,
    pub(crate) source: PhotoSource,
    pub(crate) settings: DevelopSettings,
    pub(crate) generation: u64,
    pub(crate) quality: RenderQuality,
    /// 切り抜く前の全体を描くか（[`Core::set_develop_view`]）。
    pub(crate) uncropped: bool,
    /// ヒストグラム（17c）を求めるか（ビフォーの描画では求めない）。
    pub(crate) histogram: bool,
}

/// プレビューを描く（17a・17c まで）。
///
/// 切り抜く前の全体の表示（[`Core::set_develop_view`]）では、設定の切り抜きを使わず G の外接矩形の全体を
/// 描く（回転で画像の外になった四隅は黒で塗り、ヒストグラムに数えない。[`FrameInfo::uncropped`]）。
pub(crate) fn render_frame(inner: &Inner, job: &RenderJob) -> Result<DevelopFrame, ApiError> {
    let (target, assumed_srgb) = display_target(inner);
    let mut request = PreviewRequest::new(job.quality, inner.engine.config().preview_long_edge)
        .with_output(target)
        .with_uncropped(job.uncropped);
    if job.histogram {
        request = request.with_histogram();
    }
    let r = inner.render_preview(&job.source, &job.settings, &request, Some(job.variant_id))?;
    // 表示範囲の描画（render_region）と同じ格子の、フル解像度の出力の寸法。
    let (output_width, output_height) =
        match inner
            .engine
            .full_resolution_geometry(&job.source, &job.settings, job.uncropped)
        {
            Ok(g) => g.output_size(),
            Err(e) => {
                tracing::warn!(error = %e, "フル解像度の出力の寸法を求められない");
                (0, 0)
            }
        };
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
            uncropped: job.uncropped,
            output_width,
            output_height,
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

/// 画面のプロファイルを変えたときなど、現在の設定で描き直す（ビフォーも描き直す）。
pub(crate) fn rerender(inner: &Arc<Inner>) {
    let mut state = inner.develop.state.lock();
    if let Some(s) = state.as_mut() {
        if let Some(b) = s.before.as_mut() {
            b.force_redraw();
        }
        redraw(inner, s);
    }
}

/// 設定の世代を進めて描き直す（ドラッグ中なら最後に要求した品質、そうでなければ最終品質）。
fn redraw(inner: &Arc<Inner>, s: &mut Session) {
    s.generation = inner.develop.next_generation();
    let q = if s.dragging {
        s.requested_quality
    } else {
        RenderQuality::Final
    };
    submit_render(inner, s, q);
}

/// 現像中の写真の状態の写し（[`session_snapshot`]）。
pub(crate) struct SessionSnapshot {
    /// 入力。
    pub source: PhotoSource,
    /// 今の設定（保存待ちの変更・ドラッグ中の値を含む）。
    pub settings: DevelopSettings,
    /// 設定の世代。
    pub generation: u64,
    /// 切り抜く前の全体の表示か（[`Core::set_develop_view`]）。
    pub uncropped: bool,
    /// 最新に公開したプレビュー（[`Core::develop_frame`]）が切り抜く前の全体の表示か（[`FrameInfo::uncropped`]。
    /// まだ描けていなければ `None`）。表示を切り替えた直後は、新しい表示のプレビューが届くまで
    /// [`uncropped`](Self::uncropped) と違う。
    pub frame_uncropped: Option<bool>,
    /// ビフォーで実際に描く設定（DEV-31。`before` の `effective_settings`。ビフォーを設定していなければ
    /// `None`）。
    pub before_settings: Option<DevelopSettings>,
}

/// 表示範囲の描画・WB のスポイトなどに使う、現像中の写真の状態の写し。`variant_id` の写真を開いて
/// いなければ [`ApiError::NoDevelopSession`]。
pub(crate) fn session_snapshot(
    inner: &Inner,
    variant_id: VariantId,
) -> Result<SessionSnapshot, ApiError> {
    let state = inner.develop.state.lock();
    let s = state
        .as_ref()
        .filter(|s| s.variant_id == variant_id)
        .ok_or(ApiError::NoDevelopSession)?;
    Ok(SessionSnapshot {
        source: s.source.clone(),
        settings: s.settings.clone(),
        generation: s.generation,
        uncropped: s.uncropped,
        frame_uncropped: s.frame.as_ref().map(|f| f.info.uncropped),
        before_settings: s
            .before
            .as_ref()
            .map(|b| crate::before::effective_settings(b.base(), &s.settings, &s.render_deps)),
    })
}

/// WB のスポイトの点を解釈する表示（切り抜く前の全体の表示か。[`Core::white_balance_at`]）。
///
/// UI が送った表示（クリックした画像の [`FrameInfo::uncropped`]）があればそれ。なければ、最新に公開した
/// プレビューの表示（UI が表示しているのは、それかそれより前のプレビュー）。プレビューがまだなければ今の表示。
fn pick_view(requested: Option<bool>, frame: Option<bool>, current: bool) -> bool {
    requested.or(frame).unwrap_or(current)
}

/// 処理バージョンの番号を、このアプリが知っている版にする（知らない版は [`ApiError::InvalidArgument`]。
/// `whose` はエラーの文の「〜の処理バージョン」の前の部分。例: `"variant 3 の"`）。
fn known_process_version(
    version: u32,
    whose: &str,
) -> Result<genzo_pipeline::ProcessVersion, ApiError> {
    genzo_pipeline::ProcessVersion::from_u32(version).map_err(|_| {
        ApiError::InvalidArgument(format!(
            "{whose}処理バージョン {version} は、このアプリでは扱えません（対応は {} まで）",
            genzo_model::CURRENT_PROCESS_VERSION
        ))
    })
}

/// 設定に、その処理バージョンでは使われず、最新の版に更新すると使われる項目の値があるか（版 1 の写真の
/// レンズ補正など。genzo-pipeline の `finish::unimplemented_settings` で比べる）。
pub(crate) fn unused_until_upgrade(settings: &DevelopSettings) -> bool {
    let latest = genzo_model::CURRENT_PROCESS_VERSION;
    if settings.process_version >= latest {
        return false;
    }
    let upgraded = DevelopSettings {
        process_version: latest,
        ..settings.clone()
    };
    genzo_pipeline::finish::unimplemented_settings(settings).len()
        > genzo_pipeline::finish::unimplemented_settings(&upgraded).len()
}

/// 動画の variant が含まれていればエラーにする（動画には現像設定を保存しない。implementation_status の
/// No.31。動画は現像できず（[`Core::open_develop`] の doc）、保存した設定は使われないため、一括適用や
/// CLI の `develop set` で誤って保存しないようにする）。
fn reject_videos(c: &genzo_catalog::Catalog, targets: &[VariantId]) -> Result<(), ApiError> {
    if let Some(v) = c
        .variant_summaries(targets)?
        .iter()
        .find(|s| s.kind == genzo_model::AssetKind::Video)
    {
        return Err(ApiError::InvalidArgument(format!(
            "動画（variant {}）には現像設定を保存できません（動画はカタログの管理だけ）",
            v.variant_id
        )));
    }
    Ok(())
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

/// [`shutdown`] の結果。
#[derive(Debug)]
pub(crate) enum ShutdownOutcome {
    /// すでに終了している（別の終了の処理が、現像のセッションの扱いを済ませた）。
    AlreadyClosed,
    /// 保存待ちの変更を保存した（保存待ちがなかった場合を含む）。セッションを捨て、コアを終了済みにした。
    Saved,
    /// 保存できなかったが、最後の機会（`last_chance`）なので、セッションを捨て、コアを終了済みにした。
    /// 保存待ちだった設定は失われる（呼び出し側は、カタログに「正常に終了した」印を付けない）。
    Abandoned(ApiError),
    /// 保存できなかった。セッション（未保存の設定の唯一のメモリ上の写し）もコアも、そのまま
    /// （終了済みにしない。もう一度閉じられる）。
    Kept(ApiError),
}

/// 終了の処理の最初: 保存待ちの変更を保存し、保存できたら、現像のセッションを捨て、コアを終了済みにして、
/// 自動保存のスレッドを止める。
///
/// 保存に失敗したときは、セッションを捨てない。セッションは、保存待ちの設定の唯一のメモリ上の写しで、
/// 捨てるとその編集は失われる（原因がディスクの空き容量の不足などの一時的なものでも）。`last_chance` が
/// 偽（[`Core::close`]）なら、何も変えずに [`ShutdownOutcome::Kept`] で返す（コアは終了済みにならず、
/// 自動保存のスレッドも動き続けるので、呼び出し側は、原因を取り除いてもう一度閉じられる）。真（drop・
/// 起動の失敗の後始末など、もう閉じ直せない）なら、閉じるしかないので、セッションを捨てて
/// [`ShutdownOutcome::Abandoned`] で返し、警告 [`WarningCode::SaveFailed`] で知らせる。
///
/// 終了済みの印（`closed`）は、保存の後に state のロックの中で立てる（保存に失敗したら立てない。
/// 以前は先に立てたので、失敗しても閉じ直せなかった）。ロックの中なので、同時に閉じる 2 つの処理のどちらかが
/// 保存を終えて印を立てた後、もう一方は [`ShutdownOutcome::AlreadyClosed`] になる。
pub(crate) fn shutdown(inner: &Arc<Inner>, last_chance: bool) -> ShutdownOutcome {
    let mut state = inner.develop.state.lock();
    if inner.closed.load(Ordering::Acquire) {
        return ShutdownOutcome::AlreadyClosed;
    }
    let (saved, variant) = match state.as_mut() {
        Some(s) => {
            s.dragging = false;
            (save_locked(inner, s, None).map(|_| ()), Some(s.variant_id))
        }
        None => (Ok(()), None),
    };
    let error = match saved {
        Err(e) if !last_chance => return ShutdownOutcome::Kept(e),
        Err(e) => {
            inner.events.warn(
                WarningCode::SaveFailed,
                format!(
                    "現像設定を保存できないまま終了します。保存できなかった編集は失われ、次に開いたときに「前回は正常に終了しませんでした」と表示されます: {e}"
                ),
                variant,
                None,
            );
            Some(e)
        }
        Ok(()) => None,
    };
    inner.closed.store(true, Ordering::Release);
    *state = None;
    inner.develop.stop.store(true, Ordering::Release);
    inner.develop.cv.notify_all();
    match error {
        Some(e) => ShutdownOutcome::Abandoned(e),
        None => ShutdownOutcome::Saved,
    }
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
        before: crate::before::info_of(s),
        latest_process_version: genzo_model::CURRENT_PROCESS_VERSION,
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
                    // 開き直したら、切り抜く前の全体の表示をやめる（[`Core::set_develop_view`]）。
                    if std::mem::replace(&mut s.uncropped, false) {
                        redraw(inner, s);
                        inner.develop.cv.notify_all();
                    }
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
            uncropped: false,
            frame: None,
            before: None,
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

    /// 現像中の写真の処理バージョンを、このアプリの最新（[`genzo_model::CURRENT_PROCESS_VERSION`]）に
    /// 更新する（DATA-09: 新しいアルゴリズムは、利用者が明示的に更新した写真にだけ使う。v1 の機能（レンズ
    /// 補正など）は処理バージョン 2 から適用する）。設定のほかの値は変えない。履歴に 1 件
    /// （[`HISTORY_LABEL_UPGRADE_PROCESS`]）記録してすぐに保存し、描き直す（Undo で戻せる）。すでに最新なら
    /// 何もしない。ドラッグ中なら、ドラッグを終えたものとして扱う。
    pub fn upgrade_process_version(&self) -> Result<DevelopState, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let mut state = inner.develop.state.lock();
        let s = state.as_mut().ok_or(ApiError::NoDevelopSession)?;
        let v = s.variant_id;
        let latest = genzo_model::CURRENT_PROCESS_VERSION;
        if s.settings.process_version < latest {
            s.dragging = false;
            s.drag_base_a1 = None;
            s.settings.process_version = latest;
            s.generation = inner.develop.next_generation();
            s.last_change = Instant::now();
            s.retry_after = None;
            if s.first_unsaved_change.is_none() {
                s.first_unsaved_change = Some(s.last_change);
            }
            let quality = s.source.effective_quality(RenderQuality::Final);
            submit_render(inner, s, quality);
            inner.develop.cv.notify_all();
            save_unlocked(inner, &mut state, Some(&upgrade_process_label(latest)))?;
        }
        let d = MutexGuard::unlocked(&mut state, || inner.with_catalog(|c| c.develop_state(v)))?;
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
    ///
    /// 保存できなければエラーを返し、セッションは閉じない（未保存の設定は残る。原因を取り除いてから、
    /// もう一度閉じられる）。
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
                uncropped: s.uncropped,
                histogram: true,
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
    /// 戻せなければ `None`。動画の variant は [`ApiError::InvalidArgument`]（現像設定を保存しない。No.31）。
    pub fn undo(&self, variant_id: VariantId) -> Result<Option<DevelopSettings>, ApiError> {
        self.step_history(variant_id, false)
    }

    /// 1 つ先の履歴に進める（DEV-27）。進められなければ `None`。
    pub fn redo(&self, variant_id: VariantId) -> Result<Option<DevelopSettings>, ApiError> {
        self.step_history(variant_id, true)
    }

    /// 履歴の項目 `entry` の位置へ 1 回で移る（DEV-27。履歴のパネルで項目を選んだとき）。保存待ちの変更が
    /// あれば、先に保存してから移る（保存で、今の位置より後の項目は消える）。移った後の設定を返し、その項目が
    /// ない（保存で消えた・別の variant の項目）・すでにその位置にいるときは `None`（何も変えない）。
    ///
    /// Undo / Redo を繰り返すのと同じ結果になるが、保存・変更の知らせ・描き直しは 1 回だけ。
    pub fn go_to_history(
        &self,
        variant_id: VariantId,
        entry: HistoryEntryId,
    ) -> Result<Option<DevelopSettings>, ApiError> {
        self.move_history(variant_id, HistoryMove::To(entry))
    }

    fn step_history(
        &self,
        variant_id: VariantId,
        forward: bool,
    ) -> Result<Option<DevelopSettings>, ApiError> {
        self.move_history(
            variant_id,
            if forward {
                HistoryMove::Redo
            } else {
                HistoryMove::Undo
            },
        )
    }

    fn move_history(
        &self,
        variant_id: VariantId,
        to: HistoryMove,
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
            let _write = inner.develop.save.lock();
            let result = inner.with_catalog_api(|c| {
                reject_videos(c, &[variant_id])?;
                Ok(match to {
                    HistoryMove::Undo => c.undo_develop(variant_id)?,
                    HistoryMove::Redo => c.redo_develop(variant_id)?,
                    HistoryMove::To(entry) => {
                        let history = c.history(variant_id)?;
                        match history.iter().find(|h| h.id == entry) {
                            Some(h) if !h.is_current => Some(c.go_to_history(variant_id, entry)?),
                            _ => None,
                        }
                    }
                })
            })?;
            if result.is_some() {
                inner.develop.record_written(&[variant_id], seq);
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
    /// **貼り付け先の処理バージョンは変えない**（[`PasteVersion::Keep`]。DATA-09）。版を更新しながら貼り付けるには
    /// [`Core::paste_settings_with`]。処理バージョン 1 と 2 の写真の間でも、選んだ項目だけを写せる
    /// （[`PasteVersion`] の doc）。
    ///
    /// 現像中の写真が含まれていれば、保存待ちの変更を先に保存し、適用した設定で描き直す。動画の variant が
    /// 含まれていれば [`ApiError::InvalidArgument`] で、何も保存しない（動画には現像設定を保存しない。
    /// implementation_status の No.31）。
    pub fn paste_settings(
        &self,
        source: &DevelopSettings,
        targets: &[VariantId],
        groups: SettingGroups,
    ) -> Result<usize, ApiError> {
        let report = self.paste_settings_with(
            source,
            targets,
            groups,
            PasteVersion::Keep,
            HISTORY_LABEL_PASTE,
        )?;
        Ok(report.count as usize)
    }

    /// [`Core::paste_settings`] と同じ処理で、貼り付け先の処理バージョンの扱い（`version`）と履歴の名前
    /// （`label`）を指定する（画面の貼り付けのダイアログ・CLI の `develop copy` / `develop set` /
    /// `develop reset`。CLI は写真を開かずに設定を保存するときに使い、名前は [`describe_change`] で作れる）。
    ///
    /// - [`PasteVersion::Upgrade`] で処理バージョンを上げた写真の履歴の名前は「`label`・処理バージョンを 2 に更新」
    ///   にする（[`upgrade_process_label`]。[`PasteVersion::Source`] では `label` のまま）。
    /// - 貼り付け元と貼り付け先の版で項目の意味が違う（genzo-pipeline の `ProcessVersion::shares_settings_with`
    ///   が偽）・貼り付け先の版をこのアプリが知らない（新しいアプリで現像した写真）ときは
    ///   [`ApiError::InvalidArgument`] で、何も保存しない。
    pub fn paste_settings_with(
        &self,
        source: &DevelopSettings,
        targets: &[VariantId],
        groups: SettingGroups,
        version: PasteVersion,
        label: &str,
    ) -> Result<PasteReport, ApiError> {
        source.validate().map_err(genzo_model::DevelopError::from)?;
        let source_version = known_process_version(source.process_version, "貼り付け元の設定の")?;
        let mut report = PasteReport::default();
        let ids = self.apply_to_variants(targets, false, |v, current| {
            let target_version =
                known_process_version(current.process_version, &format!("variant {v} の"))?;
            if !target_version.shares_settings_with(source_version) {
                return Err(ApiError::InvalidArgument(format!(
                    "処理バージョン {} の設定は、処理バージョン {} の variant {v} には貼り付けられません（項目の意味が違います）",
                    source.process_version, current.process_version
                )));
            }
            let mut merged = groups.merge(source, current);
            merged.process_version = match version {
                PasteVersion::Keep => current.process_version,
                PasteVersion::Upgrade => current.process_version.max(source.process_version),
                PasteVersion::Source => source.process_version,
            };
            let upgraded = merged.process_version > current.process_version;
            if upgraded {
                report.upgraded += 1;
            }
            // 上げた写真の履歴の名前に、更新したことを足す（`Source` は呼び出し側の名前のまま。CLI の
            // `develop set` の名前（`describe_change`）には「処理バージョン」が入る）。
            let label = if upgraded && version == PasteVersion::Upgrade {
                format!("{label}・{}", upgrade_process_label(merged.process_version))
            } else {
                label.to_owned()
            };
            if unused_until_upgrade(&merged) {
                report.needs_upgrade += 1;
            }
            Ok(Some((merged, label)))
        })?;
        report.count = ids.len() as u64;
        Ok(report)
    }

    /// 選んだ写真の処理バージョンを、このアプリの最新（[`genzo_model::CURRENT_PROCESS_VERSION`]）にまとめて
    /// 更新する（DATA-09 の明示的な更新。ライブラリの写真の右クリックのメニュー・CLI の `develop upgrade`）。
    /// 更新した variant の数を返す。
    ///
    /// - 設定のほかの値は変えない。1 つのトランザクションで、それぞれの履歴に「処理バージョンを 2 に更新」
    ///   （[`upgrade_process_label`]）を 1 件記録する（Undo で戻せる）。
    /// - すでに最新の写真・動画（現像設定がない）は飛ばす（数に入れない。エラーにしない）。
    /// - 現像中の写真が含まれていれば、保存待ちの変更を先に保存し、更新した設定で描き直す
    ///   （[`Core::upgrade_process_version`] と同じ結果）。
    pub fn upgrade_process_versions(&self, targets: &[VariantId]) -> Result<usize, ApiError> {
        let latest = genzo_model::CURRENT_PROCESS_VERSION;
        let label = upgrade_process_label(latest);
        let ids = self.apply_to_variants(targets, true, |_, current| {
            Ok((current.process_version < latest).then(|| {
                (
                    DevelopSettings {
                        process_version: latest,
                        ..current.clone()
                    },
                    label.clone(),
                )
            }))
        })?;
        Ok(ids.len())
    }

    /// 選んだ variant の設定を、それぞれの今の設定から `change` で作って保存する（DEV-30 の貼り付けと
    /// 処理バージョンの一括の更新の共通の処理）。`targets` の重なりは 1 つにする（`change` は variant ごとに
    /// 1 回）。`change` が `None` を返した variant は保存しない。保存した variant（昇順）を返す。
    ///
    /// - 現像中の写真が含まれていれば、保存待ちの変更を先に保存してから今の設定を読み、保存した設定で描き直す。
    /// - 1 つのトランザクションで保存する（1 件でも失敗したら、何も保存しない）。外部データ（`render_deps`）は
    ///   貼り付け先の値のまま（現像中の写真なら、実際に使っている値）。
    /// - 動画の variant: `skip_videos` なら飛ばし、そうでなければ [`ApiError::InvalidArgument`]。
    fn apply_to_variants(
        &self,
        targets: &[VariantId],
        skip_videos: bool,
        mut change: impl FnMut(
            VariantId,
            &DevelopSettings,
        ) -> Result<Option<(DevelopSettings, String)>, ApiError>,
    ) -> Result<Vec<VariantId>, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        // 重なりは 1 回だけ（`change` を 1 回だけ呼ぶ。順序は保つ）。
        let mut seen = std::collections::HashSet::new();
        let targets: Vec<VariantId> = targets
            .iter()
            .copied()
            .filter(|v| seen.insert(*v))
            .collect();
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        let mut state = inner.develop.state.lock();
        if let Some(s) = state.as_mut().filter(|s| targets.contains(&s.variant_id)) {
            s.dragging = false;
            save_locked(inner, s, None)?;
        }
        let seq = inner.develop.next_save_seq();
        let write = inner.develop.save.lock();
        // 外部データ（render_deps）は貼り付けない（写真ごとに違う。`deps` の doc）: 貼り付け先の値のままにし、
        // 現像中の写真なら、実際に使っている値にする。
        let session_deps = state
            .as_ref()
            .map(|s| (s.variant_id, s.render_deps.clone()));
        let applied: Vec<(VariantId, DevelopSettings, String)> = inner.with_catalog_api(|c| {
            let videos: Vec<VariantId> = if skip_videos {
                c.variant_summaries(&targets)?
                    .into_iter()
                    .filter(|s| s.kind == genzo_model::AssetKind::Video)
                    .map(|s| s.variant_id)
                    .collect()
            } else {
                reject_videos(c, &targets)?;
                Vec::new()
            };
            let mut items = Vec::with_capacity(targets.len());
            for &v in targets.iter().filter(|v| !videos.contains(v)) {
                let current = c.develop_settings(v)?;
                let Some((new, label)) = change(v, &current)? else {
                    continue;
                };
                let deps = match &session_deps {
                    Some((sv, d)) if *sv == v => d.clone(),
                    _ => current.render_deps.clone(),
                };
                items.push((v, crate::deps::with_render_deps(&new, &deps), label));
            }
            if !items.is_empty() {
                c.save_develop_labeled(&items)?;
            }
            Ok(items)
        })?;
        let mut ids: Vec<VariantId> = applied.iter().map(|(v, _, _)| *v).collect();
        ids.sort();
        if ids.is_empty() {
            return Ok(ids);
        }
        // 書いた variant をすべて記録する（現像中の写真に限らない。現像していた写真の古い写しの保存が、
        // 現像のセッションが閉じた後で止まっていることもある）。
        inner.develop.record_written(&ids, seq);
        drop(write);
        if let Some(s) = state.as_mut()
            && let Some((_, settings, _)) = applied.iter().find(|(v, _, _)| *v == s.variant_id)
        {
            s.settings = settings.clone();
            s.saved = settings.clone();
            s.saved_seq = seq;
            s.generation = inner.develop.next_generation();
            submit_render(inner, s, RenderQuality::Final);
        }
        drop(state);
        after_develop_saved(inner, &ids);
        Ok(ids)
    }

    /// 仮想コピーを作る（LIB-13。元の variant の現在の現像設定をコピーする）。作った仮想コピーは元の variant と
    /// 同じスタック（LIB-12）に入る（genzo-catalog の `Catalog::create_virtual_copy`）ので、カタログの変更の
    /// 知らせ（[`CatalogChange::VariantAdded`]）には、そのスタックの写真をすべて挙げる（印が変わるため）。
    pub fn create_virtual_copy(
        &self,
        variant_id: VariantId,
        name: Option<&str>,
    ) -> Result<VariantId, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        // 現像中の写真の保存待ちの変更を含めてコピーする。
        self.flush_develop()?;
        let (new_id, stacked) = inner.with_catalog(|c| {
            let id = c.create_virtual_copy(variant_id, name)?;
            let members: Vec<VariantId> = c
                .stacks_of_variants(&[id])?
                .into_iter()
                .flat_map(|s| s.members)
                .collect();
            Ok((id, members))
        })?;
        // サムネイルは元の variant のものを写しておき、現像結果から作り直す。
        if let Ok(Some(t)) = inner.with_cache(|c| c.thumbs.get(variant_id)) {
            let _ = inner.with_cache(|c| c.thumbs.put(new_id, &t.cache_key, &t.jpeg));
        }
        let mut changed = stacked;
        if !changed.contains(&new_id) {
            changed.push(new_id);
        }
        inner.events.emit(Event::CatalogChanged {
            change: CatalogChange::VariantAdded,
            variant_ids: changed,
            all: false,
        });
        crate::search::refresh(inner, &[]);
        schedule_regenerate(inner, new_id);
        Ok(new_id)
    }

    /// 切り抜きの編集中の表示を切り替える。設定の世代（描き直したなら新しい世代）を返す。
    ///
    /// - `uncropped` が真の間、現像のプレビュー（[`Core::develop_frame`]・[`Event::PreviewUpdated`]）と
    ///   表示範囲の描画（[`Core::render_region`]）は、設定の切り抜きを使わず、**切り抜く前の全体**（G の
    ///   外接矩形。`CropRect::FULL` の範囲。回転は今の設定の角度）を描く。UI は、その上に設定の切り抜き
    ///   （G の外接矩形の各軸の割合）をそのまま重ねられる（[`FrameInfo::uncropped`]）。
    /// - 回転で画像の外になった四隅は黒（0。genzo-pipeline の `OUTSIDE_IMAGE_FILL`。仮置き）で塗り、
    ///   ヒストグラムには数えない。
    /// - GPU 版はまだこの表示に対応していないので、この間のプレビューは CPU 版で描く。
    /// - 設定（保存・履歴・書き出し・サムネイル）は変えない。写真を開き直す（[`Core::open_develop`]）と
    ///   `false` に戻る。値が変われば描き直す（世代を進める）。
    /// - 写真を開いていなければ [`ApiError::NoDevelopSession`]。
    pub fn set_develop_view(&self, uncropped: bool) -> Result<u64, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let mut state = inner.develop.state.lock();
        let s = state.as_mut().ok_or(ApiError::NoDevelopSession)?;
        if s.uncropped != uncropped {
            s.uncropped = uncropped;
            redraw(inner, s);
            inner.develop.cv.notify_all();
        }
        Ok(s.generation)
    }

    /// WB のスポイト（DEV-03）: 現像中の写真の、プレビューの上の点 `(x, y)` のまわりを中立にする WB
    /// （色温度と tint）を求める。**適用はしない**（UI が `WhiteBalance::Custom` として
    /// [`Core::set_settings`] する）。
    ///
    /// - `x`・`y`: UI が表示しているプレビュー（[`FrameInfo`] の画像）の幅・高さに対する割合（0〜1。左上が
    ///   (0, 0)）。ふだんは出力の画像（C。切り抜いた後）。切り抜く前の全体の表示
    ///   （[`Core::set_develop_view`]。[`FrameInfo::uncropped`] が真）のプレビューなら、その全体の画像に
    ///   対する割合（UI が表示中のプレビューの上でクリックした位置をそのまま送れるように）。
    /// - `uncropped`: クリックした画像（UI が表示しているプレビュー）の [`FrameInfo::uncropped`]。`x`・`y` を
    ///   どちらの画像の割合として解釈するかを決める（今の表示とは違ってよい）。表示を切り替えた直後は、
    ///   新しい表示のプレビューが届くまで UI は前の表示の画像を見せているので、UI はこれを送る。`None` なら
    ///   最新に公開したプレビュー（[`Core::develop_frame`]）の表示（まだなければ今の表示）とみなす。この
    ///   ときは、新しいプレビューを公開してから UI が描き直すまでの短い間にクリックした点を、別の画像の
    ///   割合として解釈しうる。
    /// - 今の設定（保存待ち・ドラッグ中の値を含む）の段階 A1 の出力から求める。今の WB によらず、同じ点なら
    ///   同じ値になる。平均する範囲（見える範囲の長辺の 0.4%。仮置き）・計算の方法・範囲の外の値の丸めは
    ///   genzo-pipeline の `engine::pick` の doc（推定が設定できる範囲の外なら、範囲に丸めて返す）。
    ///
    /// エラー: 写真を開いていない・別の variant なら [`ApiError::NoDevelopSession`]。RAW 以外の写真（WB の
    /// 設定を使わない）、点が 0〜1 の外、切り抜く前の全体の表示で回転で画像の外になった四隅（黒く塗った
    /// 画素）の点、暗すぎる・色が極端な点は [`ApiError::InvalidArgument`]。切り抜いた後の画像では、端
    /// （0・1 ちょうど）の点も選べる。
    pub fn white_balance_at(
        &self,
        variant_id: VariantId,
        x: f64,
        y: f64,
        uncropped: Option<bool>,
    ) -> Result<PickedWhiteBalance, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let snap = session_snapshot(inner, variant_id)?;
        if !matches!(snap.source.image, SourceImage::Raw(_)) {
            return Err(ApiError::InvalidArgument(
                "RAW 以外の写真（JPEG など）は WB の設定を使わないため、WB のスポイトは使えません"
                    .to_owned(),
            ));
        }
        let view = pick_view(uncropped, snap.frame_uncropped, snap.uncropped);
        let pick = inner
            .engine
            .white_balance_at(&snap.source, &snap.settings, (x, y), view)
            .map_err(|e| match e {
                genzo_pipeline::PipelineError::InvalidArgument(m) => ApiError::InvalidArgument(m),
                e => e.into(),
            })?;
        Ok(PickedWhiteBalance {
            temperature_k: pick.temperature_k as f32,
            tint: pick.tint as f32,
        })
    }

    /// 切り抜き `crop` を、現像中の写真の寸法と角度 `angle_deg` で、描画が実際に使う範囲にする（回転で画像の
    /// 外に出る切り抜きは、中心と縦横比を保って画像の内側に収まる最大の大きさに縮める。genzo-pipeline の
    /// `finish::geometry::constrain_crop`。描画のステージ 9 と同じ計算）。設定は変えない。
    ///
    /// 値は保存するときと同じく丸めてから使う（`DevelopSettings::normalized`: 各軸を 0〜1 に収め、
    /// 左右・上下が逆なら入れ替え、幅・高さが最小値より小さければその軸を全体にする。角度は ±45 度に
    /// 収める）。NaN・無限大は [`ApiError::InvalidArgument`]。写真を開いていない・別の variant なら
    /// [`ApiError::NoDevelopSession`]。
    pub fn constrain_crop(
        &self,
        variant_id: VariantId,
        crop: &CropRect,
        angle_deg: f32,
    ) -> Result<CropRect, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let values = [crop.left, crop.top, crop.right, crop.bottom, angle_deg];
        if !values.iter().all(|v| v.is_finite()) {
            return Err(ApiError::InvalidArgument(format!(
                "切り抜き・角度に数でない値（NaN・無限大）があります（{crop:?}、{angle_deg}）"
            )));
        }
        let snap = session_snapshot(inner, variant_id)?;
        let mut s = DevelopSettings::default();
        s.geometry.crop = *crop;
        s.geometry.angle_deg = angle_deg;
        let g = s.normalized().geometry;
        let frames = snap
            .source
            .frames()?
            .with_rotation(f64::from(g.angle_deg))?;
        Ok(genzo_pipeline::finish::constrain_crop(&frames, &g.crop))
    }
}

/// 履歴の移動のしかた（[`Core::undo`]・[`Core::redo`]・[`Core::go_to_history`]）。
#[derive(Debug, Clone, Copy)]
enum HistoryMove {
    Undo,
    Redo,
    To(HistoryEntryId),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WB のスポイトの点は、UI が送った表示 → 最新に公開したプレビューの表示 → 今の表示の順に決めた画像の
    /// 割合として解釈する（表示を切り替えた直後に、UI がまだ見せている前の表示の画像の上の点を、新しい表示の
    /// 割合として解釈しない）。
    #[test]
    fn the_wb_pick_point_is_read_in_the_view_of_the_clicked_frame() {
        // 切り抜きの編集を終えた直後（今は切り抜いた表示、プレビューはまだ全体の表示）。
        assert!(pick_view(None, Some(true), false));
        // UI がクリックした画像の表示を送れば、それを使う（新しいプレビューの公開の後でも）。
        assert!(pick_view(Some(true), Some(false), false));
        assert!(!pick_view(Some(false), Some(true), true));
        // プレビューがまだなければ今の表示。
        assert!(pick_view(None, None, true));
        assert!(!pick_view(None, None, false));
    }

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
                lens_correction: None,
                lens_correction_note: None,
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
            uncropped: false,
            frame: None,
            before: None,
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

    /// state のロックを外して書く保存は、同じ variant のより新しい写しが書かれた後なら書かない（古い設定で
    /// 上書きしない）。間に別の variant を書いても、その記録は消えない（以前は全体で 1 件だけ覚えていて、
    /// 別の variant を書くと消えた）。
    #[test]
    fn older_snapshots_are_not_written_after_newer_ones() {
        let (a, b) = (VariantId::new(1), VariantId::new(2));
        let mut r = SaveRecord::default();
        // a の写し（番号 4）が、書く前で止まっている。
        r.begin(a);
        assert!(!r.has_newer(a, 4), "まだ何も書かれていない");
        r.written(&[a], 5);
        assert!(r.has_newer(a, 4), "a に 5 を書いた後の 4 は古い");
        assert!(!r.has_newer(a, 5), "同じ番号は古くない（自分自身）");
        assert!(!r.has_newer(a, 6));
        // 別の variant を書いても、a の記録は消えない。
        r.written(&[b], 9);
        assert!(
            r.has_newer(a, 4),
            "b を書いても、a の新しい写しは覚えている"
        );
        assert!(!r.has_newer(b, 1), "別の写真には関係しない");
        // 番号は戻らない（遅れて来た古い番号の記録で、新しい番号を忘れない）。
        r.written(&[a], 3);
        assert!(r.has_newer(a, 4));
        // 1 回で複数の variant を書いた場合（一括適用）。
        r.begin(b);
        r.written(&[a, b], 12);
        assert!(r.has_newer(a, 11) && r.has_newer(b, 11));
    }

    /// 追い越しの記録は、決める前の保存がある variant の分だけ持ち、保存がなくなれば捨てる（書いた variant の
    /// 数に応じて増え続けない。セッションを閉じる・variant を消すときの掃除も要らない）。
    #[test]
    fn save_record_only_keeps_variants_with_pending_saves() {
        let (a, b) = (VariantId::new(1), VariantId::new(2));
        let mut r = SaveRecord::default();
        // 決める前の保存がない variant への書き込みは、覚えない。
        r.written(&[a, b], 7);
        assert!(r.pending.is_empty());
        assert!(!r.has_newer(a, 1));
        // 同じ variant の保存が 2 つ決める前なら、両方が終わるまで覚えている。
        r.begin(a);
        r.begin(a);
        r.written(&[a], 8);
        r.end(a);
        assert!(r.has_newer(a, 6), "もう 1 つの保存が残っている");
        r.end(a);
        assert!(r.pending.is_empty(), "保存がなくなれば捨てる");
        // 捨てた後に同じ variant の保存を始めても、前の記録は残っていない（番号は新しい）。
        r.begin(a);
        assert!(!r.has_newer(a, 9));
        r.end(a);
        // 余分な end（登録のない variant）は何もしない。
        r.end(b);
        assert!(r.pending.is_empty());
    }

    /// 印（[`UnlockedSave`]）は、drop で（エラーで途中で抜けても）登録を外す。
    #[test]
    fn unlocked_save_marker_unregisters_on_drop() {
        let shared = DevelopShared::default();
        let a = VariantId::new(1);
        let seq = shared.next_save_seq();
        {
            let pending = shared.begin_unlocked_save(a, seq);
            assert!(!pending.is_superseded());
            shared.record_written(&[a], seq + 1);
            assert!(pending.is_superseded());
            assert_eq!(shared.save_record.lock().pending.len(), 1);
        }
        assert!(shared.save_record.lock().pending.is_empty());
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
        // ハイライト復元・ノイズ軽減（DEV-06・DEV-13）。
        let mut c = a.clone();
        c.highlight_recovery.method = genzo_model::HighlightMethod::Reconstruct;
        assert_eq!(describe_change(&a, &c), "ハイライト復元");
        let mut d = a.clone();
        d.detail.noise_reduction.luminance = 25.0;
        d.detail.noise_reduction.color = 40.0;
        assert_eq!(
            describe_change(&a, &d),
            "輝度ノイズの軽減 25・カラーノイズの軽減 40"
        );
        let mut e = a.clone();
        e.detail.noise_reduction.color_detail = 70.0;
        assert_eq!(describe_change(&a, &e), "ノイズ軽減");
        let mut f = a.clone();
        f.detail.sharpening.amount = 30.0;
        assert_eq!(describe_change(&a, &f), "シャープ 30");
        // テクスチャ・明瞭度・かすみの除去（DEV-11）・シャープ（DEV-12）・効果（DEV-24）。
        let mut p = a.clone();
        p.presence.texture = 20.0;
        p.presence.dehaze = -10.0;
        assert_eq!(describe_change(&a, &p), "テクスチャ +20・かすみの除去 -10");
        let mut q = a.clone();
        q.detail.sharpening.masking = 40.0;
        q.effects.vignette.amount = -30.0;
        q.effects.grain.size = 60.0;
        assert_eq!(describe_change(&a, &q), "シャープ・周辺光量 -30・粒子");
        // カラーミキサー・白黒・カラーグレーディング（DEV-09・DEV-25・DEV-10）。
        let mut g = a.clone();
        g.color.black_white.enabled = true;
        assert_eq!(describe_change(&a, &g), HISTORY_LABEL_BLACK_WHITE);
        assert_eq!(describe_change(&g, &a), HISTORY_LABEL_COLOR);
        let mut h = g.clone();
        h.color.black_white.mix.blue = -30.0;
        assert_eq!(describe_change(&g, &h), "白黒ミキサー");
        let mut i = a.clone();
        i.color.mixer.orange.luminance = 20.0;
        i.color.grading.shadows.saturation = 15.0;
        assert_eq!(
            describe_change(&a, &i),
            "カラーミキサー・カラーグレーディング"
        );
    }
}
