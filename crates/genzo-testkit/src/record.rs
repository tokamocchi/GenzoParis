//! 記録の共通部分: 入力の ID とハッシュ（05 の 1.8 節「入力」）、名前の検査、ファイルの安全な書き込み。

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use genzo_model::DevelopSettings;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::image::FloatImage;

/// 名前（基準画像・計測の名前。ファイル名に使う）の最大の長さ。
pub const MAX_NAME_LEN: usize = 128;

/// バイト列の SHA-256（小文字の 16 進数 64 文字）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    to_hex(&Sha256::digest(bytes))
}

/// ファイルの SHA-256（小文字の 16 進数 64 文字）。大きなファイルも少しずつ読んで計算する。
pub fn sha256_file_hex(path: impl AsRef<Path>) -> io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    s
}

/// 入力の ID とハッシュ（05 の 1.8 節「入力: サンプルの ID とファイルのハッシュ」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputRef {
    /// サンプルの ID（例: `"a7iv-iso100-landscape-001"`、合成なら `"synthetic:zone_plate:256x256"`）。
    pub id: String,
    /// 内容の SHA-256（小文字の 16 進数）。
    pub sha256: String,
}

impl InputRef {
    /// ID とハッシュから作る。
    pub fn new(id: impl Into<String>, sha256: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            sha256: sha256.into(),
        }
    }

    /// バイト列（RAW のファイルの内容など）から作る。
    pub fn from_bytes(id: impl Into<String>, bytes: &[u8]) -> Self {
        Self::new(id, sha256_hex(bytes))
    }

    /// ファイルから作る。
    pub fn from_file(id: impl Into<String>, path: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self::new(id, sha256_file_hex(path)?))
    }

    /// 合成の画像から作る（ハッシュは [`crate::golden::encode_float_image`] の形式のバイト列）。
    ///
    /// 注意: 三角関数・累乗などを使って作った画像は、OS の数学ライブラリによって最下位のビットが
    /// 違いうるので、ハッシュも OS ごとに変わりうる。複数の OS で共有する基準画像（golden）の入力には、
    /// 生成のパラメータを文字列にして [`from_bytes`](Self::from_bytes) で記録する。
    pub fn from_float_image(id: impl Into<String>, image: &FloatImage) -> Self {
        Self::from_bytes(id, &crate::golden::encode_float_image(image))
    }
}

/// 使った現像設定（05 の 1.8 節「入力: 使った現像設定（JSON）」）。
///
/// 設定は、丸めた設定の正規化した JSON（[`DevelopSettings::canonical_json`]）を値として保存する。
/// 型（[`DevelopSettings`]）のまま保存しないのは、スキーマが変わっても記録を読めるようにするため
/// （読み戻すときは [`DevelopSettings::from_json`] でマイグレーションする）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsRecord {
    /// 丸めた設定の正規化した JSON。
    pub settings: serde_json::Value,
    /// 設定全体のハッシュ（[`DevelopSettings::develop_hash_hex`]）。
    pub develop_hash: String,
    /// 処理バージョン（04 の 2.5 節）。
    pub process_version: u32,
}

impl SettingsRecord {
    /// 現像設定から作る。
    pub fn from_settings(settings: &DevelopSettings) -> Self {
        let json = settings.canonical_json();
        Self {
            settings: serde_json::from_str(&json)
                .expect("canonical_json は serde_json で作った JSON なので必ず解析できる"),
            develop_hash: settings.develop_hash_hex(),
            process_version: settings.process_version,
        }
    }

    /// 現像設定に読み戻す（スキーマのマイグレーションを含む）。
    pub fn to_settings(&self) -> Result<DevelopSettings, genzo_model::DevelopError> {
        DevelopSettings::from_json(&self.settings.to_string())
    }
}

/// 名前の検査のエラー。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "名前 {0:?} は使えません（1〜{MAX_NAME_LEN} 文字の英数字・`.`・`_`・`-` で、`.` で始まらないこと）"
)]
pub struct InvalidName(pub String);

/// Windows のデバイス名（大文字・小文字を区別しない）。拡張子を付けても（例: `CON.json`）
/// 通常のファイルとして扱われない場合があるため、名前の最初の `.` より前がこれらに一致する名前は
/// 使わない（対象 OS の Windows 11 で基準画像・計測の結果を書けなくなるのを防ぐ）。
const WINDOWS_RESERVED_STEMS: [&str; 4] = ["con", "prn", "aux", "nul"];

/// 名前の最初の `.` より前の部分が Windows のデバイス名（`CON`・`PRN`・`AUX`・`NUL`・`COM0`〜`COM9`・
/// `LPT0`〜`LPT9`）か。
fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_lowercase();
    if WINDOWS_RESERVED_STEMS.contains(&stem.as_str()) {
        return true;
    }
    let b = stem.as_bytes();
    b.len() == 4 && (stem.starts_with("com") || stem.starts_with("lpt")) && b[3].is_ascii_digit()
}

