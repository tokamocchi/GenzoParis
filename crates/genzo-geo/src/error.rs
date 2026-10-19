//! genzo-geo のエラー型。

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

/// 地図のタイルの取得・地名の検索のエラー。
///
/// 同じタイルの同時の要求を 1 つにまとめるとき、1 つの結果を待っていたすべての呼び出し元に返すため
/// `Clone` にしている（入出力のエラーは `Arc` で持つ）。
///
/// メッセージには要求した URL を入れない（タイルの番号や検索語から、利用者が見ている場所がわかるため。
/// ログに座標・検索語を残さない方針（決定事項ログ No.75）に合わせる）。
#[derive(Debug, Clone, thiserror::Error)]
pub enum GeoError {
    /// オンラインの地図が無効（利用者が設定で有効にしていない。docs/02 の SEC-02）。この crate は設定を
    /// 持たないので、genzo-app が通信の前に返す。
    #[error("オンラインの地図は無効です（設定で有効にすると使えます）")]
    Disabled,

    /// 引数・設定の値が不正（範囲外のタイル、https でない URL、空の検索語など）。
    #[error("不正な値です: {0}")]
    InvalidInput(String),

    /// 通信に失敗した（名前の解決・接続・TLS・タイムアウト・プロキシ）。
    #[error("通信に失敗しました: {0}")]
    Network(String),

    /// サーバーが成功以外の状態を返した（304 は呼び出し元が扱うので、ここには来ない）。
    #[error("サーバーがエラーを返しました（HTTP {0}）")]
    Status(u16),

    /// 応答が不正（種類が違う・大きすぎる・JSON が壊れている・https でない URL への転送）。
    #[error("サーバーの応答が不正です: {0}")]
    BadResponse(String),

    /// タイルのキャッシュのファイルの入出力のエラー。
    #[error("入出力のエラー（{path}）: {source}", path = .path.display())]
    Io {
        /// 対象のパス。
        path: PathBuf,
        /// 元のエラー。
        #[source]
        source: Arc<io::Error>,
    },
}

impl GeoError {
    /// 入出力のエラーを作る。
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source: Arc::new(source),
        }
    }
}
