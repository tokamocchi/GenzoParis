//! ファイルのクイックハッシュと全体のハッシュ（docs/04_architecture.md の 3.3 節。レビュー R-09）。
//!
//! - **クイックハッシュ**: ファイルサイズ ＋ 先頭と末尾の 64KB の BLAKE3。変化の検知と、
//!   再リンクの候補の絞り込みだけに使う（ファイルの中間だけが変わった場合は見分けられない）。
//! - **全体のハッシュ**: ファイル全体の BLAKE3（ストリーミング）。同一性を確定する場面で使い、
//!   `file.full_hash` に保存する。値は BLAKE3 の標準の出力（`b3sum` と同じ）。
//!
//! どちらもファイルを読み取り専用で開く（DATA-01。`std::fs::File::open` は読み取り専用）。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{CatalogError, Result};

/// クイックハッシュで読む先頭・末尾の長さ（64KB。3.3 節）。
pub const QUICK_HASH_CHUNK: u64 = 64 * 1024;

/// クイックハッシュの入力の先頭に付ける、用途と形式の版を示す文字列。
/// 計算の方法を変えたら末尾の番号を上げる（保存済みの値と比べられなくなるため）。
const QUICK_HASH_DOMAIN: &[u8] = b"genzo.quick_hash.v1\0";

/// 全体のハッシュを計算するときに一度に読む長さ。
const FULL_HASH_BUFFER: usize = 1024 * 1024;

/// ファイルの変化を検知するための情報（`file.size`・`file.mtime`・`file.quick_hash`）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileFacts {
    /// ファイルサイズ（バイト）。
    pub size: u64,
    /// 更新日時（UNIX 時刻からのナノ秒。取得できない場合は 0）。
    pub mtime_ns: i64,
    /// クイックハッシュ（BLAKE3 の 16 進数の小文字 64 文字）。
    pub quick_hash: String,
}

impl FileFacts {
    /// ファイルを読み取り専用で開いて、サイズ・更新日時・クイックハッシュを求める。
    pub fn read(path: &Path) -> Result<Self> {
        let io_err = |e| CatalogError::io(path, e);
        let mut file = File::open(path).map_err(io_err)?;
        let meta = file.metadata().map_err(io_err)?;
        if !meta.is_file() {
            return Err(CatalogError::InvalidInput(format!(
                "通常のファイルではありません: {}",
                path.display()
            )));
        }
        let mtime_ns = meta.modified().map(system_time_to_ns).unwrap_or(0);
        let size = meta.len();
        let quick_hash = quick_hash_reader(&mut file, size).map_err(io_err)?;
        Ok(Self {
            size,
            mtime_ns,
            quick_hash,
        })
    }

    /// サイズ・更新日時・クイックハッシュのいずれかが違うか（3.3 節の変化の検知）。
    pub fn differs_from(&self, other: &FileFacts) -> bool {
        self != other
    }
}

/// `SystemTime` を UNIX 時刻からのナノ秒にする（i64 に収まらない場合は飽和させる）。
pub(crate) fn system_time_to_ns(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => i64::try_from(e.duration().as_nanos())
            .map(|n| -n)
            .unwrap_or(i64::MIN),
    }
}

/// ファイルのクイックハッシュ（サイズ ＋ 先頭と末尾の 64KB の BLAKE3）を求める。
pub fn quick_hash(path: &Path) -> Result<String> {
    let io_err = |e| CatalogError::io(path, e);
    let mut file = File::open(path).map_err(io_err)?;
    let size = file.metadata().map_err(io_err)?.len();
    quick_hash_reader(&mut file, size).map_err(io_err)
}

/// 読み取り位置を移動できる入力のクイックハッシュを求める。
///
/// 入力は「用途の文字列 ‖ サイズ（u64、リトルエンディアン）‖ 先頭 min(64KB, サイズ) バイト ‖
/// 末尾の 64KB のうち先頭と重ならない部分」。128KB 以下のファイルは全体を 1 回ずつ読む。
pub fn quick_hash_reader<R: Read + Seek>(reader: &mut R, size: u64) -> std::io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(QUICK_HASH_DOMAIN);
    hasher.update(&size.to_le_bytes());

    let head_len = size.min(QUICK_HASH_CHUNK);
    reader.seek(SeekFrom::Start(0))?;
    hash_exact(reader, head_len, &mut hasher)?;

    let tail_start = size.saturating_sub(QUICK_HASH_CHUNK).max(head_len);
    if tail_start < size {
        reader.seek(SeekFrom::Start(tail_start))?;
        hash_exact(reader, size - tail_start, &mut hasher)?;
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// `len` バイトを読んでハッシュに加える。途中でファイルが短くなっていたらエラーにする。
fn hash_exact<R: Read>(
    reader: &mut R,
    len: u64,
    hasher: &mut blake3::Hasher,
) -> std::io::Result<()> {
    let mut limited = reader.take(len);
    let copied = std::io::copy(&mut limited, hasher)?;
    if copied != len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "ハッシュの計算中にファイルが短くなりました",
        ));
    }
    Ok(())
}

/// ファイル全体の BLAKE3（16 進数の小文字 64 文字）を、少しずつ読みながら求める。
pub fn full_hash(path: &Path) -> Result<String> {
    let io_err = |e| CatalogError::io(path, e);
    let file = File::open(path).map_err(io_err)?;
    full_hash_reader(file).map_err(io_err)
}

