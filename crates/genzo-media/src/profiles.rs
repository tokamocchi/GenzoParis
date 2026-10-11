//! 標準の ICC プロファイルの使い回し。
//!
//! lcms2 でプロファイルを作るのは 1 回あたり数ミリ秒かかるため、取り込みで大量のサムネイルを作る
//! ときに毎回作らないよう、プロセスの中で一度だけ作って複製して使う（[`IccProfile`] はバイト列を
//! `Arc` で共有するので、複製は軽い）。生成するバイト列は決定的（genzo-color）なので、使い回しても
//! 結果は変わらない。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use genzo_color::{IccProfile, IccVersion, StandardProfile};

use crate::error::Result;

type Cache = Mutex<HashMap<(StandardProfile, IccVersion), IccProfile>>;

/// 標準のプロファイルを返す（初回だけ作る）。
pub(crate) fn standard(kind: StandardProfile, version: IccVersion) -> Result<IccProfile> {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    // ロックの中でパニックすることはない（作成はロックの外）が、毒されていても中身は使える。
    if let Some(p) = cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(kind, version))
    {
        return Ok(p.clone());
    }
    let p = IccProfile::standard_with_version(kind, version)?;
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((kind, version), p.clone());
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_profiles_equal_fresh_ones() {
        for kind in StandardProfile::ALL {
            for v in [IccVersion::V2_4, IccVersion::V4_3] {
                let a = standard(kind, v).unwrap();
                let b = standard(kind, v).unwrap();
                assert_eq!(a, b);
                assert_eq!(a, IccProfile::standard_with_version(kind, v).unwrap());
            }
        }
    }
}
