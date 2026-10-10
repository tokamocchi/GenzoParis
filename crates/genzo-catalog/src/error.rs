//! カタログのエラー型。

use std::io;
use std::path::PathBuf;

use genzo_model::{DevelopError, FileOpId, FileOpState, VariantId};

/// カタログ・サムネイル DB・プレビューキャッシュの操作のエラー。
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// SQLite のエラー（制約違反を含む。[`CatalogError::is_constraint_violation`]）。
    #[error("SQLite のエラー: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// ファイルの入出力のエラー。
    #[error("入出力のエラー（{path}）: {source}", path = .path.display())]
    Io {
        /// 対象のパス。
        path: PathBuf,
        /// 元のエラー。
        #[source]
        source: io::Error,
    },

    /// DB のスキーマの版が、このアプリが対応している版より新しい（DATA-08）。
    #[error(
        "データベースのスキーマの版 {found} はこのアプリより新しいため開けません（対応しているのは {supported} まで）"
    )]
    FutureSchema {
        /// DB の `PRAGMA user_version`。
        found: u32,
        /// このアプリが対応している最新の版。
        supported: u32,
    },

    /// GenzoParis のカタログ（またはサムネイル DB）ではない SQLite のファイル。
    #[error("GenzoParis のデータベースではありません: {}", .0.display())]
    NotOurDatabase(PathBuf),

    /// 読み取り専用で開いたカタログのスキーマが古い（先に書き込み用に開いて移行する必要がある）。
    #[error(
        "カタログのスキーマの版 {found} が古いため、読み取り専用では開けません（必要な版は {expected}）"
    )]
    OutdatedSchema {
        /// DB の `PRAGMA user_version`。
        found: u32,
        /// このアプリのスキーマの版。
        expected: u32,
    },

    /// 指定した行が見つからない。
    #[error("{0} が見つかりません")]
    NotFound(String),

    /// 引数が不正。
    #[error("不正な値です: {0}")]
    InvalidInput(String),

    /// マスターの variant を仮想コピーの削除で消そうとした（04 の 6.4 節）。
    #[error("variant {0} はマスターのため、仮想コピーの削除では削除できません")]
    CannotDeleteMaster(VariantId),

    /// ファイル操作の記録の状態遷移として許されない（04 の 6.4 節の状態遷移図）。
    #[error("ファイル操作 {id} の状態を {from} から {to} へ変更できません")]
    InvalidFileOpTransition {
        /// ファイル操作の ID。
        id: FileOpId,
        /// 現在の状態。
        from: FileOpState,
        /// 変更しようとした状態。
        to: FileOpState,
    },

    /// 移動・リネーム先に、カタログ上で同じ名前のファイルが既にある。
    #[error("移動先に同じ名前のファイルがカタログに登録されています: {0}")]
    Conflict(String),

    /// 書き込み先のファイルが既に存在する（バックアップ・復元の誤上書きを防ぐ）。
    #[error("書き込み先が既に存在します: {}", .0.display())]
    AlreadyExists(PathBuf),

    /// 整合性チェック（`PRAGMA integrity_check`）で問題が見つかった。
    #[error("整合性チェックで問題が見つかりました: {}", .0.join(" / "))]
    IntegrityCheckFailed(Vec<String>),

    /// 現像設定の読み込み・保存のエラー。
    #[error("現像設定のエラー: {0}")]
    Develop(#[from] DevelopError),

    /// JSON のエラー（ファイル操作の記録など）。
    #[error("JSON のエラー: {0}")]
    Json(#[from] serde_json::Error),

    /// DB に保存された値が不正（列挙値に変換できないなど）。
    #[error("カタログに不正な値が保存されています: {0}")]
    Corrupt(String),
}

impl CatalogError {
    /// 制約違反（UNIQUE・CHECK・外部キー・NOT NULL）による SQLite のエラーか。
    pub fn is_constraint_violation(&self) -> bool {
        matches!(
            self,
            CatalogError::Sqlite(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation
        )
    }

    /// 入出力のエラーを作る。
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        CatalogError::Io {
            path: path.into(),
            source,
        }
    }
}

/// このクレートの `Result`。
pub type Result<T, E = CatalogError> = std::result::Result<T, E>;
