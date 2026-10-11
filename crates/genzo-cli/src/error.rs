//! CLI のエラーと終了コード（[`crate::EXIT_ERROR`]・[`crate::EXIT_USAGE`]）。
//!
//! `--json` のときに出すエラーの分類（`error.kind`）は、genzo-api の [`ErrorKind`]（snake_case。
//! `invalid_input`・`not_found`・`file_access`・`protected_destination`・`conflict`・`internal` など）と、
//! CLI の使い方の誤りを表す `usage` だけにする（機械が読む側で分岐できる閉じた集合にするため）。

use std::path::Path;

use genzo_api::{ApiError, ErrorInfo, ErrorKind};
use serde_json::json;

/// CLI のエラー。
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// 使い方の誤り（終了コード 2。`kind` は `usage`）。
    #[error("{0}")]
    Usage(String),
    /// コア API のエラー（分類は genzo-api の [`ErrorInfo`]）。
    #[error(transparent)]
    Api(#[from] ApiError),
    /// ジョブ（取り込み・書き出しなど）の失敗（genzo-api の分類を保つ）。
    #[error("{what}に失敗しました: {}", error.message)]
    Job {
        /// ジョブの表示名。
        what: String,
        /// 失敗の情報。
        error: ErrorInfo,
    },
    /// 分類と案内を CLI が決めるエラー（入力の JSON の誤り、ファイルを読めないなど。終了コード 1）。
    #[error("{message}")]
    Classified {
        /// 分類（genzo-api の [`ErrorKind`]）。
        kind: ErrorKind,
        /// 表示用の説明。
        message: String,
        /// 利用者への案内（あれば）。
        hint: Option<String>,
    },
}

impl CliError {
    /// 分類を指定して作る。
    pub fn classified(kind: ErrorKind, message: impl Into<String>, hint: Option<&str>) -> Self {
        Self::Classified {
            kind,
            message: message.into(),
            hint: hint.map(str::to_owned),
        }
    }

    /// 利用者の入力（現像設定の JSON など）の誤り（`invalid_input`）。
    pub fn input(message: impl Into<String>) -> Self {
        Self::classified(ErrorKind::InvalidInput, message, None)
    }

    /// 入出力のエラーを作る（分類は非公開の関数 `io_kind`: 見つからない・権限がないなどは
    /// `file_access`、それ以外は `storage`）。
    pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Self::classified(
            io_kind(&source),
            format!("{}: {source}", path.as_ref().display()),
            None,
        )
    }

    /// 内部のエラー・想定外の状態（`internal`）。
    pub fn other(e: impl std::fmt::Display) -> Self {
        Self::classified(ErrorKind::Internal, e.to_string(), None)
    }

    /// コア API のエラーに、CLI の文脈での案内を付ける（genzo-api の案内を置き換える）。
    pub fn with_hint(e: ApiError, hint: impl Into<String>) -> Self {
        let info = e.info();
        Self::Classified {
            kind: info.kind,
            message: info.message,
            hint: Some(hint.into()),
        }
    }

    /// 終了コード（使い方の誤りは 2、それ以外は 1）。
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) => crate::EXIT_USAGE,
            _ => crate::EXIT_ERROR,
        }
    }

    /// 分類・説明・案内（`--json` と人が読む表示の両方の元）。使い方の誤りは `None`（分類が
    /// genzo-api の [`ErrorKind`] にないため）。
    fn info(&self) -> Option<ErrorInfo> {
        match self {
            Self::Usage(_) => None,
            Self::Api(e) => Some(e.info()),
            Self::Job { what, error } => Some(ErrorInfo {
                message: format!("{what}に失敗しました: {}", error.message),
                ..error.clone()
            }),
            Self::Classified {
                kind,
                message,
                hint,
            } => Some(ErrorInfo {
                kind: *kind,
                message: message.clone(),
                user_actionable: kind.user_actionable(),
                retryable: kind.retryable(),
                hint: hint.clone(),
            }),
        }
    }

    /// `--json` のときに標準出力に出すエラーの JSON（`{"error": {"kind", "message",
    /// "user_actionable", "retryable", "hint"}}`）。
    pub fn to_json(&self) -> serde_json::Value {
        match (self, self.info()) {
            (Self::Usage(m), _) => usage_json(m),
            (_, Some(info)) => json!({ "error": info }),
            (_, None) => usage_json(&self.to_string()),
        }
    }

    /// 人が読む表示（案内があれば 2 行目に）。
    pub fn human(&self) -> String {
        match (self, self.info()) {
            (Self::Usage(m), _) => format!("使い方の誤り: {m}"),
            // 説明の改行（ワーカーのエラーなど）は 1 行にまとめ、案内だけを 2 行目に出す。
            (_, Some(ErrorInfo { message, hint, .. })) => {
                let message = crate::output::one_line(&message);
                match hint {
                    Some(h) => format!("エラー: {message}\n  {h}"),
                    None => format!("エラー: {message}"),
                }
            }
            (_, None) => format!("エラー: {self}"),
        }
    }
}

