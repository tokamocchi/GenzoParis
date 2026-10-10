//! 計測の基盤（05 の M0 タスク 12、1.8 節「計測と記録のルール」、02 の MAINT-07）。
//!
//! # 使い方
//!
//! ```no_run
//! use std::time::Duration;
//! use genzo_testkit::bench::{Bench, BenchRecorder};
//!
//! // 応答時間（30 回以上）。目標 33ms（PERF-01）は 95 パーセンタイルで判定する。
//! let result = Bench::latency("exposure_drag")
//!     .target(Duration::from_millis(33))
//!     .run_warm(|| { /* 計測する処理 */ })?;
//! // bench-results/exposure_drag.json に追記し、前回の結果と比べる。
//! let report = BenchRecorder::from_env().record(&result)?;
//! println!("{report}");
//! assert!(!report.regressed());
//! # Ok::<(), genzo_testkit::bench::BenchError>(())
//! ```
//!
//! # 1.8 節のルールとの対応
//!
//! | ルール | この実装 |
//! |---|---|
//! | 入力: サンプルの ID とハッシュ、現像設定 | [`Bench::input`]・[`Bench::develop_settings`] |
//! | 環境: OS・GPU とドライバー・ライブラリ・ビルドの設定 | [`EnvironmentInfo`]（GPU とライブラリは呼び出し側が設定） |
//! | 条件: コールドとウォーム | [`Bench::run_cold`]（毎回、計測の前に状態を戻す関数を呼ぶ）・[`Bench::run_warm`]（ウォームアップの後に計測） |
//! | 回数: 応答時間は 30 回以上、一括処理は 5 回以上 | [`BenchKind`]。下回ると [`BenchResult::meets_iteration_rule`] が偽になり、合格にしない |
//! | 報告: 平均・95 パーセンタイル・最大、目標を超えた回数 | [`DurationStats`]（パーセンタイルの定義は [`crate::stats`]） |
//! | 応答時間は入力イベントから表示まで | 処理を呼ぶだけでは測れないので、UI 側で測った時間を [`Bench::from_samples`] で渡す |
//! | 時間の内訳（展開・転送・GPU・エンコード・書き込み） | [`Bench::run_warm_phased`] などの [`PhaseTimer`] |
//! | 目標に届かなかった場合は合格と扱わない | [`BenchReport::passed`] |
//!
//! 「コールド」は、渡した関数で状態（キャッシュなど）を戻してから測る、という意味である。
//! プロセスの起動直後の時間（OS のファイルキャッシュを含む）は、プロセスを起動し直して 1 回ずつ測り、
//! [`Bench::from_samples`] で渡す。

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::hint::black_box;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use genzo_model::DevelopSettings;
use serde::{Deserialize, Serialize};

use crate::env::{BuildProfile, EnvironmentInfo};
use crate::record::{InputRef, InvalidName, SettingsRecord, validate_name, write_atomic};
use crate::stats::Summary;

/// 応答時間の計測の最小回数（05 の 1.8 節）。
pub const LATENCY_MIN_ITERATIONS: u32 = 30;
/// 一括処理の計測の最小回数（05 の 1.8 節）。
pub const BATCH_MIN_ITERATIONS: u32 = 5;
/// 応答時間のウォームアップの既定の回数。**仮置き**: JIT やキャッシュの初回の影響を除く程度の
/// 少ない回数とした（根拠のある値ではない。結果がばらつくなら増やす）。
pub const DEFAULT_LATENCY_WARMUP: u32 = 3;
/// 一括処理のウォームアップの既定の回数。**仮置き**（[`DEFAULT_LATENCY_WARMUP`] と同じ考え方で、
/// 1 回の時間が長いので 1 回とした）。
pub const DEFAULT_BATCH_WARMUP: u32 = 1;
/// 悪化とみなす 95 パーセンタイルの増加率の既定値（10%）。**仮置き**: 計測のばらつきより大きく、
/// 体感できる差より小さい値として選んだ。CI の結果のばらつきを見て見直す。
pub const DEFAULT_P95_REGRESSION_RATIO: f64 = 0.10;
/// 増加率を比べるときの許容誤差（ちょうど 10% の悪化を浮動小数点の誤差で見逃さないため）。
const RATIO_EPSILON: f64 = 1e-9;
/// 結果の出力先を指定する環境変数。
pub const BENCH_DIR_ENV: &str = "GENZO_BENCH_DIR";
/// 結果の出力先の既定値（カレントディレクトリからの相対パス）。
pub const DEFAULT_BENCH_DIR: &str = "bench-results";
/// 結果のファイルの形式のバージョン。
pub const BENCH_FILE_VERSION: u32 = 1;

/// 計測の種類（回数の規則が違う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchKind {
    /// 応答時間（30 回以上）。
    Latency,
    /// 一括処理（5 回以上）。
    Batch,
}

impl BenchKind {
    /// 最小の計測の回数。
    pub const fn min_iterations(self) -> u32 {
        match self {
            BenchKind::Latency => LATENCY_MIN_ITERATIONS,
            BenchKind::Batch => BATCH_MIN_ITERATIONS,
        }
    }

    /// 既定のウォームアップの回数。
    pub const fn default_warmup(self) -> u32 {
        match self {
            BenchKind::Latency => DEFAULT_LATENCY_WARMUP,
            BenchKind::Batch => DEFAULT_BATCH_WARMUP,
        }
    }
}

/// 計測の条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    /// コールド（キャッシュなどを戻してから測る）。
    Cold,
    /// ウォーム（ウォームアップの後に測る）。
    Warm,
}

impl fmt::Display for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Condition::Cold => "コールド",
            Condition::Warm => "ウォーム",
        })
    }
}

