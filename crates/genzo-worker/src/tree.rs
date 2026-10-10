//! ワーカーと、ワーカーが起動した子プロセス（本体から見ると孫プロセス。ffmpeg・ffprobe、テスト用の
//! 子プロセス）をまとめて終わらせる仕組みと、Windows でのワーカーのメモリの上限（決定 No.12。
//! docs/04_architecture.md の 1.2 節、9 章の AR-10）。
//!
//! | OS | まとめ方 | まとめて終わらせる方法 | メモリの上限 |
//! |---|---|---|---|
//! | Linux・macOS | プロセスグループ（本体が起動時に `process_group(0)`（`setpgid`）で新しいグループにする。グループの ID はワーカーのプロセス ID。孫プロセスは同じグループに入る） | `killpg(SIGKILL)` | Linux はワーカー自身が `RLIMIT_DATA` を設定する（[`crate::limits`]。孫プロセスにも引き継がれる）。macOS は設けない |
//! | Windows | ジョブオブジェクト（本体が起動の直後、握手の前にワーカーを入れる。孫プロセスは自動で同じジョブに入る） | `TerminateJobObject`。本体が異常終了してジョブのハンドルが閉じたときも、`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` で終わる | `JOB_OBJECT_LIMIT_PROCESS_MEMORY`（ジョブの各プロセスごと。孫プロセスにもかかる） |
//!
//! まとめて終わらせるのは、本体がワーカーを回収するとき（タイムアウト・取り消し・異常終了・不正な応答・
//! 不正なバッファの後の再起動、終了の依頼の後、[`crate::WorkerClient`] の drop）。ワーカーが自分で
//! 終わった（異常終了した）後も、残った孫プロセスを終わらせる。ワーカーは、本体が終わって標準入力が
//! 閉じたときも、自分のプロセスグループごと終わる（Unix。`kill_own_group`。Windows はジョブの
//! ハンドルが閉じることで終わる）。
//!
//! # 注意
//!
//! - **macOS と Windows は実機で未確認**（Linux の結合テストだけで確かめた。tests/process.rs の
//!   `grandchildren`）。Windows のコードはクロスコンパイルの型の確認（clippy）だけ。
//! - Unix で、ワーカーが自分で終わって回収した後にグループへ送る場合: グループに孫プロセスが残って
//!   いる間は、OS はグループの ID（＝ワーカーのプロセス ID）を再利用しない。グループが空になった後は、
//!   同じ番号のプロセスが新しく作られ、さらに自分のグループを作った場合に限って、そのグループに届きうる。
//!   回収から送るまでの間（ミリ秒以下）にプロセス ID が一巡する必要があるので、実際には起きないとみなす
//!   （送るのは 1 つのワーカーにつき 1 回だけ）。
//! - 孫プロセスが自分で別のグループ・セッションを作った場合（`setsid` など）は対象外（ffmpeg・ffprobe は
//!   作らない）。Windows では、ジョブから抜ける指定（`CREATE_BREAKAWAY_FROM_JOB`）は許可していないので、
//!   孫プロセスはジョブから抜けられない。
//! - Windows では、ワーカーを起動してからジョブに入れるまでの間に、ワーカーが子プロセスを作ることはない
//!   （子プロセスは依頼を受けてから作り、最初の依頼（握手）はジョブに入れた後に送る）。上限は、ジョブに
//!   入れた後の確保からかかる。
//! - ジョブオブジェクトを作れない・入れられない場合（Windows）は、ワーカーだけを終わらせる（孫プロセスは
//!   出力のパイプが閉じた時点で終わる）。メモリの上限は [`MemoryLimitStatus::NotApplied`] で知らせる。

use std::process::Child;

use crate::protocol::MemoryLimitStatus;

/// ワーカーとその子プロセスのまとまり（Unix はプロセスグループ、Windows はジョブオブジェクト）。
pub(crate) struct ProcessTree {
    imp: imp::Tree,
}

impl std::fmt::Debug for ProcessTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessTree").finish_non_exhaustive()
    }
}

impl ProcessTree {
    /// 起動した直後のワーカー `child` を登録する（Windows ではジョブオブジェクトに入れ、`memory_limit`
    /// （バイト）があれば、ジョブのプロセスごとのメモリの上限にする）。
    ///
    /// 返す 2 つ目の値は、本体が設定したメモリの上限の状態（Windows で上限を指定した場合だけ。Linux・macOS
    /// では `None` で、ワーカーが握手の応答で返す状態を使う）。
    pub(crate) fn attach(
        child: &Child,
        memory_limit: Option<u64>,
    ) -> (Self, Option<MemoryLimitStatus>) {
        let (imp, status) = imp::Tree::attach(child, memory_limit);
        (Self { imp }, status)
    }

    /// ワーカーと、ワーカーが起動した子プロセスをすべて強制終了する（終わるのは待たない）。
    pub(crate) fn kill_all(&self) {
        self.imp.kill_all();
    }
}

/// ワーカーの側: 自分のプロセスグループ（自分と、自分が起動した子プロセス）をすべて強制終了する
/// （本体が終わって標準入力が閉じたとき。Unix だけ）。
///
/// 自分がグループの先頭（本体が `process_group(0)` で起動した）の場合だけ送る。自分で起動した場合
/// など、ほかのプロセスと同じグループにいる場合は、ほかのプロセスを巻き込まないよう何もしない。
/// 送れた場合はこの関数から戻らない（自分も終わる）。Windows では何もしない（ジョブのハンドルが
/// 閉じたときに OS が終わらせる）。
pub(crate) fn kill_own_group() {
    imp::kill_own_group();
}

