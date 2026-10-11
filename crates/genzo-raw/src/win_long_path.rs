//! Windows の長いパス（MAX_PATH を超えるパス）を、LibRaw（C の実行時ライブラリ）に渡せる形にする。
//!
//! LibRaw は Windows では、受け取ったワイド文字のパスをそのまま `_wstati64`・`_wfopen`・
//! `std::filebuf::open`・`CreateFileW` に渡す（LibRaw 0.21 の `src/utils/open.cpp`・
//! `src/libraw_datastream.cpp`）。これらの API は、パスが `\\?\` で始まらなければ、MAX_PATH
//! （NUL を含めて 260）を超えるパスを開けない（実行ファイルに `longPathAware` のマニフェストがあり、
//! かつレジストリの `LongPathsEnabled` が 1 の場合を除く。どちらも既定では満たさない）。
//! Rust の std は長いパスに自動で `\\?\` を付けるため、std での事前の確認は通るのに LibRaw では
//! 開けず、正常な RAW が「壊れている可能性」と報告されていた（指摘 F20）。
//! そこで、長い絶対パスを std（`library/std/src/sys/path/windows.rs` の `get_long_path`）と
//! 同じ規則で拡張長パスに変えてから LibRaw に渡す。
//!
//! 文字列の変換だけなので、どの OS でもテストする（使うのは Windows で `libraw` が有効なときだけ）。
//! Windows の実機での動作（LibRaw の中の C の実行時ライブラリが `\\?\` のパスを受け付けるか）は
//! 未確認（PoC-2）。受け付けない場合に備え、呼び出し側（`libraw::Processor::open_path_native`）は
//! 長いパスで開けなければ Rust で読み込んで渡す。

/// これ以上の長さ（UTF-16 の単位。NUL を含む）の絶対パスを、拡張長パスにする。
///
/// std の `LEGACY_MAX_PATH` と同じ値（ディレクトリの作成の上限 248 に合わせた控えめな値。
/// ファイルの上限は MAX_PATH の 260）。これより短いパスは、接頭辞なしでも開ける。
const LEGACY_MAX_PATH: usize = 248;

const SEP: u16 = b'\\' as u16;
const QUERY: u16 = b'?' as u16;
const DOT: u16 = b'.' as u16;
const COLON: u16 = b':' as u16;

/// `\\?\`
const VERBATIM_PREFIX: [u16; 4] = [SEP, SEP, QUERY, SEP];
/// `\\?\UNC\`
const UNC_PREFIX: [u16; 8] = [
    SEP,
    SEP,
    QUERY,
    SEP,
    b'U' as u16,
    b'N' as u16,
    b'C' as u16,
    SEP,
];

/// 絶対パス（`std::path::absolute` の結果を UTF-16 にしたもの。NUL を含まない）が長ければ、
/// 拡張長パスにして返す。短ければ `None`（接頭辞なしで開ける。元のパスをそのまま使う）。
///
/// - `C:\…` → `\\?\C:\…`
/// - `\\server\share\…`（UNC） → `\\?\UNC\server\share\…`
/// - `\\.\…`（デバイスのパス） → `\\?\…`
/// - 既に `\\?\`・`\??\` で始まるもの、それ以外の形 → そのまま（長ければ `Some`）
///
/// `\\?\` の付いたパスは Windows が正規化しない（`/` を区切りとみなさず、`.`・`..` を解決しない）
/// ため、`absolute` は `std::path::absolute` が正規化した形（`GetFullPathNameW` の結果）を前提にする。
pub(crate) fn extended_length_path(absolute: &[u16]) -> Option<Vec<u16>> {
    if absolute.len() + 1 < LEGACY_MAX_PATH {
        return None;
    }
    let (prefix, rest): (&[u16], &[u16]) = match absolute {
        // 既に拡張長パス（`\\?\`）・NT のパス（`\??\`）: そのまま。
        [SEP, SEP, QUERY, SEP, ..] | [SEP, QUERY, QUERY, SEP, ..] => (&[], absolute),
        // `C:\` → `\\?\C:\`
        [_, COLON, SEP, ..] => (&VERBATIM_PREFIX, absolute),
        // `\\.\` → `\\?\`
        [SEP, SEP, DOT, SEP, rest @ ..] => (&VERBATIM_PREFIX, rest),
        // `\\server\share` → `\\?\UNC\server\share`
        [SEP, SEP, rest @ ..] => (&UNC_PREFIX, rest),
        // それ以外（正規化した絶対パスでは起きないはず）: そのまま。
        _ => (&[], absolute),
    };
    let mut out = Vec::with_capacity(prefix.len() + rest.len());
    out.extend_from_slice(prefix);
    out.extend_from_slice(rest);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// `head` の後に、全体が `len` 単位になるまで `a` を足したパス。
    fn padded(head: &str, len: usize) -> String {
        let n = len - head.encode_utf16().count();
        format!("{head}{}", "a".repeat(n))
    }

    #[test]
    fn short_paths_are_left_alone() {
        for p in [
            r"C:\Photos\2024\IMG_0001.ARW",
            r"\\nas\photos\2024\IMG_0001.ARW",
            r"\\?\C:\Photos\IMG_0001.ARW",
        ] {
            assert_eq!(extended_length_path(&w(p)), None, "{p}");
        }
    }

    #[test]
    fn threshold_matches_std() {
        // std と同じく、NUL を含めて 248 単位（NUL を除いて 247）以上で付ける。
        let below = padded(r"C:\Photos\", 246);
        assert_eq!(extended_length_path(&w(&below)), None);
        let at = padded(r"C:\Photos\", 247);
        let expected = format!(r"\\?\{at}");
        assert_eq!(extended_length_path(&w(&at)), Some(w(&expected)));
    }

    #[test]
    fn long_drive_path_gets_verbatim_prefix() {
        let p = padded(r"C:\Users\写真\深い\", 320) + ".dng";
        let expected = format!(r"\\?\{p}");
        assert_eq!(extended_length_path(&w(&p)), Some(w(&expected)));
    }

    #[test]
    fn long_unc_path_gets_unc_prefix() {
        let p = padded(r"\\nas\photos\2024\", 300);
        let expected = format!(r"\\?\UNC\{}", &p[2..]);
        assert_eq!(extended_length_path(&w(&p)), Some(w(&expected)));
    }

    #[test]
    fn long_device_path_becomes_verbatim() {
        let p = padded(r"\\.\C:\Photos\", 300);
        let expected = format!(r"\\?\{}", &p[4..]);
        assert_eq!(extended_length_path(&w(&p)), Some(w(&expected)));
    }

    #[test]
    fn long_verbatim_and_nt_paths_are_kept() {
        for head in [r"\\?\C:\Photos\", r"\\?\UNC\nas\photos\", r"\??\C:\Photos\"] {
            let p = padded(head, 300);
            assert_eq!(extended_length_path(&w(&p)), Some(w(&p)), "{head}");
        }
    }

    #[test]
    fn surrogate_pairs_are_preserved() {
        // サロゲートペア（BMP の外の文字）や、UTF-16 として不正な単独のサロゲートもそのまま残す
        // （Windows のパスは UTF-16 として正しいとは限らない）。
        let mut p = w(r"C:\Photos\🎞️\");
        p.push(0xD800);
        p.resize(300, u16::from(b'a'));
        let mut expected = VERBATIM_PREFIX.to_vec();
        expected.extend_from_slice(&p);
        assert_eq!(extended_length_path(&p), Some(expected));
    }
}