/// 計測のエラー。
#[derive(Debug, thiserror::Error)]
pub enum BenchError {
    /// 名前が不正。
    #[error(transparent)]
    InvalidName(#[from] InvalidName),
    /// 引数が不正。
    #[error("計測の引数が不正です: {0}")]
    InvalidArgument(String),
    /// 結果のファイルが壊れている・形式が違う（上書きしない）。
    #[error("計測の結果のファイル {} が不正です（上書きしません）: {reason}", path.display())]
    Corrupt {
        /// パス。
        path: PathBuf,
        /// 理由。
        reason: String,
    },
    /// 入出力のエラー。
    #[error("{}: {source}", path.display())]
    Io {
        /// パス。
        path: PathBuf,
        /// 元のエラー。
        source: io::Error,
    },
}

/// 1 回の計測の中の時間の内訳（05 の 1.8 節「時間の内訳」）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhaseTimer {
    entries: Vec<(String, Duration)>,
}

impl PhaseTimer {
    /// 空の内訳。
    pub fn new() -> Self {
        Self::default()
    }

    /// 関数の実行時間を `name` の内訳として記録する。
    pub fn time<T>(&mut self, name: &str, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let out = f();
        self.record(name, start.elapsed());
        out
    }

    /// 測った時間を `name` の内訳として記録する（同じ名前は 1 回の計測の中で合計する）。
    pub fn record(&mut self, name: &str, duration: Duration) {
        self.entries.push((name.to_owned(), duration));
    }

    /// 名前ごとの合計。
    fn totals(&self) -> BTreeMap<&str, Duration> {
        let mut m = BTreeMap::new();
        for (name, d) in &self.entries {
            *m.entry(name.as_str()).or_insert(Duration::ZERO) += *d;
        }
        m
    }
}

/// 時間の統計（ミリ秒）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DurationStats {
    /// 回数。
    pub count: u32,
    /// 平均。
    pub mean_ms: f64,
    /// 最小。
    pub min_ms: f64,
    /// 中央値（最近接順位法）。
    pub median_ms: f64,
    /// 95 パーセンタイル（最近接順位法）。
    pub p95_ms: f64,
    /// 最大。
    pub max_ms: f64,
    /// 目標の時間を超えた（`>`）回数（目標がなければ `None`）。
    #[serde(default)]
    pub over_target: Option<u32>,
}

impl DurationStats {
    /// ミリ秒の値の列から計算する。空、または有限でない値を含むなら `None`。
    pub fn from_ms(samples_ms: &[f64], target_ms: Option<f64>) -> Option<Self> {
        let s = Summary::from_values(samples_ms)?;
        Some(Self {
            count: u32::try_from(s.count).unwrap_or(u32::MAX),
            mean_ms: s.mean,
            min_ms: s.min,
            median_ms: s.median,
            p95_ms: s.p95,
            max_ms: s.max,
            over_target: target_ms.map(|t| {
                u32::try_from(samples_ms.iter().filter(|&&v| v > t).count()).unwrap_or(u32::MAX)
            }),
        })
    }
}

fn to_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// 1 回の計測の結果（結果のファイルの 1 件）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchResult {
    /// 名前。
    pub name: String,
    /// 種類。
    pub kind: BenchKind,
    /// 条件。
    pub condition: Condition,
    /// 計測した時刻（UTC、RFC 3339）。
    pub timestamp_utc: String,
    /// ウォームアップの回数。
    pub warmup: u32,
    /// 計測の回数。
    pub iterations: u32,
    /// 回数が 1.8 節の規則（応答時間 30 回以上・一括処理 5 回以上）を満たすか。
    pub meets_iteration_rule: bool,
    /// 目標の時間（ミリ秒。95 パーセンタイルで判定する）。
    #[serde(default)]
    pub target_ms: Option<f64>,
    /// 全体の時間の統計。
    pub stats: DurationStats,
    /// 全体の時間（ミリ秒、計測の順）。後で別の統計を計算できるよう残す。
    pub samples_ms: Vec<f64>,
    /// 時間の内訳の統計（名前ごと）。
    #[serde(default)]
    pub phases: BTreeMap<String, DurationStats>,
    /// 環境。
    pub environment: EnvironmentInfo,
    /// 入力の ID とハッシュ。
    #[serde(default)]
    pub inputs: Vec<InputRef>,
    /// 使った現像設定。
    #[serde(default)]
    pub develop: Option<SettingsRecord>,
    /// その他の条件（解像度など）。
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
}

impl BenchResult {
    /// 目標を満たしたか（95 パーセンタイル ≤ 目標）。目標がなければ `None`。
    pub fn target_met(&self) -> Option<bool> {
        self.target_ms.map(|t| self.stats.p95_ms <= t)
    }
}

impl fmt::Display for BenchResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = &self.stats;
        write!(
            f,
            "{}（{:?}・{}、{} 回）: 平均 {:.3} ms、95 パーセンタイル {:.3} ms、最大 {:.3} ms",
            self.name, self.kind, self.condition, self.iterations, s.mean_ms, s.p95_ms, s.max_ms
        )?;
        if let (Some(t), Some(over)) = (self.target_ms, s.over_target) {
            let verdict = if self.target_met() == Some(true) {
                "達成"
            } else {
                "未達"
            };
            write!(f, "、目標 {t:.3} ms（{verdict}）を超えた回数 {over}")?;
        }
        for (name, p) in &self.phases {
            write!(
                f,
                "\n  内訳 {name}: 平均 {:.3} ms、95 パーセンタイル {:.3} ms、最大 {:.3} ms",
                p.mean_ms, p.p95_ms, p.max_ms
            )?;
        }
        Ok(())
    }
}

/// 計測の設定。
#[derive(Debug, Clone)]
pub struct Bench {
    name: String,
    kind: BenchKind,
    warmup: u32,
    iterations: u32,
    target: Option<Duration>,
    environment: Option<EnvironmentInfo>,
    inputs: Vec<InputRef>,
    develop: Option<SettingsRecord>,
    notes: BTreeMap<String, String>,
}

