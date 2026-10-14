//! 処理バージョン（docs/04_architecture.md の 2.5 節・7.1 節。02 の IQ-08・DATA-09）。
//!
//! 処理ステージは [`crate::stage::StageContext::process_version`] を見てアルゴリズムを切り替える。
//! 古い版のアルゴリズムは削除せず、回帰テストの対象として残す（2.5 節）。
//!
//! 版を列挙型にしているのは、新しい版を足したときに、すべてのステージの `match` がコンパイル
//! エラーになり、版ごとの扱いを書き忘れないようにするため。
//!
//! | 版 | 内容 |
//! |---|---|
//! | 1 | MVP（04 の 2.1 節のステージのうち、v1 の印のないもの）。v1 の機能の項目に値があっても無視する（[`crate::finish::unimplemented_settings`]） |
//! | 2 | v1 の機能のステージ（2026-10-13 に作った。最初はレンズ補正（DEV-14）: ステージ 7 の周辺減光・倍率色収差と、ステージ 9 の歪曲補正。[`crate::lens`]。後から足したもの: ハイライト復元・ノイズ軽減（ステージ 4・6）、カラーミキサー・カラーグレーディング・白黒（ステージ 13。[`crate::finish::color`]）、テクスチャ・明瞭度・かすみの除去（ステージ 12。[`crate::finish::presence`]）、シャープ（ステージ 14。[`crate::finish::sharpen`]）、効果（ステージ 16 の周辺光量補正・粒子。[`crate::finish::effects`]））。v1 の機能の項目が既定値なら、版 1 とビット単位で同じ結果になる |
//!
//! **版 2 の中身の決めごと**（決定事項ログ No.120）: 配布の前なので、v1 の機能のパッケージ（ノイズ軽減・
//! ハイライト復元・HSL など）は、新しい版を作らずに版 2 に足す。配布を始めた後の変更は新しい版にする
//! （DATA-09）。版 1 のアルゴリズムは変えない（既存の写真の結果を変えない。IQ-08）。

use std::fmt;

use crate::error::{PipelineError, Result};

/// 処理バージョン（モジュールの doc の表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProcessVersion {
    /// 版 1（MVP。04 の 2.1 節のステージのうち、v1 の印のないもの）。
    V1,
    /// 版 2（v1 の機能のステージ。レンズ補正から）。
    V2,
}

impl ProcessVersion {
    /// このアプリの最新の版（genzo-model の [`genzo_model::CURRENT_PROCESS_VERSION`] と同じ）。
    pub const CURRENT: ProcessVersion = ProcessVersion::V2;

    /// 対応しているすべての版（古い順）。
    pub const ALL: [ProcessVersion; 2] = [ProcessVersion::V1, ProcessVersion::V2];

    /// v1 の機能のステージ（レンズ補正など）を適用する版か（版 2 以降）。
    pub const fn has_v1_features(self) -> bool {
        match self {
            ProcessVersion::V1 => false,
            ProcessVersion::V2 => true,
        }
    }

    /// 版 `self` と `other` で、現像設定の同じ項目が同じ意味か（版の違う写真の間で設定の項目を写してよいか。
    /// genzo-api の設定の貼り付け（DEV-30）が使う）。
    ///
    /// 版 2 は版 1 にステージを足しただけで、版 1 からある項目の意味は変えていない（v1 の機能の項目が既定値
    /// なら版 1 とビット単位で同じ結果。決定事項ログ No.120）ので、版 1 と 2 は同じ意味。後の版で既存の項目の
    /// 意味を変えたら、その組み合わせで偽を返す（貼り付けの側は拒むか、変換する）。版を足すと、この `match` が
    /// コンパイルエラーになるので、扱いを決め忘れない。
    pub const fn shares_settings_with(self, other: ProcessVersion) -> bool {
        match (self, other) {
            (ProcessVersion::V1 | ProcessVersion::V2, ProcessVersion::V1 | ProcessVersion::V2) => {
                true
            }
        }
    }

    /// 番号から作る。対応していない番号（0、未知の版）は
    /// [`PipelineError::UnsupportedProcessVersion`]。
    ///
    /// 注意: [`genzo_model::DevelopSettings::normalized`] は処理バージョンを対応範囲に丸めるため、
    /// 未知の版を検出するには丸める前の値を渡すこと。
    pub fn from_u32(value: u32) -> Result<Self> {
        match value {
            1 => Ok(ProcessVersion::V1),
            2 => Ok(ProcessVersion::V2),
            found => Err(PipelineError::UnsupportedProcessVersion {
                found,
                supported: Self::CURRENT.get(),
            }),
        }
    }

    /// 番号（1 など）。
    pub const fn get(self) -> u32 {
        match self {
            ProcessVersion::V1 => 1,
            ProcessVersion::V2 => 2,
        }
    }
}

impl TryFrom<u32> for ProcessVersion {
    type Error = PipelineError;

    fn try_from(value: u32) -> Result<Self> {
        Self::from_u32(value)
    }
}

impl fmt::Display for ProcessVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_matches_the_model() {
        assert_eq!(
            ProcessVersion::CURRENT.get(),
            genzo_model::CURRENT_PROCESS_VERSION
        );
        assert_eq!(ProcessVersion::ALL.last(), Some(&ProcessVersion::CURRENT));
    }

    #[test]
    fn known_versions_parse() {
        assert_eq!(ProcessVersion::from_u32(1).unwrap(), ProcessVersion::V1);
        assert_eq!(ProcessVersion::try_from(1).unwrap(), ProcessVersion::V1);
        assert_eq!(ProcessVersion::V1.to_string(), "v1");
        assert_eq!(ProcessVersion::from_u32(2).unwrap(), ProcessVersion::V2);
        assert_eq!(ProcessVersion::V2.to_string(), "v2");
        assert!(ProcessVersion::V1 < ProcessVersion::V2);
        assert!(!ProcessVersion::V1.has_v1_features());
        assert!(ProcessVersion::V2.has_v1_features());
        for v in ProcessVersion::ALL {
            assert_eq!(ProcessVersion::from_u32(v.get()).unwrap(), v);
        }
    }

    #[test]
    fn versions_1_and_2_share_the_meaning_of_settings() {
        // 版 2 は版 1 にステージを足しただけ（決定事項ログ No.120）。どの組み合わせでも項目を写せる。
        for a in ProcessVersion::ALL {
            for b in ProcessVersion::ALL {
                assert!(a.shares_settings_with(b), "{a} と {b}");
            }
        }
    }

    #[test]
    fn unknown_versions_are_errors() {
        for v in [0, 3, 99, u32::MAX] {
            match ProcessVersion::from_u32(v) {
                Err(PipelineError::UnsupportedProcessVersion { found, supported }) => {
                    assert_eq!(found, v);
                    assert_eq!(supported, 2);
                }
                other => panic!("{v}: {other:?}"),
            }
        }
        let msg = ProcessVersion::from_u32(7).unwrap_err().to_string();
        assert!(msg.contains('7'), "{msg}");
    }
}