/// 名前がファイル名として安全か確かめる（英数字・`.`・`_`・`-` だけ、`.` で始まらない、
/// Windows のデバイス名ではない）。
///
/// 名前はそのままファイル名（`<名前>.json` など）に使うため、パスの区切りや `..` を含む名前で
/// 別の場所に書き込まないようにする。
///
/// 注意: 大文字と小文字は区別する（`Sample` と `sample` は別の名前）が、Windows と macOS の
/// 通常のファイルシステムは区別しないため、同じディレクトリで大文字・小文字だけが違う名前を
/// 使わないこと（同じファイルを上書きし合う）。
pub fn validate_name(name: &str) -> Result<(), InvalidName> {
    let ok = !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && !is_windows_reserved(name);
    if ok {
        Ok(())
    } else {
        Err(InvalidName(name.to_owned()))
    }
}

/// [`write_atomic`] の一時ファイルの名前に付ける、プロセスの中で一意な番号。
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// ファイルを置き換える形で書く（同じディレクトリの一時ファイルに書いてから名前を変える）。
///
/// 書き込みの途中で失敗・中断しても、元のファイルが壊れた状態で残らないようにする
/// （04 の 6.4 節の考え方）。ディレクトリがなければ作る。
///
/// 一時ファイルの名前は、プロセスの番号と、プロセスの中で一意な番号から作る。テストは同じ
/// プロセスの複数のスレッドで並列に動くため、プロセスの番号だけでは、別のスレッドが同じパスに
/// 書くときに一時ファイルが衝突する（内容が混ざる、名前の変更が失敗する）。
/// 同じパスに同時に書いた場合は、最後に名前を変えたものが残る（読み込み → 変更 → 書き込みの
/// 競合は、呼び出し側で防ぐ。[`crate::bench::BenchRecorder::record`] を参照）。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(dir) = dir {
        fs::create_dir_all(dir)?;
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ファイル名がありません"))?;
    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp: PathBuf = match dir {
        Some(d) => d.join(tmp_name),
        None => PathBuf::from(tmp_name),
    };
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        // 一時ファイルの後始末。失敗しても元のエラーを返す。
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        // FIPS 180-2 の例（"abc"）と空の入力。
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_file_matches_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.bin");
        let data: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        fs::write(&p, &data).unwrap();
        assert_eq!(sha256_file_hex(&p).unwrap(), sha256_hex(&data));
        let r = InputRef::from_file("x", &p).unwrap();
        assert_eq!(r.sha256, sha256_hex(&data));
        assert!(InputRef::from_file("missing", dir.path().join("none")).is_err());
    }

    #[test]
    fn names() {
        for ok in ["a", "zone_plate-256x256", "PoC-3.exposure_drag", "A1"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        let long = "a".repeat(MAX_NAME_LEN + 1);
        for bad in [
            "",
            ".hidden",
            "..",
            "a/b",
            "a\\b",
            "a b",
            "日本語",
            "x:y",
            long.as_str(),
        ] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn settings_record_roundtrip() {
        let s = DevelopSettings {
            exposure_ev: 0.5,
            ..Default::default()
        };
        let r = SettingsRecord::from_settings(&s);
        assert_eq!(r.develop_hash, s.develop_hash_hex());
        assert_eq!(r.process_version, s.process_version);
        let back = r.to_settings().unwrap();
        assert_eq!(back.develop_hash_hex(), s.develop_hash_hex());
        let json = serde_json::to_string(&r).unwrap();
        let r2: SettingsRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(r2, r);
    }

    #[test]
    fn write_atomic_replaces_and_creates_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/dir/file.json");
        write_atomic(&p, b"one").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"one");
        write_atomic(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        // 一時ファイルが残っていない。
        let names: Vec<_> = fs::read_dir(p.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[test]
    fn write_atomic_from_threads_never_mixes_or_fails() {
        // 同じプロセスの複数のスレッドが同じパスに書く（テストは並列に動く）。一時ファイルの名前が
        // プロセスの番号だけだと、別のスレッドの一時ファイルを切り詰めたり、先に名前を変えられて
        // rename が失敗したりする。
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("shared.bin");
        let contents: Vec<Vec<u8>> = (0..8u8).map(|i| vec![i; 64 * 1024]).collect();
        std::thread::scope(|s| {
            for c in &contents {
                let p = &p;
                s.spawn(move || {
                    for _ in 0..20 {
                        write_atomic(p, c).unwrap();
                    }
                });
            }
        });
        // 最後の内容は、どれか 1 つのスレッドの内容そのもの（混ざっていない）。
        let last = fs::read(&p).unwrap();
        assert!(contents.contains(&last), "内容が混ざっています");
        // 一時ファイルが残っていない。
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[test]
    fn windows_reserved_names_are_rejected() {
        // Windows（対象 OS）ではデバイス名（拡張子付きも含む）をファイル名に使えない。
        for bad in [
            "con", "CON", "nul", "Aux", "prn", "com1", "LPT9", "nul.x", "com0",
        ] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
        for ok in ["console", "null_case", "com10", "lpt", "aux-1", "con_1"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
    }
}
