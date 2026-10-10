//! 共有メモリ（docs/04_architecture.md の 1.2 節「画像のデータは共有メモリで渡します。共有メモリは
//! 本体が確保して所有し、ワーカーは書き込むだけです。ワーカーが異常終了しても、本体が回収します」）。
//!
//! # 方式: ファイルを使う共有メモリ
//!
//! 1. 本体は、自分専用の一時ディレクトリ（[`ShmArena`]。`genzo-shm-{本体のプロセス ID}-` で始まる名前）
//!    に、依頼ごとにファイルを作り、大きさ（[`ShmBuffer::capacity`]）を決める（[`ShmArena::allocate`]）。
//!    ファイルは作った直後なので中身はすべて 0（ヘッダの印もない）。
//! 2. 依頼（[`crate::protocol::ShmRef`]）でファイルのパスと大きさを渡す。ワーカーはファイルを開いて
//!    大きさが一致することを確かめ、memmap2 でマップして、データ → ヘッダの順に書き、マップを
//!    外してから応答する（[`ShmWriter`]）。ワーカーはファイルを作らず、大きさも変えない。
//! 3. 本体は、応答を受け取ってからヘッダと付随情報を検証し、データを位置を指定した読み取りで
//!    自分のメモリに直接複製する（[`ShmBuffer::open_payload`]、`crate::validate`）。本体はファイルを
//!    **マップしない**（下の「注意」の SIGBUS を避けるため）。
//! 4. [`ShmBuffer`] を drop するとファイルを削除する。ワーカーが異常終了・タイムアウトした場合も、
//!    本体がワーカーを終了させてから drop するので、ファイルは残らない。[`ShmArena`] を drop すると
//!    ディレクトリごと削除する。
//!
//! 本体はデータを自分のメモリに複製してから中身を検証して使うため、その後にワーカーがデータを
//! 書き換えても、本体が使う値は変わらない（複製の費用は PoC-2 で計測する）。
//!
//! # ヘッダ（[`SHM_HEADER_LEN`] = 64 バイト、リトルエンディアン）
//!
//! | 位置 | 大きさ | 内容 |
//! |---|---|---|
//! | 0 | 8 | 印 [`SHM_MAGIC`]（`"GENZOSHM"`） |
//! | 8 | 4 | 形式の版 [`SHM_FORMAT_VERSION`] |
//! | 12 | 4 | データの種類（[`PayloadKind::code`]） |
//! | 16 | 4 | 幅 |
//! | 20 | 4 | 高さ |
//! | 24 | 4 | 1 画素のチャンネル数（CFA は 1、RGB は 3、JPEG は 0） |
//! | 28 | 4 | 1 チャンネルのバイト数（CFA は 2、RGB は 4、JPEG は 1） |
//! | 32 | 8 | データの長さ（バイト） |
//! | 40 | 4 | フラグ（ビット 0: チェックサムあり） |
//! | 44 | 4 | 予約（0） |
//! | 48 | 8 | チェックサム（[`checksum`]。フラグがなければ 0） |
//! | 56 | 8 | 予約（0） |
//!
//! データはヘッダの直後（64 バイト目から）に置く。値はリトルエンディアン（CFA は u16、RGB は f32 の
//! R, G, B の順、行優先）。JPEG はファイルのバイト列そのもの。
//!
//! # 注意・PoC-2 で確認すること（AR-10）
//!
//! - **OS ごとの名前付き共有メモリ**（Windows のページファイルを裏付けにしたセクション、macOS の
//!   `shm_open` / Mach のメモリ、Linux の `memfd`）との比較は PoC-2 で行う。ファイルの方式は、
//!   OS が汚れたページをディスクに書き出す場合がある（一時ファイルなので削除すれば書き出しは
//!   打ち切られることが多い）。受け渡しの時間は PoC-2 で計測する。
//! - ディスクの空きがないと、ワーカーがマップに書き込んだ時点で異常終了しうる（Unix の SIGBUS、
//!   Windows の EXCEPTION_IN_PAGE_ERROR）。その場合も本体はワーカーの異常終了として扱う。
//! - Unix では、マップしたファイルが切り詰められると、マップを読んだプロセスが SIGBUS で落ちる。
//!   乗っ取られたワーカーは、本体が大きさを確かめた後にファイルを切り詰められるので、本体は
//!   マップせず `pread`（Windows は `ReadFile`）で読む（切り詰められても短く読めるだけで、
//!   [`BufferError::FileSizeChanged`] になる）。ワーカー側のマップへの書き込みは、ワーカーが
//!   落ちるだけで済む。Linux の `memfd` と `F_SEAL_SHRINK` などとの比較は PoC-2 で行う。
//! - 本体が異常終了した場合、一時ディレクトリが残る。次の起動時に [`purge_stale_arenas`] で削除する
//!   （同時に 1 つしか起動しないことを本体側で保証したうえで呼ぶ）。

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use memmap2::MmapMut;

use crate::protocol::{ShmRef, WorkerError, WorkerErrorKind};

/// ヘッダの先頭の印。
pub const SHM_MAGIC: [u8; 8] = *b"GENZOSHM";
/// ヘッダの形式の版。
pub const SHM_FORMAT_VERSION: u32 = 1;
/// ヘッダの大きさ（バイト）。データはこの位置から始まる（f32 / u16 の境界にそろう）。
pub const SHM_HEADER_LEN: usize = 64;
/// 一時ディレクトリの名前の先頭。
pub const ARENA_DIR_PREFIX: &str = "genzo-shm-";

