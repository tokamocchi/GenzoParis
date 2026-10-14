//! 上書きしない名前の変更（アプリの中でのファイル・フォルダの移動と名前の変更。FILE-02。04 の 6.4 節）。
//!
//! - [`rename_no_replace`] は、同じボリュームの中で名前を変える（移動する）。**移動先に既にあれば置き換えずに失敗する**
//!   （[`io::ErrorKind::AlreadyExists`]）。別のボリュームへは動かさない（OS がコピーに切り替えない。呼び出し側が
//!   コピーと照合を行う）。
//!   - Linux・macOS: `renameat2` の `RENAME_NOREPLACE`（macOS は `renameatx_np` の `RENAME_EXCL`。rustix の
//!     `renameat_with`。tempfile の `persist_noclobber` と同じ）。使えないファイルシステムでは、ファイルはハード
//!     リンクと削除、それもできない（FAT・exFAT など）・フォルダは、移動先がないことを確かめてから通常の名前の
//!     変更をする（確かめてから変えるまでの間の競合は防げない。書き出しと同じ制限）。
//!   - Windows: `MoveFileExW`（`MOVEFILE_REPLACE_EXISTING` を付けない。`MOVEFILE_COPY_ALLOWED` も付けないので、
//!     別のドライブへは失敗する）。MAX_PATH を超えるパスは拡張長パス（`\\?\`）にする（genzo-raw の
//!     `win_long_path` と同じ規則。std の `get_long_path`）。**実機で未確認。**
//! - [`rename_case_safe`] は、大文字・小文字（と Unicode の正規化）だけが違う名前への変更。大文字・小文字を区別
//!   しないボリューム（Windows・macOS の既定）では、移動先の名前が自分自身として「ある」ので、同じフォルダの
//!   一時的な名前（[`case_rename_temp_path`]。新しい名前だけから決まる）を経由して 2 回で変える（どちらも上書き
//!   しない。2 回目に失敗したら元の名前に戻す）。アプリが 2 回の間に終わったときは、呼び出し側の確定の処理
//!   （genzo-api の `fileops::settle`）が一時的な名前を見つけて名前の変更を終わらせる。
//!
//! このモジュールは std と OS の API（rustix・windows-sys）だけを使う（Windows・macOS 向けにも型の確認ができるように）。
//! Windows の `MoveFileExW` の呼び出しは、この crate で `unsafe` を許す唯一の場所（crate の doc の「unsafe」。
//! 決定事項ログ No.172）。

use std::io;
use std::path::{Path, PathBuf};

/// 大文字・小文字だけの名前の変更で経由する一時的な名前の先頭（Unix・macOS では隠しファイル）。
pub const RENAME_TEMP_PREFIX: &str = ".genzo-rename-";

/// `from` の名前を `to` に変える（同じボリュームの中。移動先にあれば置き換えずに [`io::ErrorKind::AlreadyExists`]）。
/// ファイルにもフォルダにも使える。モジュールの doc。
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    imp::rename_no_replace(from, to)
}

/// `from` の名前を `to` に変える。`to` が `from` 自身（大文字・小文字を区別しないボリュームでの、大文字・小文字・
/// 正規化だけが違う名前）なら、同じフォルダの一時的な名前を経由して変える。それ以外は [`rename_no_replace`]。
pub fn rename_case_safe(from: &Path, to: &Path) -> io::Result<()> {
    let same_entry = from != to
        && std::fs::symlink_metadata(to).is_ok()
        && same_file::is_same_file(from, to).unwrap_or(false)
        && from.parent() == to.parent();
    if !same_entry {
        return rename_no_replace(from, to);
    }
    let temp = case_rename_temp_path(to);
    if let Err(e) = rename_no_replace(from, &temp) {
        if e.kind() == io::ErrorKind::AlreadyExists {
            // 前の名前の変更が一時的な名前のまま残っている（上書きしない）。
            return Err(io::Error::other(format!(
                "前の名前の変更の一時的な名前 {} が残っています（中身を確かめて、元の名前に戻すか、要らなければ消してください）",
                temp.display()
            )));
        }
        return Err(e);
    }
    if let Err(e) = rename_no_replace(&temp, to) {
        // 元の名前に戻す（戻せなければ、一時的な名前のまま残る。エラーに一時的な名前を書く）。
        if let Err(back) = rename_no_replace(&temp, from) {
            return Err(io::Error::new(
                e.kind(),
                format!(
                    "{e}（一時的な名前 {} から元の名前に戻せません: {back}）",
                    temp.display()
                ),
            ));
        }
        return Err(e);
    }
    Ok(())
}