#[cfg(unix)]
mod imp {
    use std::process::Child;

    use crate::protocol::MemoryLimitStatus;

    pub(super) struct Tree {
        /// プロセスグループの ID（＝ワーカーのプロセス ID。`process_group(0)` で起動したため）。
        group: Option<libc::pid_t>,
    }

    impl Tree {
        pub(super) fn attach(
            child: &Child,
            _memory_limit: Option<u64>,
        ) -> (Self, Option<MemoryLimitStatus>) {
            // 0 以下の ID でグループに送ると、本体のグループ（0）やすべてのプロセス（-1）に届くので、
            // 正の値だけを使う。
            let group = libc::pid_t::try_from(child.id()).ok().filter(|&g| g > 1);
            (Self { group }, None)
        }

        pub(super) fn kill_all(&self) {
            if let Some(group) = self.group {
                // SAFETY: killpg はシグナルを送るだけで、メモリを扱わない。group は 1 より大きい正の値
                // （ワーカーのプロセスグループ）なので、本体や無関係なプロセスのグループには送らない。
                // 失敗（既に誰もいない ESRCH など）は無視する。
                unsafe {
                    libc::killpg(group, libc::SIGKILL);
                }
            }
        }
    }

    pub(super) fn kill_own_group() {
        // SAFETY: getpgrp・getpid はこのプロセスの ID を返すだけ。
        let (group, me) = unsafe { (libc::getpgrp(), libc::getpid()) };
        if group == me && group > 1 {
            // SAFETY: 自分が先頭のグループ（自分と、自分が起動した子プロセス）にだけ送る。
            unsafe {
                libc::killpg(group, libc::SIGKILL);
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };

    use crate::protocol::MemoryLimitStatus;

    /// ジョブオブジェクトのハンドル（作れなかった場合は `None`）。
    pub(super) struct Tree {
        job: Option<Job>,
    }

    /// 閉じるとジョブの中のプロセスがすべて終わる（`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`）。
    struct Job(HANDLE);

    // SAFETY: ジョブオブジェクトのハンドルはカーネルのオブジェクトを指す値で、どのスレッドから使っても
    // よい。ここで呼ぶ Win32 の関数（TerminateJobObject・CloseHandle）はスレッドに依存しない。
    unsafe impl Send for Job {}
    // SAFETY: 同上（共有しても、ハンドルの値を読むだけ）。
    unsafe impl Sync for Job {}

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: CreateJobObjectW が返した有効なハンドルを 1 回だけ閉じる。
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    fn last_error(what: &str) -> String {
        format!("{what} に失敗: {}", std::io::Error::last_os_error())
    }

    /// ジョブオブジェクトを作り、制限を設定して `child` を入れる。
    fn create(child: &Child, memory_limit: Option<u64>) -> Result<Job, String> {
        // SAFETY: 既定のセキュリティ属性（null）・名前なしで作る。失敗すると null を返す。
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(last_error("CreateJobObjectW"));
        }
        let job = Job(handle);
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Some(bytes) = memory_limit {
            info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            info.ProcessMemoryLimit = usize::try_from(bytes).unwrap_or(usize::MAX);
        }
        let size = u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
            .expect("構造体の大きさは u32 に収まる");
        // SAFETY: info は有効な JOBOBJECT_EXTENDED_LIMIT_INFORMATION で、size はその大きさ。関数は
        // 読むだけ。
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                size,
            )
        };
        if ok == 0 {
            return Err(last_error("SetInformationJobObject"));
        }
        // SAFETY: child のプロセスのハンドルは、child が生きている間有効。
        let ok = unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) };
        if ok == 0 {
            return Err(last_error("AssignProcessToJobObject"));
        }
        Ok(job)
    }

    impl Tree {
        pub(super) fn attach(
            child: &Child,
            memory_limit: Option<u64>,
        ) -> (Self, Option<MemoryLimitStatus>) {
            match create(child, memory_limit) {
                Ok(job) => (
                    Self { job: Some(job) },
                    memory_limit.map(|bytes| MemoryLimitStatus::Applied { bytes }),
                ),
                Err(reason) => (
                    Self { job: None },
                    memory_limit.map(|bytes| MemoryLimitStatus::NotApplied {
                        bytes,
                        reason: format!("ジョブオブジェクトを使えない（{reason}）"),
                    }),
                ),
            }
        }

        pub(super) fn kill_all(&self) {
            if let Some(job) = &self.job {
                // SAFETY: 有効なジョブのハンドル。失敗（既に誰もいないなど）は無視する。
                unsafe {
                    TerminateJobObject(job.0, 1);
                }
            }
        }
    }

    pub(super) fn kill_own_group() {}
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::process::Child;

    use crate::protocol::MemoryLimitStatus;

    pub(super) struct Tree;

    impl Tree {
        pub(super) fn attach(
            _child: &Child,
            _memory_limit: Option<u64>,
        ) -> (Self, Option<MemoryLimitStatus>) {
            (Self, None)
        }

        pub(super) fn kill_all(&self) {}
    }

    pub(super) fn kill_own_group() {}
}