/// フラグ: チェックサムあり。
const FLAG_CHECKSUM: u32 = 1;
/// 知っているフラグ。
const KNOWN_FLAGS: u32 = FLAG_CHECKSUM;

/// ヘッダの各項目の位置。
mod at {
    pub const MAGIC: usize = 0;
    pub const VERSION: usize = 8;
    pub const KIND: usize = 12;
    pub const WIDTH: usize = 16;
    pub const HEIGHT: usize = 20;
    pub const CHANNELS: usize = 24;
    pub const BYTES_PER_SAMPLE: usize = 28;
    pub const DATA_LEN: usize = 32;
    pub const FLAGS: usize = 40;
    pub const RESERVED1: usize = 44;
    pub const CHECKSUM: usize = 48;
    pub const RESERVED2: usize = 56;
}

/// 共有メモリのデータの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PayloadKind {
    /// CFA（u16、1 チャンネル）。[`crate::protocol::Job::DecodeRaw`]。
    CfaU16,
    /// リニア BT.2020 の RGB（f32、3 チャンネル）。[`crate::protocol::Job::DecodeImage`]。
    RgbF32,
    /// JPEG のバイト列。[`crate::protocol::Job::Thumbnail`] など。
    Jpeg,
}

impl PayloadKind {
    /// ヘッダに書く番号。
    pub const fn code(self) -> u32 {
        match self {
            PayloadKind::CfaU16 => 1,
            PayloadKind::RgbF32 => 2,
            PayloadKind::Jpeg => 3,
        }
    }

    /// 番号から作る。
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            1 => Some(PayloadKind::CfaU16),
            2 => Some(PayloadKind::RgbF32),
            3 => Some(PayloadKind::Jpeg),
            _ => None,
        }
    }

    /// 1 画素のチャンネル数（JPEG は 0）。
    pub const fn channels(self) -> u32 {
        match self {
            PayloadKind::CfaU16 => 1,
            PayloadKind::RgbF32 => 3,
            PayloadKind::Jpeg => 0,
        }
    }

    /// 1 チャンネルのバイト数（JPEG は 1）。
    pub const fn bytes_per_sample(self) -> u32 {
        match self {
            PayloadKind::CfaU16 => 2,
            PayloadKind::RgbF32 => 4,
            PayloadKind::Jpeg => 1,
        }
    }

    /// 寸法から決まるデータの長さ（バイト）。JPEG は寸法から決まらないので `None`。
    pub fn data_len_for(self, width: u32, height: u32) -> Option<u64> {
        let per_pixel = u64::from(self.channels()) * u64::from(self.bytes_per_sample());
        if per_pixel == 0 {
            return None;
        }
        u64::from(width)
            .checked_mul(u64::from(height))?
            .checked_mul(per_pixel)
    }
}

/// 共有メモリのヘッダ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShmHeader {
    /// データの種類。
    pub kind: PayloadKind,
    /// 幅。
    pub width: u32,
    /// 高さ。
    pub height: u32,
    /// データの長さ（バイト）。
    pub data_len: u64,
    /// データのチェックサム（[`checksum`]）。
    pub checksum: Option<u64>,
}

impl ShmHeader {
    /// バイト列にする。
    pub fn encode(&self) -> [u8; SHM_HEADER_LEN] {
        let mut b = [0u8; SHM_HEADER_LEN];
        b[at::MAGIC..at::MAGIC + 8].copy_from_slice(&SHM_MAGIC);
        put_u32(&mut b, at::VERSION, SHM_FORMAT_VERSION);
        put_u32(&mut b, at::KIND, self.kind.code());
        put_u32(&mut b, at::WIDTH, self.width);
        put_u32(&mut b, at::HEIGHT, self.height);
        put_u32(&mut b, at::CHANNELS, self.kind.channels());
        put_u32(&mut b, at::BYTES_PER_SAMPLE, self.kind.bytes_per_sample());
        put_u64(&mut b, at::DATA_LEN, self.data_len);
        let flags = if self.checksum.is_some() {
            FLAG_CHECKSUM
        } else {
            0
        };
        put_u32(&mut b, at::FLAGS, flags);
        put_u64(&mut b, at::CHECKSUM, self.checksum.unwrap_or(0));
        b
    }

    /// バイト列から読む。印・版・種類・チャンネル数とバイト数の組・フラグ・予約の 0 を確かめる。
    ///
    /// 寸法とデータの長さの妥当性（上限・寸法との一致）は確かめない（使う側が種類ごとに確かめる）。
    pub fn decode(bytes: &[u8]) -> Result<Self, BufferError> {
        if bytes.len() < SHM_HEADER_LEN {
            return Err(BufferError::HeaderTooShort(bytes.len()));
        }
        if bytes[at::MAGIC..at::MAGIC + 8] != SHM_MAGIC {
            return Err(BufferError::BadMagic);
        }
        let version = get_u32(bytes, at::VERSION);
        if version != SHM_FORMAT_VERSION {
            return Err(BufferError::UnsupportedVersion(version));
        }
        let code = get_u32(bytes, at::KIND);
        let kind = PayloadKind::from_code(code).ok_or(BufferError::UnknownKind(code))?;
        let channels = get_u32(bytes, at::CHANNELS);
        let bps = get_u32(bytes, at::BYTES_PER_SAMPLE);
        if channels != kind.channels() || bps != kind.bytes_per_sample() {
            return Err(BufferError::InconsistentLayout(format!(
                "種類 {kind:?} のチャンネル数とバイト数は {}・{} のはずが {channels}・{bps}",
                kind.channels(),
                kind.bytes_per_sample()
            )));
        }
        let flags = get_u32(bytes, at::FLAGS);
        if flags & !KNOWN_FLAGS != 0 {
            return Err(BufferError::UnknownFlags(flags));
        }
        if get_u32(bytes, at::RESERVED1) != 0 || get_u64(bytes, at::RESERVED2) != 0 {
            return Err(BufferError::InconsistentLayout(
                "予約の領域が 0 ではない".to_owned(),
            ));
        }
        let raw_checksum = get_u64(bytes, at::CHECKSUM);
        let checksum = if flags & FLAG_CHECKSUM != 0 {
            Some(raw_checksum)
        } else if raw_checksum != 0 {
            return Err(BufferError::InconsistentLayout(
                "チェックサムのフラグがないのに値がある".to_owned(),
            ));
        } else {
            None
        };
        Ok(Self {
            kind,
            width: get_u32(bytes, at::WIDTH),
            height: get_u32(bytes, at::HEIGHT),
            data_len: get_u64(bytes, at::DATA_LEN),
            checksum,
        })
    }
}

fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_u64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

fn get_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn get_u64(b: &[u8], at: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(a)
}

/// データのチェックサム（64bit）。
///
/// 書きかけのデータや、別の依頼のデータを読んでしまう誤りを見つけるためのもので、暗号学的な
/// 強さはない（ワーカーが意図して偽る場合は防げない）。8 バイトごとに回転・排他的論理和・乗算で
/// 混ぜ、最後に splitmix64 の仕上げをかける。長さも混ぜる。
pub fn checksum(data: &[u8]) -> u64 {
    const MUL: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut h: u64 = 0x243F_6A88_85A3_08D3 ^ (data.len() as u64).wrapping_mul(MUL);
    let mut chunks = data.chunks_exact(8);
    for c in &mut chunks {
        let mut w = [0u8; 8];
        w.copy_from_slice(c);
        h = (h.rotate_left(23) ^ u64::from_le_bytes(w)).wrapping_mul(MUL);
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let mut w = [0u8; 8];
        w[..rest.len()].copy_from_slice(rest);
        h = (h.rotate_left(23) ^ u64::from_le_bytes(w)).wrapping_mul(MUL);
    }
    // splitmix64 の仕上げ。
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

/// 共有メモリ（受け取ったバッファ）の検証のエラー。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BufferError {
    /// ヘッダが短い。
    #[error("ヘッダが短い（{0} バイト）")]
    HeaderTooShort(usize),
    /// 印が違う（ワーカーが書いていない、または別のデータ）。
    #[error("ヘッダの印が違う")]
    BadMagic,
    /// 形式の版が違う。
    #[error("ヘッダの形式の版 {0} に対応していない")]
    UnsupportedVersion(u32),
    /// 種類の番号を知らない。
    #[error("データの種類 {0} を知らない")]
    UnknownKind(u32),
    /// 期待した種類と違う。
    #[error("データの種類が {actual:?}（期待したのは {expected:?}）")]
    UnexpectedKind {
        /// 期待した種類。
        expected: PayloadKind,
        /// 実際の種類。
        actual: PayloadKind,
    },
    /// ヘッダの項目の組み合わせが不正。
    #[error("ヘッダが不正: {0}")]
    InconsistentLayout(String),
    /// 知らないフラグ。
    #[error("ヘッダのフラグ {0:#x} を知らない")]
    UnknownFlags(u32),
    /// 寸法が不正（0 を含む）。
    #[error("寸法が不正（{width} × {height}）")]
    InvalidDimensions {
        /// 幅。
        width: u32,
        /// 高さ。
        height: u32,
    },
    /// 画素数が上限を超える。
    #[error("画素数 {pixels} が上限 {max} を超える")]
    TooManyPixels {
        /// 画素数。
        pixels: u64,
        /// 上限。
        max: u64,
    },
    /// データの長さが寸法・付随情報と合わない。
    #[error("データの長さ {actual} が {expected} と合わない")]
    LengthMismatch {
        /// 期待した長さ。
        expected: u64,
        /// ヘッダの長さ。
        actual: u64,
    },
    /// データが共有メモリの大きさを超える。
    #[error("データ（ヘッダを含めて {needed} バイト）が共有メモリの大きさ {capacity} を超える")]
    BeyondCapacity {
        /// 必要な大きさ。
        needed: u64,
        /// 共有メモリの大きさ。
        capacity: u64,
    },
    /// データが上限より大きい（JPEG など）。
    #[error("データの長さ {len} が上限 {max} を超える")]
    DataTooLarge {
        /// 長さ。
        len: u64,
        /// 上限。
        max: u64,
    },
    /// ヘッダの寸法と付随情報（JSON）の寸法が違う。
    #[error("ヘッダの寸法 {header:?} と付随情報の寸法 {info:?} が違う")]
    DimensionMismatch {
        /// ヘッダの（幅, 高さ）。
        header: (u32, u32),
        /// 付随情報の（幅, 高さ）。
        info: (u32, u32),
    },
    /// チェックサムを求めたのに、ヘッダにない。
    #[error("ヘッダにチェックサムがない")]
    ChecksumMissing,
    /// チェックサムが合わない。
    #[error("チェックサムが合わない")]
    ChecksumMismatch,
    /// ファイルの大きさが確保したときから変わった。
    #[error("共有メモリのファイルの大きさが {actual}（確保したのは {expected}）")]
    FileSizeChanged {
        /// 確保した大きさ。
        expected: u64,
        /// 実際の大きさ。
        actual: u64,
    },
    /// データの中身が不正（`RawImage::validate` など）。
    #[error("データが不正: {0}")]
    Invalid(String),
    /// 共有メモリを読めない。
    #[error("共有メモリを読めない: {0}")]
    Io(String),
}