impl Bench {
    /// 種類を指定して作る（回数とウォームアップは種類の既定値）。
    pub fn new(name: impl Into<String>, kind: BenchKind) -> Self {
        Self {
            name: name.into(),
            kind,
            warmup: kind.default_warmup(),
            iterations: kind.min_iterations(),
            target: None,
            environment: None,
            inputs: Vec::new(),
            develop: None,
            notes: BTreeMap::new(),
        }
    }

    /// 応答時間の計測（30 回、ウォームアップ 3 回）。
    pub fn latency(name: impl Into<String>) -> Self {
        Self::new(name, BenchKind::Latency)
    }

    /// 一括処理の計測（5 回、ウォームアップ 1 回）。
    pub fn batch(name: impl Into<String>) -> Self {
        Self::new(name, BenchKind::Batch)
    }

    /// ウォームアップの回数（ウォームの計測だけで使う）。
    pub fn warmup(mut self, n: u32) -> Self {
        self.warmup = n;
        self
    }

    /// 計測の回数（1 以上。規則の最小値を下回ると結果を合格にしない）。
    pub fn iterations(mut self, n: u32) -> Self {
        self.iterations = n;
        self
    }

    /// 目標の時間。
    pub fn target(mut self, target: Duration) -> Self {
        self.target = Some(target);
        self
    }

    /// 環境の情報（指定しなければ計測のときに [`EnvironmentInfo::detect`] で取る）。
    pub fn environment(mut self, env: EnvironmentInfo) -> Self {
        self.environment = Some(env);
        self
    }

    /// 入力を追加する。
    pub fn input(mut self, input: InputRef) -> Self {
        self.inputs.push(input);
        self
    }

    /// 使った現像設定を記録する。
    pub fn develop_settings(mut self, settings: &DevelopSettings) -> Self {
        self.develop = Some(SettingsRecord::from_settings(settings));
        self
    }

    /// その他の条件（例: `"resolution" → "2560x1707"`）を記録する。
    ///
    /// 条件が違う結果は前回の結果として比べない（[`find_previous`]）。比較に関係しない情報
    /// （電源の状態など）は [`EnvironmentInfo::with_extra`] に入れる。
    pub fn note(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.notes.insert(key.into(), value.into());
        self
    }

    /// 名前。
    pub fn name(&self) -> &str {
        &self.name
    }

    fn check(&self) -> Result<(), BenchError> {
        validate_name(&self.name)?;
        if self.iterations == 0 {
            return Err(BenchError::InvalidArgument(
                "計測の回数は 1 以上が必要です".to_owned(),
            ));
        }
        Ok(())
    }

    /// ウォームの計測: ウォームアップ（計測しない）の後に `iterations` 回測る。
    pub fn run_warm<T>(&self, mut f: impl FnMut() -> T) -> Result<BenchResult, BenchError> {
        self.run(Condition::Warm, &mut || {}, &mut |_| {
            black_box(f());
        })
    }

    /// コールドの計測: 毎回 `reset`（計測しない）で状態を戻してから測る。ウォームアップはしない。
    pub fn run_cold<T>(
        &self,
        mut reset: impl FnMut(),
        mut f: impl FnMut() -> T,
    ) -> Result<BenchResult, BenchError> {
        self.run(Condition::Cold, &mut reset, &mut |_| {
            black_box(f());
        })
    }

    /// 内訳つきのウォームの計測。
    pub fn run_warm_phased<T>(
        &self,
        mut f: impl FnMut(&mut PhaseTimer) -> T,
    ) -> Result<BenchResult, BenchError> {
        self.run(Condition::Warm, &mut || {}, &mut |t| {
            black_box(f(t));
        })
    }

    /// 内訳つきのコールドの計測。
    pub fn run_cold_phased<T>(
        &self,
        mut reset: impl FnMut(),
        mut f: impl FnMut(&mut PhaseTimer) -> T,
    ) -> Result<BenchResult, BenchError> {
        self.run(Condition::Cold, &mut reset, &mut |t| {
            black_box(f(t));
        })
    }

    fn run(
        &self,
        condition: Condition,
        reset: &mut dyn FnMut(),
        f: &mut dyn FnMut(&mut PhaseTimer),
    ) -> Result<BenchResult, BenchError> {
        self.check()?;
        if condition == Condition::Warm {
            for _ in 0..self.warmup {
                f(&mut PhaseTimer::new());
            }
        }
        let mut samples = Vec::with_capacity(self.iterations as usize);
        let mut phases = Vec::with_capacity(self.iterations as usize);
        for _ in 0..self.iterations {
            if condition == Condition::Cold {
                reset();
            }
            let mut timer = PhaseTimer::new();
            let start = Instant::now();
            f(&mut timer);
            samples.push(start.elapsed());
            phases.push(timer);
        }
        let warmup = if condition == Condition::Warm {
            self.warmup
        } else {
            0
        };
        self.build(condition, warmup, &samples, &phases)
    }

    /// 外で測った時間から結果を作る（UI で測った入力から表示までの時間、プロセスを起動し直して
    /// 測ったコールドの時間など）。`phases` は空か、`samples` と同じ数。回数は `samples` の数。
    pub fn from_samples(
        &self,
        condition: Condition,
        samples: &[Duration],
        phases: &[PhaseTimer],
    ) -> Result<BenchResult, BenchError> {
        validate_name(&self.name)?;
        if samples.is_empty() {
            return Err(BenchError::InvalidArgument("計測の結果が空です".to_owned()));
        }
        if !phases.is_empty() && phases.len() != samples.len() {
            return Err(BenchError::InvalidArgument(format!(
                "内訳の数 {} が計測の回数 {} と一致しません",
                phases.len(),
                samples.len()
            )));
        }
        self.build(condition, 0, samples, phases)
    }

