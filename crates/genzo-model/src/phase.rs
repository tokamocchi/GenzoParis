//! パイプラインの段階（docs/04_architecture.md の 2.2 節）。

use std::fmt;

use serde::{Deserialize, Serialize};

/// 現像パイプラインの段階。段階の境目で結果をキャッシュする（04 の 2.2 節）。
///
/// 並び順は処理の順（A0 < A1 < B < C）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Phase {
    /// 段階 A0: RAW の展開（ステージ 1）。対話用ワーカーで実行し、CFA を本体のメモリに保持する。
    A0,
    /// 段階 A1: センサー処理（ステージ 2〜8）。WB・ノイズ軽減・レンズ補正（周辺減光・倍率色収差）・
    /// カメラプロファイルを変えたときに再計算し、プレビュー解像度の作業色空間の画像を保持する。
    /// 歪曲補正はステージ 9（ジオメトリ）なので含まない。
    A1,
    /// 段階 B: ガイドの計算（ステージ 11, 12 のガイド。2.7 節）。A1 の項目に加えて、回転と
    /// 歪曲補正に依存する（ガイドは G 座標で作るため）。
    B,
    /// 段階 C: 仕上げ（ステージ 9〜17）。スライダーを操作するたびに計算する。
    C,
}

impl Phase {
    /// すべての段階（処理の順）。
    pub const ALL: [Phase; 4] = [Phase::A0, Phase::A1, Phase::B, Phase::C];

    /// 表示・ログ用の名前（`"A0"` など）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Phase::A0 => "A0",
            Phase::A1 => "A1",
            Phase::B => "B",
            Phase::C => "C",
        }
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_are_ordered_by_processing_order() {
        assert!(Phase::A0 < Phase::A1);
        assert!(Phase::A1 < Phase::B);
        assert!(Phase::B < Phase::C);
        assert_eq!(Phase::ALL.map(Phase::as_str), ["A0", "A1", "B", "C"]);
        assert_eq!(serde_json::to_string(&Phase::A1).unwrap(), "\"A1\"");
    }
}