/// 本体が所有する、共有メモリのファイルを置く一時ディレクトリ。
///
/// drop するとディレクトリごと削除する。
#[derive(Debug)]
pub struct ShmArena {
    dir: tempfile::TempDir,
    next: AtomicU64,
}

impl ShmArena {
    /// OS の一時ディレクトリ（`std::env::temp_dir()`）の下に作る。
    pub fn new() -> io::Result<Self> {
        Self::new_in(&std::env::temp_dir())
    }

    /// `root` の下に作る。
    pub fn new_in(root: &Path) -> io::Result<Self> {
        let prefix = format!("{ARENA_DIR_PREFIX}{}-", std::process::id());
        // Unix では 0700 で作られる（tempfile の既定）。
        let dir = tempfile::Builder::new().prefix(&prefix).tempdir_in(root)?;
        Ok(Self {
            dir,
            next: AtomicU64::new(0),
        })
    }

    /// ディレクトリのパス。
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// `capacity` バイト（[`SHM_HEADER_LEN`] 以上）の共有メモリを作る。中身はすべて 0。
    pub fn allocate(&self, capacity: u64) -> io::Result<ShmBuffer> {
        if capacity < SHM_HEADER_LEN as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "共有メモリの大きさ {capacity} がヘッダの大きさ {SHM_HEADER_LEN} より小さい"
                ),
            ));
        }
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let path = self.dir.path().join(format!("{n}.shm"));
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            /// FILE_ATTRIBUTE_TEMPORARY（Win32）。キャッシュからディスクへの書き出しを控えさせる。
            const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x100;
            opts.attributes(FILE_ATTRIBUTE_TEMPORARY);
        }
        let file = opts.open(&path)?;
        let buffer = ShmBuffer {
            file: Some(file),
            path,
            capacity,
        };
        // 失敗したら drop でファイルを削除する。
        buffer.file().set_len(capacity)?;
        Ok(buffer)
    }

    /// ディレクトリに残っている共有メモリのファイルの数（診断・テスト用）。
    pub fn live_buffers(&self) -> io::Result<usize> {
        let mut n = 0;
        for entry in fs::read_dir(self.dir.path())? {
            if entry?.file_type()?.is_file() {
                n += 1;
            }
        }
        Ok(n)
    }
}

/// `root` の下にある、[`ShmArena`] の一時ディレクトリ（`genzo-shm-` で始まるもの）を削除する。
///
/// 本体が異常終了して残ったディレクトリを、次の起動時に掃除するためのもの。**他の本体のプロセスが
/// 動いていないことを確かめてから呼ぶ**（動いているプロセスの共有メモリも消してしまうため）。
/// `keep` のディレクトリは残す。削除した数を返す。
pub fn purge_stale_arenas(root: &Path, keep: Option<&ShmArena>) -> io::Result<usize> {
    let keep = keep.map(|a| a.path().to_path_buf());
    let mut removed = 0;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let is_arena = name
            .to_str()
            .is_some_and(|n| n.starts_with(ARENA_DIR_PREFIX));
        if !is_arena || !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if keep.as_deref() == Some(path.as_path()) {
            continue;
        }
        if fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// 本体が所有する共有メモリ（1 つのファイル）。drop するとファイルを削除する。
#[derive(Debug)]
pub struct ShmBuffer {
    /// 削除の前に閉じる（Windows では開いているファイルを削除できないため）。
    file: Option<File>,
    path: PathBuf,
    capacity: u64,
}

impl ShmBuffer {
    fn file(&self) -> &File {
        self.file.as_ref().expect("drop の前は常に Some")
    }

    /// ファイルのパス。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 大きさ（バイト。ヘッダを含む）。
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// 依頼に入れる参照。
    pub fn shm_ref(&self, checksum: bool) -> ShmRef {
        ShmRef {
            path: self.path.clone(),
            capacity: self.capacity,
            checksum,
        }
    }

    /// ワーカーが書いたデータのヘッダを読んで検証する。データはまだ読まない
    /// （[`ReceivedPayload::read_into`] で、本体のメモリに直接読む）。
    ///
    /// 確かめること: ファイルの大きさが確保したときのまま、ヘッダの形式（[`ShmHeader::decode`]）、
    /// 種類が `expected`、データが共有メモリに収まる、`verify_checksum` のときはヘッダに
    /// チェックサムがあること（値はデータを読むときに確かめる）。寸法とデータの長さの関係・上限は、
    /// 種類ごとに使う側で確かめる（`crate::validate`）。
    ///
    /// 本体はファイルを **マップせず**、位置を指定した読み取り（`pread` / `ReadFile`）で複製する。
    /// マップして読むと、乗っ取られたワーカーが検証の後にファイルを切り詰めた場合に、本体が
    /// SIGBUS（Unix）で落ちるため（SEC-05。読み取りなら、短く読めるだけでエラーとして扱える）。
    /// 複製は 1 回で、マップしてから複製する場合と費用は変わらない。
    pub fn open_payload(
        &self,
        expected: PayloadKind,
        verify_checksum: bool,
    ) -> Result<ReceivedPayload<'_>, BufferError> {
        let file = self.file();
        let len = self.current_len()?;
        if len != self.capacity {
            return Err(BufferError::FileSizeChanged {
                expected: self.capacity,
                actual: len,
            });
        }
        let mut bytes = [0u8; SHM_HEADER_LEN];
        read_exact_at(file, &mut bytes, 0).map_err(|e| self.read_error(&e))?;
        let header = ShmHeader::decode(&bytes)?;
        if header.kind != expected {
            return Err(BufferError::UnexpectedKind {
                expected,
                actual: header.kind,
            });
        }
        let needed = (SHM_HEADER_LEN as u64).saturating_add(header.data_len);
        if needed > self.capacity {
            return Err(BufferError::BeyondCapacity {
                needed,
                capacity: self.capacity,
            });
        }
        if verify_checksum && header.checksum.is_none() {
            return Err(BufferError::ChecksumMissing);
        }
        Ok(ReceivedPayload {
            buffer: self,
            header,
            verify_checksum,
        })
    }

    /// ファイルの今の大きさ。
    fn current_len(&self) -> Result<u64, BufferError> {
        Ok(self
            .file()
            .metadata()
            .map_err(|e| BufferError::Io(e.to_string()))?
            .len())
    }

    /// 読み取りのエラー。途中で終わった（切り詰められた）場合は [`BufferError::FileSizeChanged`]。
    fn read_error(&self, e: &io::Error) -> BufferError {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            let actual = self.current_len().unwrap_or(0);
            if actual != self.capacity {
                return BufferError::FileSizeChanged {
                    expected: self.capacity,
                    actual,
                };
            }
        }
        BufferError::Io(e.to_string())
    }
}