    fn build(
        &self,
        condition: Condition,
        warmup: u32,
        samples: &[Duration],
        phases: &[PhaseTimer],
    ) -> Result<BenchResult, BenchError> {
        let iterations = u32::try_from(samples.len())
            .map_err(|_| BenchError::InvalidArgument("計測の回数が多すぎます".to_owned()))?;
        let target_ms = self.target.map(to_ms);
        let samples_ms: Vec<f64> = samples.iter().copied().map(to_ms).collect();
        let stats = DurationStats::from_ms(&samples_ms, target_ms)
            .ok_or_else(|| BenchError::InvalidArgument("計測の結果が空です".to_owned()))?;
        let mut per_phase: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for timer in phases {
            for (name, d) in timer.totals() {
                per_phase.entry(name.to_owned()).or_default().push(to_ms(d));
            }
        }
        let phases = per_phase
            .into_iter()
            .filter_map(|(name, v)| DurationStats::from_ms(&v, None).map(|s| (name, s)))
            .collect();
        Ok(BenchResult {
            name: self.name.clone(),
            kind: self.kind,
            condition,
            timestamp_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            warmup,
            iterations,
            meets_iteration_rule: iterations >= self.kind.min_iterations(),
            target_ms,
            stats,
            samples_ms,
            phases,
            environment: self
                .environment
                .clone()
                .unwrap_or_else(EnvironmentInfo::detect),
            inputs: self.inputs.clone(),
            develop: self.develop.clone(),
            notes: self.notes.clone(),
        })
    }
}

/// 悪化の判定の基準。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegressionPolicy {
    /// 95 パーセンタイルの増加率（前回比）がこの値以上なら悪化とする（既定 0.10 = 10%。仮置き）。
    pub p95_ratio: f64,
}

impl Default for RegressionPolicy {
    fn default() -> Self {
        Self {
            p95_ratio: DEFAULT_P95_REGRESSION_RATIO,
        }
    }
}

impl RegressionPolicy {
    /// 値が正しいか（増加率は 0 以上の有限の値）。NaN だと悪化を一度も検出せず、負の値だと
    /// 速くなっても悪化と判定するため、記録の前に確かめる。
    pub fn validate(&self) -> Result<(), BenchError> {
        if self.p95_ratio.is_finite() && self.p95_ratio >= 0.0 {
            Ok(())
        } else {
            Err(BenchError::InvalidArgument(format!(
                "悪化の判定の増加率は 0 以上の有限の値が必要です（{}）",
                self.p95_ratio
            )))
        }
    }
}

/// 前回の結果との比較。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchComparison {
    /// 前回の計測の時刻。
    pub previous_timestamp_utc: String,
    /// 前回の 95 パーセンタイル（ミリ秒）。
    pub previous_p95_ms: f64,
    /// 今回の 95 パーセンタイル（ミリ秒）。
    pub current_p95_ms: f64,
    /// 95 パーセンタイルの増加率（(今回 − 前回) / 前回。前回が 0 なら `None`）。
    pub p95_change: Option<f64>,
    /// 前回の平均（ミリ秒）。
    pub previous_mean_ms: f64,
    /// 今回の平均（ミリ秒）。
    pub current_mean_ms: f64,
    /// 平均の増加率（参考。悪化の判定には使わない）。
    pub mean_change: Option<f64>,
    /// 悪化したか（95 パーセンタイルの増加率 ≥ 基準）。
    pub regressed: bool,
}

fn change(previous: f64, current: f64) -> Option<f64> {
    (previous > 0.0).then(|| (current - previous) / previous)
}

/// 2 つの結果を比べる（環境が比べられるかは確かめない。[`find_previous`] で選んだものを渡す）。
///
/// `policy` は [`RegressionPolicy::validate`] で確かめたものを渡す（NaN なら悪化と判定しない）。
pub fn compare_results(
    previous: &BenchResult,
    current: &BenchResult,
    policy: &RegressionPolicy,
) -> BenchComparison {
    let p95_change = change(previous.stats.p95_ms, current.stats.p95_ms);
    BenchComparison {
        previous_timestamp_utc: previous.timestamp_utc.clone(),
        previous_p95_ms: previous.stats.p95_ms,
        current_p95_ms: current.stats.p95_ms,
        p95_change,
        previous_mean_ms: previous.stats.mean_ms,
        current_mean_ms: current.stats.mean_ms,
        mean_change: change(previous.stats.mean_ms, current.stats.mean_ms),
        regressed: p95_change.is_some_and(|c| c >= policy.p95_ratio - RATIO_EPSILON),
    }
}

/// 比べる相手（前回の結果）を選ぶ: 履歴の新しいほうから、同じ名前・種類・条件（コールドか
/// ウォームか）で、環境が比べられ（[`EnvironmentInfo::comparable_with`]）、回数の規則を満たし、
/// 入力（ID とハッシュ）・現像設定（[`SettingsRecord::develop_hash`] と処理バージョン）・
/// その他の条件（[`BenchResult::notes`]。解像度など）が同じ最初の結果。
///
/// 入力や解像度の違う結果と比べると、処理の悪化と入力の違いを区別できない（05 の 1.8 節
/// 「入力」「解像度」）ため、比べない。
pub fn find_previous<'a>(
    history: &'a [BenchResult],
    current: &BenchResult,
) -> Option<&'a BenchResult> {
    let settings_key = |r: &BenchResult| {
        r.develop
            .as_ref()
            .map(|d| (d.develop_hash.clone(), d.process_version))
    };
    history.iter().rev().find(|r| {
        r.name == current.name
            && r.kind == current.kind
            && r.condition == current.condition
            && r.meets_iteration_rule
            && r.environment.comparable_with(&current.environment)
            && r.inputs == current.inputs
            && settings_key(r) == settings_key(current)
            && r.notes == current.notes
    })
}

