//! ワーカーのメモリの上限（docs/04_architecture.md の 1.2 節「ワーカーごとにメモリの上限を設けます
//! （OS の機能で設定できるかは PoC-2 で確認）」、9 章の AR-10）。
//!
//! - **Linux**: ワーカーが起動直後に自分の `RLIMIT_DATA`（`setrlimit`）を設定する。Linux 4.7 以降の
//!   `RLIMIT_DATA` は、ヒープと、共有でない書き込み可能なマップ（大きな `malloc`、スレッドの
//!   スタック）の合計を制限する。共有メモリ（[`crate::shm`] のファイルの共有マップ）は含まない。
//!   上限を超える確保は失敗し、Rust の確保では異常終了（`handle_alloc_error`）になる。本体はそれを
//!   ワーカーの異常終了として扱う。ソフトとハードの両方を下げるので、ワーカー自身は上限を戻せない。
//! - **Windows**（Job Object の `JOB_OBJECT_LIMIT_PROCESS_MEMORY`）・**macOS**（`RLIMIT_DATA` は
//!   `mmap` を制限しないとされる）: 未対応。上限を指定しても設定せず、
//!   [`MemoryLimitStatus::NotApplied`] を返す。**PoC-2 で確認する。**

use crate::protocol::MemoryLimitStatus;

/// 上限として受け付ける最小の値（バイト）。
///
/// **仮置き**: 64 MiB。これより小さいと、ワーカーの起動（LibRaw・lcms2 の初期化、スレッドの
/// スタック）にも足りなくなるおそれがある。
pub const MIN_MEMORY_LIMIT_BYTES: u64 = 64 * 1024 * 1024;

/// このプロセス（ワーカー）のメモリの上限を設定する。
pub fn apply_memory_limit(bytes: u64) -> MemoryLimitStatus {
    if bytes < MIN_MEMORY_LIMIT_BYTES {
        return MemoryLimitStatus::NotApplied {
            bytes,
            reason: format!("上限が小さすぎる（{MIN_MEMORY_LIMIT_BYTES} バイト以上が必要）"),
        };
    }
    imp::apply(bytes)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::MemoryLimitStatus;

    pub(super) fn apply(bytes: u64) -> MemoryLimitStatus {
        let not_applied = |reason: String| MemoryLimitStatus::NotApplied { bytes, reason };
        let mut current = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: getrlimit は、渡した有効な構造体に現在の上限を書くだけ。
        if unsafe { libc::getrlimit(libc::RLIMIT_DATA, &mut current) } != 0 {
            return not_applied(format!(
                "getrlimit に失敗: {}",
                std::io::Error::last_os_error()
            ));
        }
        let want: libc::rlim_t = bytes;
        // ハードの上限は下げることしかできない（特権がない場合）。
        let hard = if current.rlim_max == libc::RLIM_INFINITY {
            want
        } else {
            want.min(current.rlim_max)
        };
        let limit = libc::rlimit {
            rlim_cur: hard,
            rlim_max: hard,
        };
        // SAFETY: setrlimit は、渡した有効な構造体を読んでこのプロセスの上限を設定するだけ。
        if unsafe { libc::setrlimit(libc::RLIMIT_DATA, &limit) } != 0 {
            return not_applied(format!(
                "setrlimit に失敗: {}",
                std::io::Error::last_os_error()
            ));
        }
        MemoryLimitStatus::Applied { bytes: hard }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::MemoryLimitStatus;

    pub(super) fn apply(bytes: u64) -> MemoryLimitStatus {
        MemoryLimitStatus::NotApplied {
            bytes,
            reason: "この OS ではワーカーのメモリの上限に未対応（PoC-2 で確認する）".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_small_limits_are_not_applied() {
        // テストのプロセスに上限をかけないよう、設定されない値だけを確かめる。
        assert!(matches!(
            apply_memory_limit(1024),
            MemoryLimitStatus::NotApplied { bytes: 1024, .. }
        ));
    }
}