/// `file` の `offset` から `buf` の長さだけ読む（ファイルの位置は共有しない）。
#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

/// `file` の `offset` から `buf` の長さだけ読む（ファイルの位置は共有しない）。
#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Ok(n) => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// `file` の `offset` から `buf` の長さだけ読む。
#[cfg(not(any(unix, windows)))]
fn read_exact_at(mut file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf)
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        drop(self.file.take());
        // 失敗しても、ShmArena の drop でディレクトリごと削除する。
        let _ = fs::remove_file(&self.path);
    }
}

/// 受け取ったデータ（ヘッダは検証済み。データは [`Self::read_into`] で読む）。
///
/// 共有メモリ（[`ShmBuffer`]）を借りるので、共有メモリより先に削除されることはない。
#[derive(Debug)]
pub struct ReceivedPayload<'a> {
    buffer: &'a ShmBuffer,
    header: ShmHeader,
    verify_checksum: bool,
}

impl ReceivedPayload<'_> {
    /// ヘッダ。
    pub fn header(&self) -> &ShmHeader {
        &self.header
    }

    /// データ（ヘッダの直後から、ヘッダのデータの長さだけ）を `dst` に読む。`dst` の長さは
    /// データの長さと同じにする（違えば [`BufferError::LengthMismatch`]）。
    ///
    /// チェックサムを確かめる設定なら、読んだ値で確かめる（読んだ後にワーカーが書き換えても、
    /// 本体が使う値は変わらない）。
    pub fn read_into(&self, dst: &mut [u8]) -> Result<(), BufferError> {
        if dst.len() as u64 != self.header.data_len {
            return Err(BufferError::LengthMismatch {
                expected: self.header.data_len,
                actual: dst.len() as u64,
            });
        }
        read_exact_at(self.buffer.file(), dst, SHM_HEADER_LEN as u64)
            .map_err(|e| self.buffer.read_error(&e))?;
        if self.verify_checksum {
            let expected = self.header.checksum.ok_or(BufferError::ChecksumMissing)?;
            if checksum(dst) != expected {
                return Err(BufferError::ChecksumMismatch);
            }
        }
        Ok(())
    }

    /// データを新しい `Vec` に読む（[`Self::read_into`]）。
    pub fn read_to_vec(&self) -> Result<Vec<u8>, BufferError> {
        let len = usize::try_from(self.header.data_len).map_err(|_| BufferError::DataTooLarge {
            len: self.header.data_len,
            max: usize::MAX as u64,
        })?;
        let mut data = vec![0u8; len];
        self.read_into(&mut data)?;
        Ok(data)
    }
}

/// ワーカーが共有メモリに書き込むためのマップ。
#[derive(Debug)]
pub struct ShmWriter {
    map: MmapMut,
}

impl ShmWriter {
    /// データの長さ `data_len` が `shm` に収まるか確かめる。収まらなければ
    /// [`WorkerErrorKind::ShmTooSmall`]（必要な大きさ付き）。
    pub fn check_capacity(shm: &ShmRef, data_len: u64) -> Result<(), WorkerError> {
        let required = (SHM_HEADER_LEN as u64).saturating_add(data_len);
        if required > shm.capacity {
            return Err(WorkerError::new(
                WorkerErrorKind::ShmTooSmall {
                    required_bytes: required,
                },
                format!(
                    "共有メモリが足りない（必要 {required} バイト、確保されたのは {} バイト）",
                    shm.capacity
                ),
            ));
        }
        Ok(())
    }

