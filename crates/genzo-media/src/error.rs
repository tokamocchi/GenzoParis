//! genzo-media のエラー型。

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use genzo_color::ColorError;
use genzo_model::ExportSettingsError;
use thiserror::Error;

/// genzo-media の処理で起きるエラー。
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MediaError {
    /// ファイルの入出力に失敗した。
    #[error("ファイルの入出力に失敗した（{}）: {source}", display_path(path))]
    Io {
        /// 対象のパス（分からなければ `None`）。
        path: Option<PathBuf>,
        /// 元のエラー。
        #[source]
        source: io::Error,
    },

    /// 画像をデコードできない（壊れたファイルなど）。
    #[error("画像をデコードできない: {reason}")]
    Decode {
        /// 理由。
        reason: String,
    },

    /// 画像をエンコードできない。
    #[error("画像をエンコードできない: {reason}")]
    Encode {
        /// 理由。
        reason: String,
    },

    /// 対応していない形式・色の型。
    #[error("対応していない形式: {reason}")]
    Unsupported {
        /// 理由。
        reason: String,
    },

    /// 画像・ファイルが上限より大きい（壊れたファイルによる巨大な確保を防ぐため）。
    #[error("{what}が大きすぎる（{actual}、上限 {max}）")]
    TooLarge {
        /// 何が大きいか（「画素数」「ファイル」など）。
        what: &'static str,
        /// 実際の値。
        actual: u64,
        /// 上限。
        max: u64,
    },

    /// 画像の寸法・データの長さが不正。
    #[error("画像の寸法・データが不正: {reason}")]
    InvalidImage {
        /// 理由。
        reason: String,
    },

    /// 引数が不正。
    #[error("引数が不正: {reason}")]
    InvalidArgument {
        /// 理由。
        reason: String,
    },

    /// 書き出しの設定が不正。
    #[error("書き出しの設定が不正: {0}")]
    InvalidExportSettings(#[from] ExportSettingsError),

    /// 色の処理（ICC プロファイル・lcms2）のエラー。
    #[error(transparent)]
    Color(#[from] ColorError),

    /// 書き出し先が、保護するファイル（原本）と同じファイル（04 の 6.4 節）。
    #[error("書き出し先 {} は保護されたファイル {} と同じため、書き出しを中止した", destination.display(), protected.display())]
    ProtectedDestination {
        /// 書き出し先。
        destination: PathBuf,
        /// 一致した保護対象のファイル。
        protected: PathBuf,
    },

    /// 連番を付けた名前が上限まで使われている。
    #[error("連番を付けた名前が上限（{max}）まで使われている（{}）", path.display())]
    SequenceExhausted {
        /// 書き出そうとしたパス。
        path: PathBuf,
        /// 連番の上限。
        max: u32,
    },

    /// 外部プログラム（ffprobe / ffmpeg）を起動できない。
    #[error("外部プログラム {program} を起動できない: {source}")]
    ToolSpawn {
        /// プログラム名（パス）。
        program: String,
        /// 元のエラー。
        #[source]
        source: io::Error,
    },

    /// 外部プログラムが時間内に終わらなかった（強制終了した）。
    #[error("外部プログラム {program} が {timeout:?} 以内に終わらなかったため、強制終了した")]
    ToolTimeout {
        /// プログラム名（パス）。
        program: String,
        /// タイムアウト。
        timeout: Duration,
    },

    /// 外部プログラムの出力が上限を超えた（強制終了した）。
    #[error("外部プログラム {program} の出力が上限（{max} バイト）を超えたため、強制終了した")]
    ToolOutputTooLarge {
        /// プログラム名（パス）。
        program: String,
        /// 上限（バイト）。
        max: usize,
    },

    /// 外部プログラムが失敗した（終了コードが 0 以外）。
    #[error("外部プログラム {program} が失敗した（{status}）: {stderr}")]
    ToolFailed {
        /// プログラム名（パス）。
        program: String,
        /// 終了状態。
        status: String,
        /// 標準エラー出力の末尾。
        stderr: String,
    },

    /// 動画の情報（ffprobe の出力）を解釈できない。
    #[error("動画の情報を解釈できない: {reason}")]
    VideoProbe {
        /// 理由。
        reason: String,
    },
}

fn display_path(path: &Option<PathBuf>) -> String {
    path.as_ref()
        .map_or_else(|| "パス不明".to_owned(), |p| p.display().to_string())
}

impl MediaError {
    /// パス付きの入出力のエラーを作る。
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: Some(path.into()),
            source,
        }
    }

    /// パスなしの入出力のエラーを作る。
    pub(crate) fn io_no_path(source: io::Error) -> Self {
        Self::Io { path: None, source }
    }

    pub(crate) fn decode(reason: impl Into<String>) -> Self {
        Self::Decode {
            reason: reason.into(),
        }
    }

    pub(crate) fn encode(reason: impl Into<String>) -> Self {
        Self::Encode {
            reason: reason.into(),
        }
    }

    pub(crate) fn unsupported(reason: impl Into<String>) -> Self {
        Self::Unsupported {
            reason: reason.into(),
        }
    }

    pub(crate) fn invalid_image(reason: impl Into<String>) -> Self {
        Self::InvalidImage {
            reason: reason.into(),
        }
    }

    pub(crate) fn invalid_argument(reason: impl Into<String>) -> Self {
        Self::InvalidArgument {
            reason: reason.into(),
        }
    }
}

/// genzo-media の `Result`。
pub type Result<T> = std::result::Result<T, MediaError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_readable() {
        let e = MediaError::io("/tmp/a.jpg", io::Error::other("x"));
        assert!(e.to_string().contains("/tmp/a.jpg"));
        let e = MediaError::io_no_path(io::Error::other("x"));
        assert!(e.to_string().contains("パス不明"));
        let e = MediaError::TooLarge {
            what: "画素数",
            actual: 10,
            max: 5,
        };
        assert_eq!(e.to_string(), "画素数が大きすぎる（10、上限 5）");
    }

    #[test]
    fn error_is_send_sync() {
        fn check<T: Send + Sync + 'static>() {}
        check::<MediaError>();
    }
}