/// 結果のファイル（`<出力先>/<名前>.json`）の中身。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct BenchFile {
    format_version: u32,
    name: String,
    runs: Vec<BenchResult>,
}

/// 記録と比較の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct BenchReport {
    /// 結果のファイル。
    pub path: PathBuf,
    /// 今回の結果。
    pub result: BenchResult,
    /// 前回の結果との比較（比べられる前回の結果がなければ `None`）。
    pub comparison: Option<BenchComparison>,
}

impl BenchReport {
    /// 前回より悪化したか。
    pub fn regressed(&self) -> bool {
        self.comparison.as_ref().is_some_and(|c| c.regressed)
    }

    /// 合格か: 回数の規則を満たし、目標があれば達成し、悪化していない（05 の 1.8 節
    /// 「未達の項目を合格と扱わない」）。
    pub fn passed(&self) -> bool {
        self.result.meets_iteration_rule
            && self.result.target_met() != Some(false)
            && !self.regressed()
    }
}

impl fmt::Display for BenchReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.result)?;
        match &self.comparison {
            Some(c) => {
                let pct = |v: Option<f64>| match v {
                    Some(v) => format!("{:+.1}%", v * 100.0),
                    None => "—".to_owned(),
                };
                write!(
                    f,
                    "\n  前回（{}）との比較: 95 パーセンタイル {:.3} → {:.3} ms（{}）、平均 {:.3} → {:.3} ms（{}）",
                    c.previous_timestamp_utc,
                    c.previous_p95_ms,
                    c.current_p95_ms,
                    pct(c.p95_change),
                    c.previous_mean_ms,
                    c.current_mean_ms,
                    pct(c.mean_change)
                )?;
                if c.regressed {
                    write!(f, "\n  ※ 悪化しています（95 パーセンタイル）")?;
                }
            }
            None => write!(f, "\n  比べられる前回の結果はありません")?,
        }
        if !self.result.meets_iteration_rule {
            write!(
                f,
                "\n  ※ 計測の回数 {} が 1.8 節の規則（{} 回以上）を満たしません",
                self.result.iterations,
                self.result.kind.min_iterations()
            )?;
        }
        if self.result.environment.build_profile == BuildProfile::Debug {
            write!(f, "\n  ※ debug ビルドの計測です（時間は参考値）")?;
        }
        write!(f, "\n  記録: {}", self.path.display())
    }
}

/// 同じプロセスの中で、結果のファイルの読み込み → 追記 → 書き込みを 1 つずつ行うための錠。
///
/// テストは同じプロセスの複数のスレッドで並列に動くため、錠がないと同じ名前の記録が重なって
/// 片方の結果が失われる。別のプロセスどうしの競合は防げない（[`BenchRecorder::record`]）。
static RECORD_LOCK: Mutex<()> = Mutex::new(());

/// 結果の記録先。
#[derive(Debug, Clone)]
pub struct BenchRecorder {
    dir: PathBuf,
    policy: RegressionPolicy,
}