    /// 本体が作ったファイルを開いてマップする。ファイルは作らず、大きさも変えない。
    pub fn open(shm: &ShmRef) -> Result<Self, WorkerError> {
        let shm_err = |msg: String| WorkerError::new(WorkerErrorKind::Shm, msg);
        if shm.capacity < SHM_HEADER_LEN as u64 {
            return Err(shm_err(format!(
                "共有メモリの大きさ {} がヘッダより小さい",
                shm.capacity
            )));
        }
        if usize::try_from(shm.capacity).is_err() {
            return Err(shm_err(format!(
                "共有メモリの大きさ {} をマップできない",
                shm.capacity
            )));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&shm.path)
            .map_err(|e| {
                shm_err(format!(
                    "共有メモリを開けない（{}）: {e}",
                    shm.path.display()
                ))
            })?;
        let len = file
            .metadata()
            .map_err(|e| shm_err(format!("共有メモリの大きさを読めない: {e}")))?
            .len();
        if len != shm.capacity {
            return Err(shm_err(format!(
                "共有メモリの大きさ {len} が依頼の {} と違う",
                shm.capacity
            )));
        }
        // SAFETY: 本体が作ったファイルで、大きさを確かめた。本体は応答を受け取るまでこのファイルを
        // 読まず、書き換えもしない。マップへの書き込みは &mut を通してだけ行う。
        let map = unsafe { MmapMut::map_mut(&file) }
            .map_err(|e| shm_err(format!("共有メモリをマップできない: {e}")))?;
        Ok(Self { map })
    }

    /// 大きさ（バイト）。
    pub fn capacity(&self) -> usize {
        self.map.len()
    }

    /// データの領域（ヘッダの直後から `len` バイト）。収まらなければ `None`。
    pub fn data_mut(&mut self, len: usize) -> Option<&mut [u8]> {
        let end = SHM_HEADER_LEN.checked_add(len)?;
        self.map.get_mut(SHM_HEADER_LEN..end)
    }

    /// ヘッダを書いて終える。`with_checksum` ならデータのチェックサムを求めて書く。
    ///
    /// データを書き終えてから呼ぶ（ヘッダは最後に書く）。
    pub fn finish(mut self, mut header: ShmHeader, with_checksum: bool) -> Result<(), WorkerError> {
        let len = usize::try_from(header.data_len).ok();
        let data = len.and_then(|len| self.data_mut(len)).ok_or_else(|| {
            WorkerError::new(
                WorkerErrorKind::Internal,
                "データの長さが共有メモリに収まらない",
            )
        })?;
        header.checksum = with_checksum.then(|| checksum(data));
        self.write_header_bytes(&header.encode());
        Ok(())
    }