/// 大文字・小文字だけの名前の変更（[`rename_case_safe`]）で、`to` に変える間に経由する一時的な名前（`to` と同じ
/// フォルダの `.genzo-rename-<新しい名前のハッシュ>.tmp`）。新しい名前だけから決まるので、アプリが 2 回の名前の
/// 変更の間に終わっても、確定の処理がこの名前を見て、名前の変更を終わらせられる。
///
/// ハッシュは名前のバイト列（`OsStr::as_encoded_bytes`）の FNV-1a（64 ビット）。暗号のハッシュでなくてよい
/// （同じフォルダの中で、行き先の名前ごとに決まればよい。重なれば、残っているものを上書きせずに失敗する）。
pub fn case_rename_temp_path(to: &Path) -> PathBuf {
    let name = to.file_name().map_or(&[][..], |n| n.as_encoded_bytes());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    to.parent()
        .unwrap_or(Path::new("."))
        .join(format!("{RENAME_TEMP_PREFIX}{h:016x}.tmp"))
}

/// 移動先がないことを確かめてから通常の名前の変更をする（上書きしない名前の変更が使えないファイルシステム）。
#[cfg_attr(windows, allow(dead_code))]
fn check_then_rename(from: &Path, to: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("移動先に同じ名前のものがあります: {}", to.display()),
        ));
    }
    std::fs::rename(from, to)
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
))]
mod imp {
    use std::io;
    use std::path::Path;

    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use rustix::io::Errno;

    pub(super) fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
        match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
            Ok(()) => return Ok(()),
            // このファイルシステム・カーネルが RENAME_NOREPLACE に対応しない。
            Err(Errno::INVAL | Errno::NOSYS | Errno::NOTSUP) => {}
            Err(e) => return Err(e.into()),
        }
        let is_dir = std::fs::symlink_metadata(from)?.is_dir();
        if !is_dir {
            // ハードリンクは、移動先にあれば失敗する（上書きしない）。
            match std::fs::hard_link(from, to) {
                Ok(()) => {
                    if let Err(e) = std::fs::remove_file(from) {
                        // 元の名前を消せない: 作ったリンクを消して、何もしなかった状態に戻す。
                        let _ = std::fs::remove_file(to);
                        return Err(e);
                    }
                    return Ok(());
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(e),
                // ハードリンクも使えない（FAT・exFAT など）。
                Err(_) => {}
            }
        }
        super::check_then_rename(from, to)
    }
}

#[cfg(all(
    unix,
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))
))]
mod imp {
    use std::io;
    use std::path::Path;

    pub(super) fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
        super::check_then_rename(from, to)
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    /// これ以上の長さ（UTF-16 の単位。NUL を含む）の絶対パスを、拡張長パスにする（std の `LEGACY_MAX_PATH`）。
    const LEGACY_MAX_PATH: usize = 248;

