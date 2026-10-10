//! ワーカーのメモリの上限（docs/04_architecture.md の 1.2 節「ワーカーごとにメモリの上限を設けます」、
//! 9 章の AR-10、決定 No.12）。
//!
//! - **既定値**: [`DEFAULT_MEMORY_LIMIT_BYTES`]（2 GiB。仮置き）。[`crate::WorkerConfig::memory_limit_bytes`]
//!   で変えられる（`None` なら上限を設けない）。
//! - **Linux**: ワーカーが起動直後に自分の `RLIMIT_DATA`（`setrlimit`）を設定する。Linux 4.7 以降の
//!   `RLIMIT_DATA` は、ヒープと、共有でない書き込み可能なマップ（大きな `malloc`、スレッドの
//!   スタック）の合計を制限する。共有メモリ（[`crate::shm`] のファイルの共有マップ）は含まない。
//!   上限を超える確保は失敗し、Rust の確保では異常終了（`handle_alloc_error`）になる。本体はそれを
//!   ワーカーの異常終了として扱う。ソフトとハードの両方を下げるので、ワーカー自身は上限を戻せない。
//!   上限はワーカーが起動する子プロセス（ffmpeg・ffprobe）にも引き継がれる（プロセスごとに同じ上限）。
//! - **Windows**: 本体がワーカーを入れるジョブオブジェクトの `JOB_OBJECT_LIMIT_PROCESS_MEMORY`
//!   （プロセスごとのコミットの上限。孫プロセスにもかかる。[`crate::tree`]）。ワーカーの側では設定しない
//!   （本体は Windows では `--memory-limit-bytes` を渡さず、握手の応答の状態を本体の側の結果で置き換える。
//!   [`crate::WorkerClient::ping`] の応答も、この握手で確定した状態を返す）。
//!   **実機で未確認。**
//! - **macOS**: 設けない（`RLIMIT_DATA` は `mmap` による確保を制限しないとされ、効果が限られるため。
//!   決定 No.12）。上限を指定しても [`MemoryLimitStatus::NotApplied`] を返す。

use crate::protocol::MemoryLimitStatus;

/// 上限として受け付ける最小の値（バイト）。
///
/// **仮置き**: 64 MiB。これより小さいと、ワーカーの起動（LibRaw・lcms2 の初期化、スレッドの
/// スタック）にも足りなくなるおそれがある。
pub const MIN_MEMORY_LIMIT_BYTES: u64 = 64 * 1024 * 1024;

/// ワーカーのメモリの上限の既定値（バイト。[`crate::WorkerConfig::memory_limit_bytes`]）。
///
/// **仮置き**: 2 GiB。実機の ARW（α7C と、α7 IV の APS-C クロップ）の展開で計測したワーカーのピーク
/// （Linux の VmHWM）は約 96〜104 MB（PoC-2）。α7 IV のフルサイズ（約 3,300 万画素。CFA だけで約 66 MB）の
/// 展開、大きな JPEG / TIFF のデコード、子プロセス（ffmpeg。同じ上限が引き継がれる）を見込んで余裕を
/// 持たせた。画素数の上限（genzo-media の `MAX_IMAGE_PIXELS`、2 億画素）に近い 16 bit の TIFF などは、
/// この上限に当たってデコードに失敗しうる（未計測）。PoC-2 で計測して見直す。
pub const DEFAULT_MEMORY_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

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
        let reason = if cfg!(windows) {
            "Windows では本体がジョブオブジェクトで設定する（ワーカーの側では設定しない）"
        } else if cfg!(target_os = "macos") {
            "macOS ではワーカーのメモリの上限を設けない（決定 No.12）"
        } else {
            "この OS ではワーカーのメモリの上限に未対応"
        };
        MemoryLimitStatus::NotApplied {
            bytes,
            reason: reason.to_owned(),
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