impl BenchRecorder {
    /// 出力先を指定して作る。
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            policy: RegressionPolicy::default(),
        }
    }

    /// 出力先を環境変数 `GENZO_BENCH_DIR`（なければ `bench-results`）にして作る。
    pub fn from_env() -> Self {
        let dir = std::env::var_os(BENCH_DIR_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_BENCH_DIR));
        Self::new(dir)
    }

    /// 悪化の判定の基準を指定する。
    pub fn with_policy(mut self, policy: RegressionPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// 出力先。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 結果のファイルのパス（`<出力先>/<名前>.json`）。
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.json"))
    }

    /// これまでの結果（古い順）。ファイルがなければ空。
    pub fn load(&self, name: &str) -> Result<Vec<BenchResult>, BenchError> {
        validate_name(name)?;
        let path = self.path(name);
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(BenchError::Io { path, source: e }),
        };
        let file: BenchFile = serde_json::from_str(&text).map_err(|e| BenchError::Corrupt {
            path: path.clone(),
            reason: format!("JSON を解析できません: {e}"),
        })?;
        if file.format_version != BENCH_FILE_VERSION {
            return Err(BenchError::Corrupt {
                path,
                reason: format!(
                    "対応していない形式のバージョンです（{}。対応しているのは {BENCH_FILE_VERSION}）",
                    file.format_version
                ),
            });
        }
        if file.name != name {
            return Err(BenchError::Corrupt {
                path,
                reason: format!("名前 {:?} がファイル名と一致しません", file.name),
            });
        }
        Ok(file.runs)
    }

    /// 結果を追記し、前回の結果と比べる。
    ///
    /// ファイルは一時ファイルに書いてから置き換える。既存のファイルが壊れている場合は上書きせずに
    /// エラーにする。同じプロセスの中の記録は 1 つずつ行う（並列のテストでも結果を失わない）が、
    /// 同時に **複数のプロセス** から同じ名前で記録すると、片方の結果が失われうる。
    pub fn record(&self, result: &BenchResult) -> Result<BenchReport, BenchError> {
        self.policy.validate()?;
        // 錠を持っていたスレッドがパニックしても、ファイルは一時ファイル経由で置き換えるので
        // 壊れていない。続けて使う。
        let _guard = RECORD_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut runs = self.load(&result.name)?;
        let comparison =
            find_previous(&runs, result).map(|prev| compare_results(prev, result, &self.policy));
        runs.push(result.clone());
        let file = BenchFile {
            format_version: BENCH_FILE_VERSION,
            name: result.name.clone(),
            runs,
        };
        let path = self.path(&result.name);
        let mut json = serde_json::to_string_pretty(&file).map_err(|e| BenchError::Corrupt {
            path: path.clone(),
            reason: format!("JSON にできません: {e}"),
        })?;
        json.push('\n');
        write_atomic(&path, json.as_bytes()).map_err(|e| BenchError::Io {
            path: path.clone(),
            source: e,
        })?;
        Ok(BenchReport {
            path,
            result: result.clone(),
            comparison,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn ms(v: &[u64]) -> Vec<Duration> {
        v.iter().map(|&x| Duration::from_millis(x)).collect()
    }

    fn fixed_env() -> EnvironmentInfo {
        EnvironmentInfo::detect()
    }

    /// 1〜30 ms の 30 回（決定的な入力）。
    fn result_1_to_30(name: &str, scale: u64) -> BenchResult {
        let samples: Vec<u64> = (1..=30).map(|v| v * scale).collect();
        Bench::latency(name)
            .target(Duration::from_millis(25 * scale))
            .environment(fixed_env())
            .from_samples(Condition::Warm, &ms(&samples), &[])
            .unwrap()
    }

    #[test]
    fn stats_of_deterministic_samples() {
        let r = result_1_to_30("x", 1);
        let s = &r.stats;
        assert_eq!(s.count, 30);
        assert!((s.mean_ms - 15.5).abs() < 1e-9);
        assert_eq!(s.min_ms, 1.0);
        // 最近接順位法: 中央値は順位 15、95 パーセンタイルは順位 29。
        assert_eq!(s.median_ms, 15.0);
        assert_eq!(s.p95_ms, 29.0);
        assert_eq!(s.max_ms, 30.0);
        // 目標 25 ms を超えた（26〜30）回数は 5。ちょうど 25 は超えていない。
        assert_eq!(s.over_target, Some(5));
        assert_eq!(r.target_met(), Some(false));
        assert!(r.meets_iteration_rule);
        assert_eq!(r.iterations, 30);
        assert_eq!(r.samples_ms.len(), 30);
        assert_eq!(r.warmup, 0);
        let text = r.to_string();
        assert!(text.contains("95 パーセンタイル 29.000 ms"), "{text}");
        // 目標も他の値と同じ桁数で出す（1/15 秒の 66.666666… をそのまま出さない）。
        assert!(text.contains("目標 25.000 ms（未達）"), "{text}");
    }

    #[test]
    fn target_met_uses_p95() {
        let r = Bench::batch("b")
            .target(Duration::from_millis(100))
            .environment(fixed_env())
            .from_samples(Condition::Cold, &ms(&[10, 20, 30, 40, 500]), &[])
            .unwrap();
        // 5 回の 95 パーセンタイルは順位 5（最大）= 500 なので未達。
        assert_eq!(r.stats.p95_ms, 500.0);
        assert_eq!(r.target_met(), Some(false));
        assert!(r.meets_iteration_rule);
        let r = Bench::batch("b")
            .environment(fixed_env())
            .from_samples(Condition::Cold, &ms(&[10, 20, 30, 40, 50]), &[])
            .unwrap();
        assert_eq!(r.target_met(), None);
        assert_eq!(r.stats.over_target, None);
    }

    #[test]
    fn iteration_rule() {
        let r = Bench::latency("few")
            .environment(fixed_env())
            .from_samples(Condition::Warm, &ms(&[1; 29]), &[])
            .unwrap();
        assert!(!r.meets_iteration_rule);
        let r = Bench::batch("few")
            .environment(fixed_env())
            .from_samples(Condition::Warm, &ms(&[1; 4]), &[])
            .unwrap();
        assert!(!r.meets_iteration_rule);
        assert_eq!(BenchKind::Latency.min_iterations(), 30);
        assert_eq!(BenchKind::Batch.min_iterations(), 5);
        assert!(matches!(
            Bench::latency("z").iterations(0).run_warm(|| ()),
            Err(BenchError::InvalidArgument(_))
        ));
        assert!(matches!(
            Bench::latency("z").from_samples(Condition::Warm, &[], &[]),
            Err(BenchError::InvalidArgument(_))
        ));
        assert!(matches!(
            Bench::latency("a/b").run_warm(|| ()),
            Err(BenchError::InvalidName(_))
        ));
    }

    #[test]
    fn warm_runs_warmup_then_iterations() {
        let calls = Cell::new(0u32);
        let r = Bench::latency("warm")
            .warmup(4)
            .iterations(30)
            .environment(fixed_env())
            .run_warm(|| calls.set(calls.get() + 1))
            .unwrap();
        assert_eq!(calls.get(), 34);
        assert_eq!(r.condition, Condition::Warm);
        assert_eq!(r.warmup, 4);
        assert_eq!(r.iterations, 30);
        assert_eq!(r.samples_ms.len(), 30);
    }

    #[test]
    fn cold_resets_before_each_iteration_without_warmup() {
        // reset と計測が交互に呼ばれ、ウォームアップはしない。
        let log = std::cell::RefCell::new(Vec::new());
        let r = Bench::batch("cold")
            .warmup(10)
            .environment(fixed_env())
            .run_cold(|| log.borrow_mut().push('r'), || log.borrow_mut().push('f'))
            .unwrap();
        assert_eq!(log.borrow().iter().collect::<String>(), "rfrfrfrfrf");
        assert_eq!(r.condition, Condition::Cold);
        assert_eq!(r.warmup, 0);
        assert_eq!(r.iterations, 5);
    }

    #[test]
    fn measured_time_includes_work_but_not_reset() {
        let r = Bench::batch("sleep")
            .environment(fixed_env())
            .run_cold(
                || std::thread::sleep(Duration::from_millis(30)),
                || std::thread::sleep(Duration::from_millis(2)),
            )
            .unwrap();
        // 計測は 2 ms 以上（sleep は指定より短くならない）。reset の 30 ms は含まない
        // （上限はスケジューラの遅れを見込んで緩くする）。
        assert!(r.stats.min_ms >= 2.0, "{}", r.stats.min_ms);
        assert!(r.stats.median_ms < 30.0, "{}", r.stats.median_ms);
    }

    #[test]
    fn phases_are_aggregated_per_name() {
        let phases: Vec<PhaseTimer> = (1..=5)
            .map(|i| {
                let mut t = PhaseTimer::new();
                t.record("decode", Duration::from_millis(10 * i));
                t.record("encode", Duration::from_millis(1));
                // 同じ名前は 1 回の中で合計する。
                t.record("encode", Duration::from_millis(2));
                t
            })
            .collect();
        let r = Bench::batch("phased")
            .environment(fixed_env())
            .from_samples(Condition::Warm, &ms(&[20, 30, 40, 50, 60]), &phases)
            .unwrap();
        let d = &r.phases["decode"];
        assert_eq!(
            (d.count, d.min_ms, d.max_ms, d.mean_ms),
            (5, 10.0, 50.0, 30.0)
        );
        let e = &r.phases["encode"];
        assert_eq!((e.count, e.mean_ms), (5, 3.0));
        assert!(r.to_string().contains("内訳 decode"));
        assert!(matches!(
            Bench::batch("p").from_samples(Condition::Warm, &ms(&[1, 2]), &phases),
            Err(BenchError::InvalidArgument(_))
        ));
        // 実行時の内訳。
        let r = Bench::batch("phased_run")
            .environment(fixed_env())
            .run_warm_phased(|t| {
                t.time("a", || std::thread::sleep(Duration::from_millis(1)));
                t.time("b", || 42)
            })
            .unwrap();
        assert!(r.phases["a"].min_ms >= 1.0);
        assert_eq!(r.phases["b"].count, 5);
    }

    #[test]
    fn comparison_and_regression_threshold() {
        let policy = RegressionPolicy::default();
        let base = result_1_to_30("cmp", 10); // p95 = 290
        // ちょうど 10% の悪化（p95 = 319）は悪化とする。
        let mut worse = base.clone();
        worse.stats.p95_ms = 319.0;
        let c = compare_results(&base, &worse, &policy);
        assert!(c.regressed, "{c:?}");
        assert!((c.p95_change.unwrap() - 0.1).abs() < 1e-12);
        // 9.9% は悪化としない。
        worse.stats.p95_ms = 318.7;
        assert!(!compare_results(&base, &worse, &policy).regressed);
        // 速くなったのは悪化ではない。
        let better = result_1_to_30("cmp", 5);
        let c = compare_results(&base, &better, &policy);
        assert!(!c.regressed);
        assert!((c.p95_change.unwrap() + 0.5).abs() < 1e-12);
        // 前回が 0 なら増加率は出さない。
        let mut zero = base.clone();
        zero.stats.p95_ms = 0.0;
        zero.stats.mean_ms = 0.0;
        let c = compare_results(&zero, &base, &policy);
        assert_eq!(c.p95_change, None);
        assert!(!c.regressed);
        // 基準を変えられる。
        let strict = RegressionPolicy { p95_ratio: 0.01 };
        let mut slightly = base.clone();
        slightly.stats.p95_ms = 293.0;
        assert!(compare_results(&base, &slightly, &strict).regressed);
    }

    #[test]
    fn find_previous_skips_incomparable_runs() {
        let a = result_1_to_30("f", 1);
        let mut other_gpu = result_1_to_30("f", 2);
        other_gpu.environment = other_gpu.environment.with_gpu("RTX 3080", None);
        let mut cold = result_1_to_30("f", 3);
        cold.condition = Condition::Cold;
        let mut few = result_1_to_30("f", 4);
        few.meets_iteration_rule = false;
        let history = vec![a.clone(), other_gpu, cold, few];
        let current = result_1_to_30("f", 1);
        assert_eq!(find_previous(&history, &current), Some(&history[0]));
        assert_eq!(find_previous(&[], &current), None);
    }

    #[test]
    fn recorder_appends_and_compares() {
        let dir = tempfile::tempdir().unwrap();
        let rec = BenchRecorder::new(dir.path().join("bench"));
        let first = result_1_to_30("rec", 10);
        let report = rec.record(&first).unwrap();
        assert!(report.comparison.is_none());
        assert_eq!(report.path, dir.path().join("bench/rec.json"));
        // 目標 250 ms に対して p95 は 290 なので未達 → 合格にしない。
        assert!(!report.passed());
        assert!(
            report
                .to_string()
                .contains("比べられる前回の結果はありません")
        );

        let second = result_1_to_30("rec", 12); // p95 290 → 348（+20%）
        let report = rec.record(&second).unwrap();
        let c = report.comparison.as_ref().unwrap();
        assert!(c.regressed);
        assert!((c.p95_change.unwrap() - 0.2).abs() < 1e-12);
        assert!(report.regressed());
        assert!(report.to_string().contains("悪化しています"));

        let runs = rec.load("rec").unwrap();
        assert_eq!(runs, vec![first, second.clone()]);
        // 次の比較の相手は直前の結果。
        let third = result_1_to_30("rec", 12);
        let report = rec.record(&third).unwrap();
        let c = report.comparison.unwrap();
        assert_eq!(c.previous_p95_ms, second.stats.p95_ms);
        assert!(!c.regressed);
        assert_eq!(rec.load("rec").unwrap().len(), 3);
        // 存在しない名前は空。
        assert!(rec.load("none").unwrap().is_empty());
    }

    #[test]
    fn recorder_does_not_overwrite_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let rec = BenchRecorder::new(dir.path());
        fs::write(rec.path("bad"), "not json").unwrap();
        let r = result_1_to_30("bad", 1);
        assert!(matches!(rec.record(&r), Err(BenchError::Corrupt { .. })));
        assert_eq!(fs::read_to_string(rec.path("bad")).unwrap(), "not json");
        // 別の名前の中身のファイルも受け付けない。
        rec.record(&result_1_to_30("good", 1)).unwrap();
        fs::copy(rec.path("good"), rec.path("other")).unwrap();
        assert!(matches!(rec.load("other"), Err(BenchError::Corrupt { .. })));
        assert!(matches!(rec.load("../x"), Err(BenchError::InvalidName(_))));
    }

    #[test]
    fn report_passed_requires_rule_target_and_no_regression() {
        let dir = tempfile::tempdir().unwrap();
        let rec = BenchRecorder::new(dir.path());
        let ok = Bench::latency("pass")
            .target(Duration::from_millis(100))
            .environment(fixed_env())
            .from_samples(Condition::Warm, &ms(&[10; 30]), &[])
            .unwrap();
        assert!(rec.record(&ok).unwrap().passed());
        let few = Bench::latency("pass_few")
            .environment(fixed_env())
            .from_samples(Condition::Warm, &ms(&[10; 3]), &[])
            .unwrap();
        let report = rec.record(&few).unwrap();
        assert!(!report.passed());
        assert!(report.to_string().contains("規則"));
    }

    #[test]
    fn result_json_has_required_fields() {
        let r = Bench::latency("json")
            .environment(fixed_env().with_gpu("llvmpipe", Some("Mesa".to_owned())))
            .input(InputRef::from_bytes("synthetic:x", b"x"))
            .develop_settings(&DevelopSettings::default())
            .note("resolution", "2560x1707")
            .from_samples(Condition::Warm, &ms(&[5; 30]), &[])
            .unwrap();
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        for key in [
            "timestamp_utc",
            "condition",
            "iterations",
            "stats",
            "environment",
            "inputs",
            "develop",
            "notes",
        ] {
            assert!(v.get(key).is_some(), "{key}");
        }
        assert_eq!(v["stats"]["p95_ms"], 5.0);
        assert_eq!(v["condition"], "warm");
        assert_eq!(v["environment"]["gpu"], "llvmpipe");
        let back: BenchResult = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
        // 時刻は RFC 3339 の UTC（末尾 Z）。
        assert!(r.timestamp_utc.ends_with('Z'), "{}", r.timestamp_utc);
        assert!(chrono::DateTime::parse_from_rfc3339(&r.timestamp_utc).is_ok());
    }

    #[test]
    fn concurrent_records_in_one_process_are_not_lost() {
        // テストは同じプロセスの複数のスレッドで並列に動く。同じ名前の記録が読み込み → 追記 →
        // 書き込みの途中で重なると、片方の結果が失われる（または一時ファイルが衝突して失敗する）。
        let dir = tempfile::tempdir().unwrap();
        let rec = BenchRecorder::new(dir.path());
        let env = fixed_env();
        std::thread::scope(|s| {
            for i in 0..8u64 {
                let (rec, env) = (&rec, &env);
                s.spawn(move || {
                    let samples: Vec<u64> = (1..=30).map(|v| v + i).collect();
                    let r = Bench::latency("shared")
                        .environment(env.clone())
                        .from_samples(Condition::Warm, &ms(&samples), &[])
                        .unwrap();
                    rec.record(&r).unwrap();
                });
            }
        });
        assert_eq!(rec.load("shared").unwrap().len(), 8);
    }

    #[test]
    fn find_previous_requires_same_inputs_settings_and_notes() {
        // 入力・現像設定・条件（notes。解像度など）が違う結果は、比べても意味がない
        // （例: 2560 × 1707 とフル解像度の時間を比べて「悪化」と判定しない）。
        let base = || {
            Bench::latency("cond")
                .environment(fixed_env())
                .input(InputRef::from_bytes("sample-a", b"a"))
                .note("resolution", "2560x1707")
        };
        let samples = ms(&[10; 30]);
        let prev = base().from_samples(Condition::Warm, &samples, &[]).unwrap();
        let other_input = Bench::latency("cond")
            .environment(fixed_env())
            .input(InputRef::from_bytes("sample-b", b"b"))
            .note("resolution", "2560x1707")
            .from_samples(Condition::Warm, &samples, &[])
            .unwrap();
        let other_note = Bench::latency("cond")
            .environment(fixed_env())
            .input(InputRef::from_bytes("sample-a", b"a"))
            .note("resolution", "7008x4672")
            .from_samples(Condition::Warm, &samples, &[])
            .unwrap();
        let settings = DevelopSettings {
            exposure_ev: 1.0,
            ..Default::default()
        };
        let other_settings = base()
            .develop_settings(&settings)
            .from_samples(Condition::Warm, &samples, &[])
            .unwrap();
        let history = vec![prev.clone()];
        for current in [&other_input, &other_note, &other_settings] {
            assert_eq!(find_previous(&history, current), None, "{current}");
        }
        let same = base().from_samples(Condition::Warm, &samples, &[]).unwrap();
        assert_eq!(find_previous(&history, &same), Some(&history[0]));
        // 同じ設定なら比べる（現像設定は develop_hash と処理バージョンで比べる）。
        let with_settings = base()
            .develop_settings(&settings)
            .from_samples(Condition::Warm, &samples, &[])
            .unwrap();
        assert_eq!(
            find_previous(std::slice::from_ref(&with_settings), &other_settings),
            Some(&with_settings)
        );
    }

    #[test]
    fn invalid_policy_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        for ratio in [f64::NAN, -0.1, f64::INFINITY] {
            let rec =
                BenchRecorder::new(dir.path()).with_policy(RegressionPolicy { p95_ratio: ratio });
            assert!(
                matches!(
                    rec.record(&result_1_to_30("policy", 1)),
                    Err(BenchError::InvalidArgument(_))
                ),
                "{ratio}"
            );
        }
        // 何も書かない。
        assert!(
            BenchRecorder::new(dir.path())
                .load("policy")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn recorder_from_env_default_dir() {
        if std::env::var_os(BENCH_DIR_ENV).is_none() {
            assert_eq!(
                BenchRecorder::from_env().dir(),
                Path::new(DEFAULT_BENCH_DIR)
            );
        }
    }
}
