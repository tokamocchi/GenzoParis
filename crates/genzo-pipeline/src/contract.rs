//! 色の契約（docs/04_architecture.md の 2.6 節。レビュー R-01）。
//!
//! パイプラインの各境界で、画像の値が何を意味するかを [`ColorContract`] で表す。RGB の画像
//! （[`crate::image::RgbImage`]）は自分がどの境界の値かを持ち、ステージは入力の契約を確かめる
//! （[`crate::stage::run_stage_cpu`]）。CFA（B0）は型（[`crate::image::CfaImage`]）で区別する。
//!
//! | 境界 | 位置 | 原色 | 白色点 | 伝達関数 | 値の範囲 |
//! |---|---|---|---|---|---|
//! | B0 | ステージ 2 の出力 | センサー（チャンネルごと） | — | リニア | 黒レベル 0・白レベル 1。飽和した画素に印 |
//! | B1 | ステージ 3〜7 | センサー | WB で被写体の中立色を (1, 1, 1) に正規化 | リニア | 0 以上、上限なし |
//! | B2 | ステージ 8〜14 | ITU-R BT.2020 | D65 | リニア | 上限なし。負の値（色域外）を保持 |
//! | B3 | ステージ 15 の出力〜16 | ITU-R BT.2020 | D65 | リニア（ディスプレイの白 = 1） | 輝度 0〜1。負の値を保持 |
//! | B4a | ステージ 17a | モニターの ICC | モニターの ICC | モニターの ICC | 0〜1 |
//! | B4b | ステージ 17b | sRGB / Display P3 / Adobe RGB | D65 | IEC 61966-2-1（sRGB・P3）、ガンマ 563/256（Adobe RGB） | 0〜1 |
//! | B5 | キャッシュ（L0 / L1） | Display P3 | D65 | IEC 61966-2-1 | 0〜1（8bit JPEG） |
//!
//! B3 のトーンカーブ（ステージ 16）は、計算のときだけ一時的に IEC 61966-2-1 で符号化する
//! （出力は B3 のリニアのまま）。

use std::fmt;

/// パイプラインの境界の色の契約（2.6 節の表の B0〜B5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorContract {
    /// B0: CFA（ステージ 2 の出力）。黒レベル 0・白レベル 1 のリニアなセンサーの値。
    B0Cfa,
    /// B1: カメラ RGB（ステージ 3〜7）。WB の係数で被写体の中立色を (1, 1, 1) にしたリニアな値。
    /// 0 以上、上限なし。
    B1CameraRgb,
    /// B2: 作業色空間（ステージ 8〜14）。リニア ITU-R BT.2020・D65。上限なし、負の値を保持。
    B2Working,
    /// B3: ディスプレイ参照（ステージ 15 の出力〜16）。リニア BT.2020・D65、ディスプレイの白 = 1。
    B3DisplayReferred,
    /// B4a: 画面への出力（ステージ 17a）。モニターの ICC プロファイルの値（0〜1）。
    B4aDisplay,
    /// B4b: 書き出し（ステージ 17b）。出力の色空間で符号化した値（0〜1）。
    B4bExport,
    /// B5: キャッシュ（L0 / L1）。Display P3・IEC 61966-2-1 で符号化した値（0〜1）。
    B5Cache,
}

impl ColorContract {
    /// 表の名前（`"B2"` など）。
    pub const fn label(self) -> &'static str {
        match self {
            ColorContract::B0Cfa => "B0",
            ColorContract::B1CameraRgb => "B1",
            ColorContract::B2Working => "B2",
            ColorContract::B3DisplayReferred => "B3",
            ColorContract::B4aDisplay => "B4a",
            ColorContract::B4bExport => "B4b",
            ColorContract::B5Cache => "B5",
        }
    }

    /// シーンリニア・ディスプレイ参照のリニアな値か（B0〜B3）。リニアな値は面積平均で縮小してよい
    /// （[`crate::resample`]）。
    pub const fn is_linear(self) -> bool {
        matches!(
            self,
            ColorContract::B0Cfa
                | ColorContract::B1CameraRgb
                | ColorContract::B2Working
                | ColorContract::B3DisplayReferred
        )
    }
}

impl fmt::Display for ColorContract {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_linearity() {
        assert_eq!(ColorContract::B2Working.to_string(), "B2");
        assert_eq!(ColorContract::B4aDisplay.label(), "B4a");
        assert!(ColorContract::B1CameraRgb.is_linear());
        assert!(ColorContract::B3DisplayReferred.is_linear());
        assert!(!ColorContract::B4bExport.is_linear());
        assert!(!ColorContract::B5Cache.is_linear());
    }
}