/// 入出力のエラーの分類: 見つからない・権限がない・フォルダとファイルの取り違えは `file_access`
/// （利用者がパスを直す）、それ以外（容量など）は `storage`。
pub fn io_kind(e: &std::io::Error) -> ErrorKind {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotFound | K::PermissionDenied | K::IsADirectory | K::NotADirectory => {
            ErrorKind::FileAccess
        }
        _ => ErrorKind::Storage,
    }
}

/// 使い方の誤りの JSON（clap の解釈の誤りにも使う）。
pub fn usage_json(message: &str) -> serde_json::Value {
    json!({
        "error": {
            "kind": "usage",
            "message": message,
            "user_actionable": true,
            "retryable": false,
            "hint": "genzo --help で使い方を確かめてください",
        }
    })
}

impl From<genzo_worker::WorkerClientError> for CliError {
    fn from(e: genzo_worker::WorkerClientError) -> Self {
        Self::Api(ApiError::Worker(e))
    }
}

impl From<genzo_pipeline::PipelineError> for CliError {
    fn from(e: genzo_pipeline::PipelineError) -> Self {
        Self::Api(ApiError::Pipeline(e))
    }
}

impl From<genzo_media::MediaError> for CliError {
    fn from(e: genzo_media::MediaError) -> Self {
        Self::Api(ApiError::Media(e))
    }
}

impl From<genzo_model::DevelopError> for CliError {
    fn from(e: genzo_model::DevelopError) -> Self {
        Self::Api(ApiError::Develop(e))
    }
}

impl From<genzo_gpu::GpuError> for CliError {
    fn from(e: genzo_gpu::GpuError) -> Self {
        Self::Api(ApiError::Gpu(e))
    }
}

impl From<genzo_testkit::bench::BenchError> for CliError {
    fn from(e: genzo_testkit::bench::BenchError) -> Self {
        use genzo_testkit::bench::BenchError as B;
        let kind = match &e {
            B::Io { .. } => ErrorKind::Storage,
            // 記録のファイルが壊れている・形式が違う（上書きせずに止める。利用者が退避して直す）。
            B::Corrupt { .. } => ErrorKind::InvalidInput,
            _ => ErrorKind::Internal,
        };
        Self::classified(kind, format!("計測のエラー: {e}"), None)
    }
}

/// CLI の処理の結果。
pub type CliResult<T> = Result<T, CliError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// `--json` のエラーの `kind` は genzo-api の分類か `usage` だけ（機械が読む側の分岐のため）。
    #[test]
    fn json_kinds_are_api_kinds_or_usage() {
        let cases = [
            (CliError::Usage("x".into()), "usage", 2),
            (
                CliError::Api(ApiError::NotFound("variant 9".into())),
                "not_found",
                1,
            ),
            (CliError::input("設定の項目がない"), "invalid_input", 1),
            (CliError::other("想定外"), "internal", 1),
            (
                CliError::io("a.json", std::io::Error::from(std::io::ErrorKind::NotFound)),
                "file_access",
                1,
            ),
            (
                CliError::io("o", std::io::Error::other("disk full")),
                "storage",
                1,
            ),
        ];
        for (e, kind, code) in cases {
            let v = e.to_json();
            assert_eq!(v["error"]["kind"], kind, "{e:?}");
            assert_eq!(e.exit_code(), code);
            for key in ["message", "user_actionable", "retryable", "hint"] {
                assert!(v["error"].get(key).is_some(), "{key}: {v}");
            }
        }
        assert_eq!(
            CliError::input("x").to_json()["error"]["user_actionable"],
            true
        );
    }

    #[test]
    fn hints_replace_the_api_hint_and_are_shown_on_the_second_line() {
        let e = CliError::with_hint(
            ApiError::NotFound("カタログ c.db".into()),
            "genzo catalog init で作れます",
        );
        assert_eq!(e.to_json()["error"]["kind"], "not_found");
        assert_eq!(
            e.human(),
            "エラー: カタログ c.db が見つかりません\n  genzo catalog init で作れます"
        );
        let job = CliError::Job {
            what: "書き出し".into(),
            error: ApiError::Cancelled.info(),
        };
        assert_eq!(job.to_json()["error"]["kind"], "cancelled");
        assert!(job.human().starts_with("エラー: 書き出しに失敗しました: "));
        assert_eq!(CliError::Usage("--x".into()).human(), "使い方の誤り: --x");
    }
}