/// 入力全体の BLAKE3（16 進数の小文字 64 文字）を求める。
pub fn full_hash_reader<R: Read>(mut reader: R) -> std::io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; FULL_HASH_BUFFER];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// 小文字の 16 進数 64 文字（BLAKE3・SHA-256 の 16 進数表記）か。
pub(crate) fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::*;

    /// BLAKE3 の公表されている値: 空の入力のハッシュ
    /// （BLAKE3 の仕様書・公式の test_vectors.json の input_len = 0）。
    const BLAKE3_EMPTY: &str = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";

    fn data(len: usize) -> Vec<u8> {
        // 公式のテストベクタと同じ入力の作り方（0, 1, ..., 250 の繰り返し）。
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn full_hash_matches_published_empty_vector() {
        assert_eq!(
            full_hash_reader(Cursor::new(Vec::new())).unwrap(),
            BLAKE3_EMPTY
        );
    }

    #[test]
    fn full_hash_streaming_equals_one_shot() {
        for len in [
            1,
            1023,
            1024,
            1025,
            FULL_HASH_BUFFER + 7,
            3 * FULL_HASH_BUFFER,
        ] {
            let d = data(len);
            assert_eq!(
                full_hash_reader(Cursor::new(&d)).unwrap(),
                blake3::hash(&d).to_hex().to_string(),
                "len = {len}"
            );
        }
    }

    #[test]
    fn quick_hash_reads_whole_small_files_once() {
        // 128KB 以下は全体を 1 回ずつ読む: 期待値を直接組み立てて比べる。
        for len in [0usize, 1, 65_536, 65_537, 131_072] {
            let d = data(len);
            let mut h = blake3::Hasher::new();
            h.update(QUICK_HASH_DOMAIN);
            h.update(&(len as u64).to_le_bytes());
            h.update(&d);
            let expected = h.finalize().to_hex().to_string();
            let got = quick_hash_reader(&mut Cursor::new(&d), len as u64).unwrap();
            assert_eq!(got, expected, "len = {len}");
        }
    }

    #[test]
    fn quick_hash_uses_head_and_tail_of_large_files() {
        let len = 1_000_000usize;
        let d = data(len);
        let mut h = blake3::Hasher::new();
        h.update(QUICK_HASH_DOMAIN);
        h.update(&(len as u64).to_le_bytes());
        h.update(&d[..65_536]);
        h.update(&d[len - 65_536..]);
        let expected = h.finalize().to_hex().to_string();
        assert_eq!(
            quick_hash_reader(&mut Cursor::new(&d), len as u64).unwrap(),
            expected
        );

        // 中間だけを変えてもクイックハッシュは変わらない（だから候補の絞り込みにだけ使う）。
        let mut middle_changed = d.clone();
        middle_changed[len / 2] ^= 0xff;
        assert_eq!(
            quick_hash_reader(&mut Cursor::new(&middle_changed), len as u64).unwrap(),
            expected
        );
        // 全体のハッシュは変わる。
        assert_ne!(
            full_hash_reader(Cursor::new(&middle_changed)).unwrap(),
            full_hash_reader(Cursor::new(&d)).unwrap()
        );
        // 末尾を変えると変わる。
        let mut tail_changed = d.clone();
        tail_changed[len - 1] ^= 0xff;
        assert_ne!(
            quick_hash_reader(&mut Cursor::new(&tail_changed), len as u64).unwrap(),
            expected
        );
    }

    #[test]
    fn quick_hash_depends_on_size() {
        // 同じ内容でもサイズが違えば値が違う（サイズを入力に含める）。
        let d = data(10);
        let a = quick_hash_reader(&mut Cursor::new(&d), 10).unwrap();
        let mut longer = d.clone();
        longer.push(0);
        let b = quick_hash_reader(&mut Cursor::new(&longer), 11).unwrap();
        assert_ne!(a, b);
        assert!(is_hex64(&a));
    }

    #[test]
    fn truncated_input_is_an_error() {
        let d = data(100);
        // 実際より大きいサイズを渡すと、読み切れずにエラーになる。
        assert!(quick_hash_reader(&mut Cursor::new(&d), 200).is_err());
    }

    #[test]
    fn file_facts_read_from_disk_and_leave_file_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        let d = data(200_000);
        std::fs::File::create(&path).unwrap().write_all(&d).unwrap();
        let before = full_hash(&path).unwrap();

        let facts = FileFacts::read(&path).unwrap();
        assert_eq!(facts.size, 200_000);
        assert!(facts.mtime_ns > 0);
        assert_eq!(facts.quick_hash, quick_hash(&path).unwrap());
        assert!(!facts.differs_from(&facts.clone()));

        // DATA-01: 読み取りの前後で内容が変わらない。
        assert_eq!(full_hash(&path).unwrap(), before);
        assert_eq!(before, blake3::hash(&d).to_hex().to_string());

        // ディレクトリや存在しないファイルはエラー。
        assert!(FileFacts::read(dir.path()).is_err());
        assert!(matches!(
            FileFacts::read(&dir.path().join("none")),
            Err(CatalogError::Io { .. })
        ));
    }

    #[test]
    fn system_time_conversion_handles_before_epoch() {
        assert_eq!(system_time_to_ns(UNIX_EPOCH), 0);
        let before = UNIX_EPOCH - std::time::Duration::from_secs(1);
        assert_eq!(system_time_to_ns(before), -1_000_000_000);
    }

    #[test]
    fn hex64_check() {
        assert!(is_hex64(BLAKE3_EMPTY));
        assert!(!is_hex64(&BLAKE3_EMPTY.to_uppercase()));
        assert!(!is_hex64("abc"));
    }
}
