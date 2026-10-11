//! 処理バージョン（docs/04_architecture.md の 2.5 節・7.1 節。02 の IQ-08・DATA-09）。
//!
//! 処理ステージは [`crate::stage::StageContext::process_version`] を見てアルゴリズムを切り替える。
//! 古い版のアルゴリズムは削除せず、回帰テストの対象として残す（2.5 節）。
//!
//! 版を列挙型にしているのは、新しい版を足したときに、すべてのステージの `match` がコンパイル
//! エラーになり、版ごとの扱いを書き忘れないようにするため。

use std::fmt;

use crate::error::{PipelineError, Result};

/// 処理バージョン。現在は v1 だけ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProcessVersion {
    /// 版 1（MVP。04 の 2.1 節のステージのうち、v1 の印のないもの）。
    V1,
}

impl ProcessVersion {
    /// このアプリの最新の版（genzo-model の [`genzo_model::CURRENT_PROCESS_VERSION`] と同じ）。
    pub const CURRENT: ProcessVersion = ProcessVersion::V1;

    /// 対応しているすべての版（古い順）。
    pub const ALL: [ProcessVersion; 1] = [ProcessVersion::V1];

    /// 番号から作る。対応していない番号（0、未知の版）は
    /// [`PipelineError::UnsupportedProcessVersion`]。
    ///
    /// 注意: [`genzo_model::DevelopSettings::normalized`] は処理バージョンを対応範囲に丸めるため、
    /// 未知の版を検出するには丸める前の値を渡すこと。
    pub fn from_u32(value: u32) -> Result<Self> {
        match value {
            1 => Ok(ProcessVersion::V1),
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
    }

    #[test]
    fn unknown_versions_are_errors() {
        for v in [0, 2, 99, u32::MAX] {
            match ProcessVersion::from_u32(v) {
                Err(PipelineError::UnsupportedProcessVersion { found, supported }) => {
                    assert_eq!(found, v);
                    assert_eq!(supported, 1);
                }
                other => panic!("{v}: {other:?}"),
            }
        }
        let msg = ProcessVersion::from_u32(7).unwrap_err().to_string();
        assert!(msg.contains('7'), "{msg}");
    }
}
