//! コア API のエラー（[`ApiError`]）と、UI に見せるための分類（[`ErrorInfo`]。04 の 6.3 節）。

use std::path::PathBuf;

use genzo_catalog::CatalogError;
use genzo_gpu::GpuError;
use genzo_media::MediaError;
use genzo_model::DevelopError;
use genzo_pipeline::PipelineError;
use genzo_worker::{WorkerClientError, WorkerErrorKind};
use serde::{Deserialize, Serialize};

/// コア API のエラー。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ApiError {
    /// 古い世代の検索結果への要求（3.7 節）。UI は最新の世代で範囲を要求し直す。
    #[error(
        "検索結果が更新されました（要求した世代 {requested}、現在の世代 {current:?}）。範囲を要求し直してください"
    )]
    Stale {
        /// 要求に添えられた世代。
        requested: u64,
        /// 現在の世代（検索していなければ `None`）。
        current: Option<u64>,
    },
    /// 指定したもの（variant・ジョブ・計画など）が見つからない。
    #[error("{0} が見つかりません")]
    NotFound(String),
    /// 引数が不正。
    #[error("不正な値です: {0}")]
    InvalidArgument(String),
    /// 現像中の写真がない（[`crate::Core::open_develop`] の前の操作）。
    #[error("現像中の写真がありません")]
    NoDevelopSession,
    /// コアは終了している。
    #[error("コアは終了しています")]
    Closed,
    /// 取り消された。
    #[error("処理は取り消されました")]
    Cancelled,
    /// 削除の計画の後にカタログが変わった（もう一度計画を作って確認する）。
    #[error("確認の後にカタログが変わりました。もう一度確認してください（{0}）")]
    PlanOutdated(String),
    /// 元ファイルにアクセスできない（見つからない、権限がない）。
    #[error("ファイルにアクセスできません（{}）: {message}", path.display())]
    FileAccess {
        /// ファイル。
        path: PathBuf,
        /// 理由。
        message: String,
    },
    /// OS のゴミ箱へ移せない（ゴミ箱のないドライブなど。直接の削除には切り替えない。6.4 節）。
    #[error("ゴミ箱へ移動できません（{}）: {message}", path.display())]
    Trash {
        /// ファイル。
        path: PathBuf,
        /// 理由。
        message: String,
    },
    /// カタログ・サムネイル DB・プレビューキャッシュのエラー。
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// ワーカーのエラー。
    #[error(transparent)]
    Worker(#[from] WorkerClientError),
    /// 現像パイプラインのエラー。
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
    /// GPU のエラー（通常は CPU 版に切り替えるので、利用者には届かない）。
    #[error(transparent)]
    Gpu(#[from] GpuError),
    /// 画像の入出力のエラー（書き出しなど）。
    #[error(transparent)]
    Media(#[from] MediaError),
    /// 現像設定のエラー。
    #[error(transparent)]
    Develop(#[from] DevelopError),
    /// ファイルの入出力のエラー。
    #[error("入出力のエラー（{}）: {source}", path.display())]
    Io {
        /// 対象のパス。
        path: PathBuf,
        /// 元のエラー。
        #[source]
        source: std::io::Error,
    },
    /// ジョブの実行の基盤のエラー（スケジューラの終了・パニックなど）。
    #[error("ジョブのエラー: {0}")]
    Job(String),
    /// 内部のエラー（不具合）。
    #[error("内部のエラー: {0}")]
    Internal(String),
}

impl ApiError {
    /// 入出力のエラーを作る。
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// 取り消し（取り消し・置き換え）によるエラーか。
    pub fn is_cancelled(&self) -> bool {
        match self {
            Self::Cancelled => true,
            Self::Worker(WorkerClientError::Cancelled) => true,
            Self::Pipeline(PipelineError::Cancelled) => true,
            Self::Gpu(e) => e.is_cancelled(),
            _ => false,
        }
    }

    /// UI に見せるための分類（[`ErrorInfo`]）。
    pub fn info(&self) -> ErrorInfo {
        let (kind, hint) = classify(self);
        ErrorInfo {
            kind,
            message: self.to_string(),
            user_actionable: kind.user_actionable(),
            retryable: kind.retryable(),
            hint: hint.map(str::to_owned),
        }
    }

    /// 分類の種類。
    pub fn kind(&self) -> ErrorKind {
        classify(self).0
    }
}

impl From<genzo_jobs::JobError> for ApiError {
    fn from(e: genzo_jobs::JobError) -> Self {
        if e.is_cancellation() {
            Self::Cancelled
        } else {
            Self::Job(e.to_string())
        }
    }
}

/// エラーの分類（UI での表示と、利用者の操作で直せるか）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// 入力・設定の誤り（利用者が直せる）。
    InvalidInput,
    /// 指定したものが見つからない（削除された、など）。
    NotFound,
    /// 検索結果が更新された（最新の世代で要求し直す）。
    Stale,
    /// 取り消された。
    Cancelled,
    /// 元ファイルにアクセスできない（ドライブの接続・権限を確かめる）。
    FileAccess,
    /// 対応していない形式。
    UnsupportedFile,
    /// 壊れたファイル（デコードできない、ワーカーが異常終了した）。
    CorruptFile,
    /// 書き出し先が原本と同じ（6.4 節）。
    ProtectedDestination,
    /// 名前の衝突など。
    Conflict,
    /// OS のゴミ箱が使えない。
    TrashUnavailable,
    /// ディスクの容量・書き込みのエラー。
    Storage,
    /// カタログの整合性の問題（バックアップからの復元を案内する。6.3 節）。
    CatalogCorrupt,
    /// ワーカー・外部プログラム（ffmpeg）の問題。
    Worker,
    /// GPU の問題。
    Gpu,
    /// コアは終了している。
    Closed,
    /// 内部のエラー（不具合）。
    Internal,
}

impl ErrorKind {
    /// 利用者の操作で直せるか（入力を直す、ドライブをつなぐ、別のフォルダを選ぶなど）。
    pub const fn user_actionable(self) -> bool {
        matches!(
            self,
            Self::InvalidInput
                | Self::FileAccess
                | Self::ProtectedDestination
                | Self::Conflict
                | Self::TrashUnavailable
                | Self::Storage
                | Self::CatalogCorrupt
        )
    }

    /// 同じ要求をもう一度試す意味があるか（世代を新しくして要求し直す、一時的な失敗）。
    pub const fn retryable(self) -> bool {
        matches!(self, Self::Stale | Self::Worker | Self::Gpu | Self::Storage)
    }
}

/// UI に見せるエラーの情報（serde でシリアライズできる。コマンドの応答の失敗に使う）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorInfo {
    /// 分類。
    pub kind: ErrorKind,
    /// 表示用の説明（日本語）。
    pub message: String,
    /// 利用者の操作で直せるか。
    pub user_actionable: bool,
    /// もう一度試す意味があるか。
    pub retryable: bool,
    /// 利用者への案内（あれば）。
    pub hint: Option<String>,
}

impl From<&ApiError> for ErrorInfo {
    fn from(e: &ApiError) -> Self {
        e.info()
    }
}

const HINT_RESTORE: &str = "詳細チェックを行い、問題があればバックアップから復元してください";
const HINT_FILE: &str = "ファイルの場所・ドライブの接続・権限を確かめてください";
const HINT_PROTECTED: &str = "書き出し先のフォルダか、衝突時の扱い（連番）を変えてください";
const HINT_TRASH: &str = "ゴミ箱のあるドライブか確かめてください。直接の削除は行いません";
const HINT_WORKER: &str = "ワーカーの実行ファイルの場所（GENZO_WORKER_PATH）を確かめてください";
const HINT_FFMPEG: &str =
    "ffmpeg / ffprobe が使えるか（GENZO_FFMPEG / GENZO_FFPROBE）確かめてください";

fn classify(e: &ApiError) -> (ErrorKind, Option<&'static str>) {
    use ErrorKind as K;
    match e {
        ApiError::Stale { .. } => (K::Stale, None),
        ApiError::NotFound(_) => (K::NotFound, None),
        ApiError::InvalidArgument(_) | ApiError::NoDevelopSession => (K::InvalidInput, None),
        ApiError::Closed => (K::Closed, None),
        ApiError::Cancelled => (K::Cancelled, None),
        ApiError::PlanOutdated(_) => (K::Conflict, None),
        ApiError::FileAccess { .. } => (K::FileAccess, Some(HINT_FILE)),
        ApiError::Trash { .. } => (K::TrashUnavailable, Some(HINT_TRASH)),
        ApiError::Catalog(c) => classify_catalog(c),
        ApiError::Worker(w) => classify_worker(w),
        ApiError::Pipeline(p) => match p {
            PipelineError::Cancelled => (K::Cancelled, None),
            PipelineError::InvalidArgument(_) | PipelineError::UnsupportedProcessVersion { .. } => {
                (K::InvalidInput, None)
            }
            PipelineError::Raw(_) => (K::CorruptFile, None),
            _ => (K::Internal, None),
        },
        ApiError::Gpu(g) if g.is_cancelled() => (K::Cancelled, None),
        ApiError::Gpu(_) => (K::Gpu, None),
        ApiError::Media(m) => classify_media(m),
        ApiError::Develop(_) => (K::InvalidInput, None),
        ApiError::Io { .. } => (K::Storage, None),
        ApiError::Job(_) | ApiError::Internal(_) => (K::Internal, None),
    }
}

fn classify_catalog(e: &CatalogError) -> (ErrorKind, Option<&'static str>) {
    use ErrorKind as K;
    match e {
        CatalogError::NotFound(_) => (K::NotFound, None),
        CatalogError::InvalidInput(_) | CatalogError::CannotDeleteMaster(_) => {
            (K::InvalidInput, None)
        }
        CatalogError::Develop(_) => (K::InvalidInput, None),
        CatalogError::Conflict(_) | CatalogError::AlreadyExists(_) => (K::Conflict, None),
        CatalogError::IntegrityCheckFailed(_)
        | CatalogError::Corrupt(_)
        | CatalogError::NotOurDatabase(_)
        | CatalogError::FutureSchema { .. }
        | CatalogError::OutdatedSchema { .. } => (K::CatalogCorrupt, Some(HINT_RESTORE)),
        CatalogError::Io { .. } => (K::Storage, None),
        CatalogError::Sqlite(rusqlite_err) => {
            let text = rusqlite_err.to_string();
            if e.is_constraint_violation() {
                (K::Conflict, None)
            } else if text.contains("disk is full") || text.contains("database or disk is full") {
                (K::Storage, None)
            } else if text.contains("malformed") || text.contains("not a database") {
                (K::CatalogCorrupt, Some(HINT_RESTORE))
            } else {
                (K::Internal, None)
            }
        }
        CatalogError::InvalidFileOpTransition { .. } | CatalogError::Json(_) => (K::Internal, None),
    }
}

fn classify_worker(e: &WorkerClientError) -> (ErrorKind, Option<&'static str>) {
    use ErrorKind as K;
    match e {
        WorkerClientError::Job(job) => match &job.kind {
            WorkerErrorKind::NotFound | WorkerErrorKind::Io => (K::FileAccess, Some(HINT_FILE)),
            WorkerErrorKind::Unsupported | WorkerErrorKind::TooLarge => (K::UnsupportedFile, None),
            WorkerErrorKind::Decode => (K::CorruptFile, None),
            WorkerErrorKind::Tool => (K::Worker, Some(HINT_FFMPEG)),
            _ => (K::Worker, None),
        },
        WorkerClientError::Timeout { .. }
        | WorkerClientError::Crashed { .. }
        | WorkerClientError::Protocol(_)
        | WorkerClientError::InvalidBuffer(_) => (K::Worker, None),
        WorkerClientError::Skipped { .. } => (K::CorruptFile, None),
        WorkerClientError::Cancelled => (K::Cancelled, None),
        WorkerClientError::Spawn(_) => (K::Worker, Some(HINT_WORKER)),
        WorkerClientError::InvalidArgument(_) => (K::InvalidInput, None),
        _ => (K::Worker, None),
    }
}

fn classify_media(e: &MediaError) -> (ErrorKind, Option<&'static str>) {
    use ErrorKind as K;
    match e {
        MediaError::ProtectedDestination { .. } => (K::ProtectedDestination, Some(HINT_PROTECTED)),
        MediaError::SequenceExhausted { .. } => (K::Conflict, Some(HINT_PROTECTED)),
        MediaError::InvalidExportSettings(_) | MediaError::InvalidArgument { .. } => {
            (K::InvalidInput, None)
        }
        MediaError::Io { .. } => (K::Storage, None),
        MediaError::Decode { .. } => (K::CorruptFile, None),
        MediaError::Unsupported { .. } | MediaError::TooLarge { .. } => (K::UnsupportedFile, None),
        MediaError::ToolSpawn { .. }
        | MediaError::ToolTimeout { .. }
        | MediaError::ToolOutputTooLarge { .. }
        | MediaError::ToolFailed { .. } => (K::Worker, Some(HINT_FFMPEG)),
        _ => (K::Internal, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_of_common_errors() {
        let stale = ApiError::Stale {
            requested: 1,
            current: Some(2),
        };
        let info = stale.info();
        assert_eq!(info.kind, ErrorKind::Stale);
        assert!(info.retryable && !info.user_actionable);
        let protected = ApiError::Media(MediaError::ProtectedDestination {
            destination: "/a.jpg".into(),
            protected: "/a.jpg".into(),
        });
        let info = protected.info();
        assert_eq!(info.kind, ErrorKind::ProtectedDestination);
        assert!(info.user_actionable);
        assert!(info.hint.is_some());
        assert!(ApiError::Worker(WorkerClientError::Cancelled).is_cancelled());
        assert!(ApiError::Pipeline(PipelineError::Cancelled).is_cancelled());
        assert_eq!(
            ApiError::Catalog(CatalogError::Corrupt("x".into())).kind(),
            ErrorKind::CatalogCorrupt
        );
        assert_eq!(
            ApiError::from(genzo_jobs::JobError::Superseded).kind(),
            ErrorKind::Cancelled
        );
        // JSON の形（UI に渡す）。
        let json = serde_json::to_value(stale.info()).unwrap();
        assert_eq!(json["kind"], "stale");
        assert!(json["message"].as_str().unwrap().contains("世代"));
    }

    #[test]
    fn errors_are_send_and_sync() {
        fn check<T: Send + Sync + 'static>() {}
        check::<ApiError>();
    }
}