    /// パスを `MoveFileExW` に渡す NUL で終わる UTF-16 にする（長い絶対パスは `\\?\` を付ける）。
    pub(super) fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let abs = std::path::absolute(path)?;
        let w: Vec<u16> = abs.as_os_str().encode_wide().collect();
        let mut out = match extended(&w) {
            Some(x) => x,
            None => w,
        };
        if out.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "パスに NUL が含まれています",
            ));
        }
        out.push(0);
        Ok(out)
    }

    /// 長い絶対パスを拡張長パスにする（短ければ `None`）。
    fn extended(abs: &[u16]) -> Option<Vec<u16>> {
        const SEP: u16 = b'\\' as u16;
        const QUERY: u16 = b'?' as u16;
        const DOT: u16 = b'.' as u16;
        const COLON: u16 = b':' as u16;
        if abs.len() + 1 < LEGACY_MAX_PATH {
            return None;
        }
        let verbatim: [u16; 4] = [SEP, SEP, QUERY, SEP];
        let unc: [u16; 8] = [
            SEP,
            SEP,
            QUERY,
            SEP,
            u16::from(b'U'),
            u16::from(b'N'),
            u16::from(b'C'),
            SEP,
        ];
        let (prefix, rest): (&[u16], &[u16]) = match abs {
            [SEP, SEP, QUERY, SEP, ..] | [SEP, QUERY, QUERY, SEP, ..] => (&[], abs),
            [_, COLON, SEP, ..] => (&verbatim, abs),
            [SEP, SEP, DOT, SEP, rest @ ..] => (&verbatim, rest),
            [SEP, SEP, rest @ ..] => (&unc, rest),
            _ => (&[], abs),
        };
        let mut out = Vec::with_capacity(prefix.len() + rest.len());
        out.extend_from_slice(prefix);
        out.extend_from_slice(rest);
        Some(out)
    }

    // この crate で `unsafe` を許す唯一の場所（crate の doc の「unsafe」。決定事項ログ No.172）。std の
    // `fs::rename` は移動先を置き換えるので、上書きしない名前の変更には Win32 API を直接呼ぶ。
    #[allow(unsafe_code)]
    pub(super) fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
        let f = wide(from)?;
        let t = wide(to)?;
        // SAFETY: f・t は `wide` が作った NUL で終わる UTF-16 の文字列（途中に NUL はない）で、呼び出しの間は
        // 生きている。関数はどちらも読むだけで、ポインタを保持しない。フラグは 0（上書きしない・別のドライブへ
        // コピーしない）。
        let ok = unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), 0) };
        if ok == 0 {
            // ERROR_FILE_EXISTS（80）・ERROR_ALREADY_EXISTS（183）は AlreadyExists になる（std の対応表）。
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_and_folders_are_renamed_without_replacing() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.jpg");
        let b = d.path().join("b.jpg");
        std::fs::write(&a, b"A").unwrap();
        std::fs::write(&b, b"B").unwrap();
        let e = rename_no_replace(&a, &b).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&b).unwrap(), b"B", "上書きしない");
        assert_eq!(std::fs::read(&a).unwrap(), b"A");
        let c = d.path().join("c.jpg");
        rename_no_replace(&a, &c).unwrap();
        assert!(!a.exists());
        assert_eq!(std::fs::read(&c).unwrap(), b"A");

        // フォルダ（中身ごと）。
        let src = d.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("x"), b"x").unwrap();
        let taken = d.path().join("taken");
        std::fs::create_dir(&taken).unwrap();
        assert_eq!(
            rename_no_replace(&src, &taken).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        let dst = d.path().join("dst");
        rename_no_replace(&src, &dst).unwrap();
        assert_eq!(std::fs::read(dst.join("x")).unwrap(), b"x");
        // 移動先のフォルダがなければ失敗（作らない）。
        assert!(rename_no_replace(&c, &d.path().join("ない").join("c.jpg")).is_err());
        assert!(c.exists());
    }

    /// 大文字・小文字だけの名前の変更（区別するファイルシステムでは、そのまま変わる。区別しないものでは一時的な
    /// 名前を経由する）。別のファイルがある名前へは変えない。
    #[test]
    fn case_only_renames_work_and_other_files_are_kept() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("dsc1.arw");
        std::fs::write(&a, b"A").unwrap();
        let upper = d.path().join("DSC1.ARW");
        rename_case_safe(&a, &upper).unwrap();
        let names: Vec<String> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["DSC1.ARW".to_owned()], "一時的な名前は残らない");
        let other = d.path().join("other.arw");
        std::fs::write(&other, b"O").unwrap();
        assert_eq!(
            rename_case_safe(&upper, &other).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&other).unwrap(), b"O");
    }

    /// 一時的な名前は、新しい名前だけから決まり（同じ名前なら同じ）、新しい名前と同じフォルダの隠しファイルの
    /// 名前になる。
    #[test]
    fn case_rename_temp_names_depend_only_on_the_new_name() {
        let d = Path::new("/写真/2024");
        let t = case_rename_temp_path(&d.join("dsc1.arw"));
        assert_eq!(t, case_rename_temp_path(&d.join("dsc1.arw")));
        assert_ne!(t, case_rename_temp_path(&d.join("DSC1.ARW")));
        assert_eq!(t.parent(), Some(d));
        let name = t.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with(RENAME_TEMP_PREFIX) && name.ends_with(".tmp"),
            "{name}"
        );
        assert_eq!(name.len(), RENAME_TEMP_PREFIX.len() + 16 + 4);
        // FNV-1a（64 ビット）の空の入力の値（オフセットの基底）。
        assert_eq!(
            case_rename_temp_path(Path::new("")),
            Path::new(".").join(format!("{RENAME_TEMP_PREFIX}cbf29ce484222325.tmp"))
        );
    }
}