    /// ヘッダの領域にそのまま書く（テスト用の口で、不正なヘッダを書くために使う）。
    pub fn write_header_bytes(&mut self, bytes: &[u8; SHM_HEADER_LEN]) {
        self.map[..SHM_HEADER_LEN].copy_from_slice(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> ShmHeader {
        ShmHeader {
            kind: PayloadKind::CfaU16,
            width: 6,
            height: 4,
            data_len: 48,
            checksum: Some(0x1234_5678_9ABC_DEF0),
        }
    }

    #[test]
    fn header_round_trip_and_layout() {
        let h = header();
        let b = h.encode();
        assert_eq!(&b[..8], b"GENZOSHM");
        assert_eq!(get_u32(&b, at::VERSION), 1);
        assert_eq!(get_u32(&b, at::KIND), 1);
        assert_eq!(get_u32(&b, at::WIDTH), 6);
        assert_eq!(get_u32(&b, at::CHANNELS), 1);
        assert_eq!(get_u32(&b, at::BYTES_PER_SAMPLE), 2);
        assert_eq!(get_u64(&b, at::DATA_LEN), 48);
        assert_eq!(get_u32(&b, at::FLAGS), 1);
        assert_eq!(ShmHeader::decode(&b).unwrap(), h);
        let no_sum = ShmHeader {
            checksum: None,
            ..h
        };
        let b = no_sum.encode();
        assert_eq!(get_u32(&b, at::FLAGS), 0);
        assert_eq!(get_u64(&b, at::CHECKSUM), 0);
        assert_eq!(ShmHeader::decode(&b).unwrap(), no_sum);
        for kind in [PayloadKind::CfaU16, PayloadKind::RgbF32, PayloadKind::Jpeg] {
            assert_eq!(PayloadKind::from_code(kind.code()), Some(kind));
            let h = ShmHeader { kind, ..h };
            assert_eq!(ShmHeader::decode(&h.encode()).unwrap(), h);
        }
    }

    #[test]
    fn broken_headers_are_rejected() {
        let good = header().encode();
        assert_eq!(
            ShmHeader::decode(&good[..63]),
            Err(BufferError::HeaderTooShort(63))
        );
        assert_eq!(
            ShmHeader::decode(&[0u8; 64]),
            Err(BufferError::BadMagic),
            "作った直後のファイル（すべて 0）"
        );
        let mut b = good;
        put_u32(&mut b, at::VERSION, 2);
        assert_eq!(
            ShmHeader::decode(&b),
            Err(BufferError::UnsupportedVersion(2))
        );
        let mut b = good;
        put_u32(&mut b, at::KIND, 9);
        assert_eq!(ShmHeader::decode(&b), Err(BufferError::UnknownKind(9)));
        let mut b = good;
        put_u32(&mut b, at::BYTES_PER_SAMPLE, 4);
        assert!(matches!(
            ShmHeader::decode(&b),
            Err(BufferError::InconsistentLayout(_))
        ));
        let mut b = good;
        put_u32(&mut b, at::FLAGS, 0x80);
        assert_eq!(ShmHeader::decode(&b), Err(BufferError::UnknownFlags(0x80)));
        let mut b = good;
        b[60] = 1;
        assert!(matches!(
            ShmHeader::decode(&b),
            Err(BufferError::InconsistentLayout(_))
        ));
        let mut b = good;
        put_u32(&mut b, at::FLAGS, 0);
        assert!(matches!(
            ShmHeader::decode(&b),
            Err(BufferError::InconsistentLayout(_))
        ));
    }

    #[test]
    fn data_len_for_kinds() {
        assert_eq!(PayloadKind::CfaU16.data_len_for(6, 4), Some(48));
        assert_eq!(PayloadKind::RgbF32.data_len_for(6, 4), Some(288));
        assert_eq!(PayloadKind::Jpeg.data_len_for(6, 4), None);
        assert_eq!(
            PayloadKind::RgbF32.data_len_for(65_535, 65_535),
            Some(65_535 * 65_535 * 12)
        );
        // u64 であふれる寸法は None（パニックしない）。
        assert_eq!(PayloadKind::RgbF32.data_len_for(u32::MAX, u32::MAX), None);
    }

    #[test]
    fn checksum_detects_changes() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 % 251) as u8).collect();
        let base = checksum(&data);
        assert_eq!(base, checksum(&data), "決定的");
        let mut sums = std::collections::HashSet::new();
        sums.insert(base);
        for i in [0, 1, 7, 8, 500, 998, 999] {
            for bit in [0, 3, 7] {
                let mut d = data.clone();
                d[i] ^= 1 << bit;
                assert!(sums.insert(checksum(&d)), "{i} {bit}");
            }
        }
        // 長さが違えば（末尾が 0 でも）値が違う。
        let mut longer = data.clone();
        longer.push(0);
        assert_ne!(checksum(&longer), base);
        assert_ne!(checksum(&[]), checksum(&[0]));
    }

    #[test]
    fn arena_allocates_and_reclaims_files() {
        let root = tempfile::tempdir().unwrap();
        let arena = ShmArena::new_in(root.path()).unwrap();
        let dir = arena.path().to_path_buf();
        assert!(
            dir.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(&format!("genzo-shm-{}-", std::process::id()))
        );
        let a = arena.allocate(4096).unwrap();
        let b = arena.allocate(64).unwrap();
        assert_ne!(a.path(), b.path());
        assert_eq!(fs::metadata(a.path()).unwrap().len(), 4096);
        assert_eq!(arena.live_buffers().unwrap(), 2);
        drop(a);
        assert_eq!(arena.live_buffers().unwrap(), 1);
        drop(b);
        assert_eq!(arena.live_buffers().unwrap(), 0);
        assert!(arena.allocate(63).is_err());
        assert_eq!(arena.live_buffers().unwrap(), 0);
        let c = arena.allocate(128).unwrap();
        drop(arena);
        assert!(!dir.exists(), "ディレクトリごと削除される");
        drop(c);
    }

    #[test]
    fn writer_and_reader_share_the_file() {
        let root = tempfile::tempdir().unwrap();
        let arena = ShmArena::new_in(root.path()).unwrap();
        let buf = arena.allocate(SHM_HEADER_LEN as u64 + 16).unwrap();
        let shm = buf.shm_ref(true);
        ShmWriter::check_capacity(&shm, 16).unwrap();
        let err = ShmWriter::check_capacity(&shm, 17).unwrap_err();
        assert_eq!(
            err.kind,
            WorkerErrorKind::ShmTooSmall { required_bytes: 81 }
        );

        // 書く前（すべて 0）は印がない。
        assert_eq!(
            buf.open_payload(PayloadKind::Jpeg, false).unwrap_err(),
            BufferError::BadMagic
        );

        let mut w = ShmWriter::open(&shm).unwrap();
        assert_eq!(w.capacity(), 80);
        assert!(w.data_mut(17).is_none());
        w.data_mut(16)
            .unwrap()
            .copy_from_slice(b"\xFF\xD8\xFF0123456789abc");
        w.finish(
            ShmHeader {
                kind: PayloadKind::Jpeg,
                width: 2,
                height: 1,
                data_len: 16,
                checksum: None,
            },
            true,
        )
        .unwrap();

        let p = buf.open_payload(PayloadKind::Jpeg, true).unwrap();
        assert_eq!(p.read_to_vec().unwrap(), b"\xFF\xD8\xFF0123456789abc");
        assert_eq!((p.header().width, p.header().height), (2, 1));
        // 長さの違う領域には読まない。
        assert_eq!(
            p.read_into(&mut [0u8; 15]).unwrap_err(),
            BufferError::LengthMismatch {
                expected: 16,
                actual: 15
            }
        );
        assert_eq!(
            buf.open_payload(PayloadKind::CfaU16, false).unwrap_err(),
            BufferError::UnexpectedKind {
                expected: PayloadKind::CfaU16,
                actual: PayloadKind::Jpeg
            }
        );

        // データを書き換えるとチェックサムが合わない（データを読むときに確かめる）。
        let mut w = ShmWriter::open(&shm).unwrap();
        w.data_mut(1).unwrap()[0] = 0;
        drop(w);
        let p = buf.open_payload(PayloadKind::Jpeg, true).unwrap();
        assert_eq!(p.read_to_vec().unwrap_err(), BufferError::ChecksumMismatch);
        // 検証しなければ読める。
        buf.open_payload(PayloadKind::Jpeg, false)
            .unwrap()
            .read_to_vec()
            .unwrap();

        // データの長さが大きさを超えるヘッダ。
        let mut w = ShmWriter::open(&shm).unwrap();
        w.write_header_bytes(
            &ShmHeader {
                kind: PayloadKind::Jpeg,
                width: 1,
                height: 1,
                data_len: 17,
                checksum: None,
            }
            .encode(),
        );
        drop(w);
        assert_eq!(
            buf.open_payload(PayloadKind::Jpeg, false).unwrap_err(),
            BufferError::BeyondCapacity {
                needed: 81,
                capacity: 80
            }
        );
        // チェックサムを求めたのにない。
        let mut w = ShmWriter::open(&shm).unwrap();
        w.write_header_bytes(
            &ShmHeader {
                kind: PayloadKind::Jpeg,
                width: 1,
                height: 1,
                data_len: 1,
                checksum: None,
            }
            .encode(),
        );
        drop(w);
        assert_eq!(
            buf.open_payload(PayloadKind::Jpeg, true).unwrap_err(),
            BufferError::ChecksumMissing
        );
    }

    #[test]
    fn writer_rejects_mismatched_files() {
        let root = tempfile::tempdir().unwrap();
        let arena = ShmArena::new_in(root.path()).unwrap();
        let buf = arena.allocate(128).unwrap();
        let mut shm = buf.shm_ref(false);
        shm.capacity = 256;
        assert_eq!(
            ShmWriter::open(&shm).unwrap_err().kind,
            WorkerErrorKind::Shm
        );
        shm.capacity = 32;
        assert_eq!(
            ShmWriter::open(&shm).unwrap_err().kind,
            WorkerErrorKind::Shm
        );
        let missing = ShmRef {
            path: arena.path().join("missing.shm"),
            capacity: 128,
            checksum: false,
        };
        assert_eq!(
            ShmWriter::open(&missing).unwrap_err().kind,
            WorkerErrorKind::Shm
        );
        // ワーカーはファイルを作らない。
        assert!(!missing.path.exists());
    }

    #[test]
    fn truncated_files_are_detected_before_mapping() {
        let root = tempfile::tempdir().unwrap();
        let arena = ShmArena::new_in(root.path()).unwrap();
        let buf = arena.allocate(4096).unwrap();
        OpenOptions::new()
            .write(true)
            .open(buf.path())
            .unwrap()
            .set_len(100)
            .unwrap();
        assert_eq!(
            buf.open_payload(PayloadKind::CfaU16, false).unwrap_err(),
            BufferError::FileSizeChanged {
                expected: 4096,
                actual: 100
            }
        );
    }

    /// 検証の後にファイルが切り詰められても、本体は落ちずにエラーにする（以前はマップを読んで
    /// SIGBUS で本体が落ちた）。
    #[test]
    fn truncation_after_validation_does_not_crash_the_host() {
        let root = tempfile::tempdir().unwrap();
        let arena = ShmArena::new_in(root.path()).unwrap();
        let len: u64 = 1 << 20;
        let buf = arena.allocate(SHM_HEADER_LEN as u64 + len).unwrap();
        let shm = buf.shm_ref(false);
        let mut w = ShmWriter::open(&shm).unwrap();
        w.data_mut(len as usize).unwrap().fill(7);
        w.finish(
            ShmHeader {
                kind: PayloadKind::Jpeg,
                width: 1,
                height: 1,
                data_len: len,
                checksum: None,
            },
            false,
        )
        .unwrap();
        let p = buf.open_payload(PayloadKind::Jpeg, false).unwrap();
        // 乗っ取られたワーカーが、検証の後にファイルを切り詰める。
        OpenOptions::new()
            .write(true)
            .open(buf.path())
            .unwrap()
            .set_len(SHM_HEADER_LEN as u64 + 10)
            .unwrap();
        assert_eq!(
            p.read_to_vec().unwrap_err(),
            BufferError::FileSizeChanged {
                expected: SHM_HEADER_LEN as u64 + len,
                actual: SHM_HEADER_LEN as u64 + 10
            }
        );
        // 読み終えた値は、その後に切り詰められても変わらない。
        OpenOptions::new()
            .write(true)
            .open(buf.path())
            .unwrap()
            .set_len(SHM_HEADER_LEN as u64 + len)
            .unwrap();
        let mut w = ShmWriter::open(&shm).unwrap();
        w.data_mut(len as usize).unwrap().fill(7);
        drop(w);
        let data = p.read_to_vec().unwrap();
        OpenOptions::new()
            .write(true)
            .open(buf.path())
            .unwrap()
            .set_len(0)
            .unwrap();
        assert!(data.iter().all(|&b| b == 7));
        assert_eq!(data.len() as u64, len);
    }

    #[test]
    fn purge_removes_only_other_arenas() {
        let root = tempfile::tempdir().unwrap();
        let keep = ShmArena::new_in(root.path()).unwrap();
        let stale = root.path().join("genzo-shm-99999-abc");
        fs::create_dir(&stale).unwrap();
        fs::write(stale.join("0.shm"), b"x").unwrap();
        let other = root.path().join("other-dir");
        fs::create_dir(&other).unwrap();
        fs::write(root.path().join("genzo-shm-file"), b"x").unwrap();
        assert_eq!(purge_stale_arenas(root.path(), Some(&keep)).unwrap(), 1);
        assert!(!stale.exists());
        assert!(other.exists());
        assert!(keep.path().exists());
        assert!(
            root.path().join("genzo-shm-file").exists(),
            "ファイルは消さない"
        );
    }
}
