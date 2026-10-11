//! 安全な書き出し（04 の 6.4 節「書き出しの安全性」。レビュー R-06）。
//!
//! 1. **書き出し先の決定**（[`resolve_destination`]）: [`ConflictPolicy`] に従う。
//!    - 連番を付ける（既定）: `name.jpg` が既にあれば `name-1.jpg`、`name-2.jpg` … の空いている名前。
//!    - 上書き: そのまま（既存のファイルを置き換える）。
//!    - スキップ: 既にあれば書き出さない。
//! 2. **原本の保護**（[`ProtectedFiles`]）: 書き出し先が、カタログに登録されたファイル・今回の
//!    入力と同じファイルなら中止する。パスの文字列（正規化したもの）の比較に加え、既存の通常の
//!    ファイルなら same-file の `Handle`（Unix はデバイスと i-node、Windows はボリュームとファイル
//!    ID）で同一性を確かめる（シンボリックリンク・ハードリンクを経由した同じファイルを見分けるため）。
//!    **上書きの設定でも必ず確かめる**。名前付きパイプ（FIFO）などは開かない（開くと止まるため）。
//!    上書きの設定でも、通常のファイル以外（フォルダ・FIFO など）は置き換えない。
//! 3. **原子的な書き出し**（[`write_atomically`]）: 書き出し先と同じフォルダの一時ファイルに書き、
//!    flush と sync をしてから名前を変更して公開する。上書きしない設定では、名前の変更でも既存の
//!    ファイルを置き換えない（`persist_noclobber`）。途中で失敗した（またはパニックした）ときは、
//!    一時ファイルを削除する（不完全なファイルを残さない）。
//!
//! 制限: 照合から名前の変更までの間に、別のプロセスが書き出し先を作り替えることは防げない
//! （照合と変更を 1 つの操作にできないため）。プロセスが強制終了されたときは、一時ファイル
//! （[`TEMP_PREFIX`] で始まる名前）が残ることがある。

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use genzo_model::ConflictPolicy;
use same_file::Handle;

use crate::error::{MediaError, Result};

/// 連番の上限（仮置き）。`name-9999.jpg` まで試して空きがなければエラーにする。
pub const MAX_SEQUENCE_NUMBER: u32 = 9_999;

/// 一時ファイルの名前の先頭（Unix・macOS では、ドットで始まるので隠しファイルになる）。
pub const TEMP_PREFIX: &str = ".genzo-export-";

/// 一時ファイルの名前の末尾。
pub const TEMP_SUFFIX: &str = ".tmp";

/// `path` に連番 `n` を付けたパス（`dir/name.jpg` → `dir/name-n.jpg`。拡張子がなければ末尾に付ける）。
pub fn sequence_path(path: &Path, n: u32) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default();
    let mut name = OsString::from(stem);
    name.push(format!("-{n}"));
    if let Some(ext) = path.extension() {
        name.push(".");
        name.push(ext);
    }
    path.with_file_name(name)
}

/// パスに何か（ファイル・フォルダ・壊れたシンボリックリンクを含む）があるか。
fn exists_no_follow(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(MediaError::io(path, e)),
    }
}

/// 書き出し先の決定の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// このパスに書き出す。
    Write {
        /// 書き出し先。
        path: PathBuf,
        /// 既存のファイルを置き換えるか（上書きの設定で、既にファイルがある場合）。
        replace_existing: bool,
    },
    /// 既にファイルがあるので書き出さない（スキップの設定）。
    Skip {
        /// 既にあるファイル。
        existing: PathBuf,
    },
}

fn check_file_name(desired: &Path) -> Result<()> {
    match desired.file_name() {
        Some(n) if !n.is_empty() => Ok(()),
        _ => Err(MediaError::invalid_argument(format!(
            "書き出し先にファイル名がない（{}）",
            desired.display()
        ))),
    }
}

/// 衝突の扱いに従って、書き出し先を決める（ファイルは作らない）。
///
/// 連番を付けるときは、`desired` → `-1` → `-2` … の順に、何もない名前を探す。
/// 上書きの設定で、`desired` が通常のファイルでない（フォルダ・名前付きパイプなど。リンク先を
/// 含む）ときは [`MediaError::InvalidArgument`]。
pub fn resolve_destination(desired: &Path, policy: ConflictPolicy) -> Result<Destination> {
    check_file_name(desired)?;
    let exists = exists_no_follow(desired)?;
    Ok(match policy {
        ConflictPolicy::Overwrite => {
            // フォルダ・名前付きパイプ（FIFO）・デバイスなど、通常のファイルでないもの（リンク先を
            // 含む）は置き換えない。リンク先のない（壊れた）シンボリックリンクは、リンクそのものを
            // 置き換える。
            if exists && fs::metadata(desired).is_ok_and(|m| !m.is_file()) {
                return Err(MediaError::invalid_argument(format!(
                    "書き出し先が通常のファイルではない（フォルダ・名前付きパイプなど。{}）",
                    desired.display()
                )));
            }
            Destination::Write {
                path: desired.to_path_buf(),
                replace_existing: exists,
            }
        }
        ConflictPolicy::Skip if exists => Destination::Skip {
            existing: desired.to_path_buf(),
        },
        ConflictPolicy::Skip => Destination::Write {
            path: desired.to_path_buf(),
            replace_existing: false,
        },
        ConflictPolicy::Sequence => Destination::Write {
            path: next_free_sequence(desired, if exists { 1 } else { 0 })?.1,
            replace_existing: false,
        },
    })
}

/// 連番 `start` 以降（0 は連番なしの元の名前）で、何もない名前を探す。（番号, パス）を返す。
fn next_free_sequence(desired: &Path, start: u32) -> Result<(u32, PathBuf)> {
    for n in start..=MAX_SEQUENCE_NUMBER {
        let p = if n == 0 {
            desired.to_path_buf()
        } else {
            sequence_path(desired, n)
        };
        if !exists_no_follow(&p)? {
            return Ok((n, p));
        }
    }
    Err(MediaError::SequenceExhausted {
        path: desired.to_path_buf(),
        max: MAX_SEQUENCE_NUMBER,
    })
}

/// パスの比較に使うキー（絶対パスにして `.`・`..` を字句的に取り除き、大文字と小文字を区別しない
/// ファイルシステムが既定の OS（Windows・macOS）では小文字にしたもの）。
fn path_key(path: &Path) -> OsString {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    fold_case(out.into_os_string())
}

#[cfg(any(windows, target_os = "macos"))]
fn fold_case(s: OsString) -> OsString {
    match s.to_str() {
        Some(t) => OsString::from(t.to_lowercase()),
        None => s,
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn fold_case(s: OsString) -> OsString {
    s
}

/// パスの比較に使うキーの候補（字句的なキーと、実在するフォルダを解決したキー）。
fn path_keys(path: &Path) -> Vec<OsString> {
    let mut keys = vec![path_key(path)];
    // ファイルそのものが実在すれば、シンボリックリンクを解決したパス。
    if let Ok(c) = fs::canonicalize(path) {
        keys.push(path_key(&c));
    } else if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        // ファイルがなければ、フォルダだけ解決する（フォルダのシンボリックリンク経由のパス）。
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        if let Ok(c) = fs::canonicalize(parent) {
            keys.push(path_key(&c.join(name)));
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

/// 通常のファイルなら、同一性（same-file の `Handle`）を得る（リンクは辿る）。それ以外は `None`。
///
/// Unix の `Handle::from_path` はファイルを読み取りで開くため、名前付きパイプ（FIFO）では
/// 書き込む側が現れるまで止まる。開く前に種類を確かめる（確かめてから開くまでの間に
/// 差し替えられることは防げない）。保護するのは写真・動画のファイルなので、通常のファイル以外の
/// 同一性は要らない（パスでは照合する）。
fn file_identity(path: &Path) -> Option<Handle> {
    match fs::metadata(path) {
        Ok(m) if m.is_file() => Handle::from_path(path).ok(),
        _ => None,
    }
}

fn handle_hash(h: &Handle) -> u64 {
    let mut s = DefaultHasher::new();
    h.hash(&mut s);
    s.finish()
}

/// 書き出し先にしてはいけないファイル（カタログに登録されたファイルと、今回の書き出しの入力）。
///
/// 追加するときに、パスのキーと、実在するファイルの同一性（same-file の `Handle` のハッシュ値）を
/// 記録する。ファイルを開いたままにはしない（多数のファイルを登録できるように）。
///
/// 追加には 1 件あたりファイルを開く・パスを解決する数回のシステムコールがかかる（カタログ全体の
/// 数十万件では数秒になりうる）。書き出しのバッチごとに 1 回作って、全件の照合に使い回す。
/// 照合（[`check`](Self::check)）は件数によらず、ほぼ一定の時間で終わる。
#[derive(Debug, Clone, Default)]
pub struct ProtectedFiles {
    keys: HashMap<OsString, PathBuf>,
    identities: HashMap<u64, Vec<PathBuf>>,
    len: usize,
}

impl ProtectedFiles {
    /// 空の一覧。
    pub fn new() -> Self {
        Self::default()
    }

    /// 保護するファイルを追加する。ファイルがなくても（欠落していても）パスで保護する。
    ///
    /// 同一性を確かめるため、ファイルを読み取り専用で一瞬だけ開く（DATA-01）。
    pub fn insert(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref();
        for k in path_keys(path) {
            self.keys.entry(k).or_insert_with(|| path.to_path_buf());
        }
        if let Some(h) = file_identity(path) {
            self.identities
                .entry(handle_hash(&h))
                .or_default()
                .push(path.to_path_buf());
        }
        self.len += 1;
    }

    /// 追加した数。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 空か。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `destination` に書き出してよいか確かめる。保護するファイルと同じなら
    /// [`MediaError::ProtectedDestination`]。
    pub fn check(&self, destination: &Path) -> Result<()> {
        let refuse = |protected: &Path| MediaError::ProtectedDestination {
            destination: destination.to_path_buf(),
            protected: protected.to_path_buf(),
        };
        for k in path_keys(destination) {
            if let Some(p) = self.keys.get(&k) {
                return Err(refuse(p));
            }
        }
        // 既存のファイルなら、同一性（ハードリンク・シンボリックリンクの先を含む）で確かめる。
        if let Some(h) = file_identity(destination)
            && let Some(candidates) = self.identities.get(&handle_hash(&h))
        {
            for p in candidates {
                // ハッシュ値の衝突に備え、開き直して比べる。
                if file_identity(p).is_some_and(|ph| ph == h) {
                    return Err(refuse(p));
                }
            }
        }
        Ok(())
    }
}

impl<P: AsRef<Path>> FromIterator<P> for ProtectedFiles {
    fn from_iter<I: IntoIterator<Item = P>>(iter: I) -> Self {
        let mut s = Self::new();
        for p in iter {
            s.insert(p);
        }
        s
    }
}

impl<P: AsRef<Path>> Extend<P> for ProtectedFiles {
    fn extend<I: IntoIterator<Item = P>>(&mut self, iter: I) {
        for p in iter {
            self.insert(p);
        }
    }
}

/// 書き出しの結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    /// 書き出した。
    Written {
        /// 書き出したパス（連番を付けた場合は付けた後の名前）。
        path: PathBuf,
        /// 既存のファイルを置き換えたか。
        replaced: bool,
    },
    /// 既にファイルがあるので書き出さなかった（スキップの設定）。
    Skipped {
        /// 既にあるファイル。
        existing: PathBuf,
    },
}

fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

fn temp_file_in(dir: &Path) -> Result<tempfile::NamedTempFile> {
    let mut b = tempfile::Builder::new();
    b.prefix(TEMP_PREFIX).suffix(TEMP_SUFFIX);
    // 一時ファイルは既定で所有者だけが読める（0600）ため、公開後に通常のファイルと同じ権限
    // （umask を適用した 0666）になるようにする。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        b.permissions(fs::Permissions::from_mode(0o666));
    }
    b.tempfile_in(dir).map_err(|e| MediaError::io(dir, e))
}

/// 名前の変更の後、フォルダの変更を永続化する（Unix。失敗しても書き出し自体は成功している）。
fn sync_dir(dir: &Path) {
    // Apple でも F_FULLFSYNC が使えなければ fsync に戻す（genzo_model::fs_sync。失敗は無視する）。
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = genzo_model::fs_sync::sync_file_full(&d);
    }
    #[cfg(not(unix))]
    let _ = dir;
}

fn is_already_exists(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::AlreadyExists
}

/// 一時ファイルに書いてから、名前を変更して公開する（04 の 6.4 節）。
///
/// - `desired`: 書き出したいパス。`policy` に従って実際の書き出し先を決める。
/// - `protected`: 書き出し先にしてはいけないファイル（上書きの設定でも必ず確かめる）。
/// - `write`: 一時ファイル（書き出し先と同じフォルダ）に中身を書く関数。エラーを返すかパニックする
///   と、一時ファイルを削除して何も公開しない。
///
/// 書き出し先のフォルダは呼び出し側で作っておく。
pub fn write_atomically<F>(
    desired: &Path,
    policy: ConflictPolicy,
    protected: &ProtectedFiles,
    write: F,
) -> Result<WriteOutcome>
where
    F: FnOnce(&mut File) -> Result<()>,
{
    let (mut target, replace) = match resolve_destination(desired, policy)? {
        Destination::Skip { existing } => {
            // スキップでも、原本と同じかどうかは確かめない（何も書かないため）。
            return Ok(WriteOutcome::Skipped { existing });
        }
        Destination::Write {
            path,
            replace_existing,
        } => (path, replace_existing),
    };
    protected.check(&target)?;
    let dir = parent_dir(&target).to_path_buf();
    let mut tmp = temp_file_in(&dir)?;
    write(tmp.as_file_mut())?;
    // std の `sync_all` は Apple で F_FULLFSYNC に対応しないファイルシステム（SMB など）に書き出すと
    // 毎回失敗するため、fsync に戻す同期を使う（指摘 F19。genzo_model::fs_sync）。
    tmp.as_file_mut()
        .flush()
        .and_then(|()| genzo_model::fs_sync::sync_file_full(tmp.as_file()))
        .map_err(|e| MediaError::io(tmp.path(), e))?;

    if policy == ConflictPolicy::Overwrite {
        tmp.persist(&target)
            .map_err(|e| MediaError::io(&target, e.error))?;
        sync_dir(&dir);
        return Ok(WriteOutcome::Written {
            path: target,
            replaced: replace,
        });
    }

    // 上書きしない設定: 既存のファイルを置き換えない名前の変更。
    // 連番を探し直すときは、前に選んだ番号より後ろだけを探す（「空いている」と判定した名前で
    // 名前の変更が既存のファイルとの衝突を返し続けても、連番の上限で必ず終わるように）。
    let mut next_start = 1;
    loop {
        match tmp.persist_noclobber(&target) {
            Ok(_) => {
                sync_dir(&dir);
                return Ok(WriteOutcome::Written {
                    path: target,
                    replaced: false,
                });
            }
            Err(e) if is_already_exists(&e.error) => {
                tmp = e.file;
                // 決めた後に別のプロセスが同じ名前を作った。
                match policy {
                    ConflictPolicy::Skip => return Ok(WriteOutcome::Skipped { existing: target }),
                    _ => {
                        let (n, next) = next_free_sequence(desired, next_start)?;
                        next_start = n + 1;
                        protected.check(&next)?;
                        target = next;
                    }
                }
            }
            Err(e) => {
                // ハードリンクも RENAME_NOREPLACE も使えないファイルシステム（一部の外付けの
                // ファイルシステムなど）。名前が空いていることを確かめてから通常の名前の変更をする
                // （確かめてから変更するまでの間の競合は防げない）。
                tmp = e.file;
                if exists_no_follow(&target)? {
                    return Err(MediaError::io(&target, e.error));
                }
                tmp.persist(&target)
                    .map_err(|e| MediaError::io(&target, e.error))?;
                sync_dir(&dir);
                return Ok(WriteOutcome::Written {
                    path: target,
                    replaced: false,
                });
            }
        }
    }
}

/// フォルダに残った一時ファイル（[`TEMP_PREFIX`] で始まり [`TEMP_SUFFIX`] で終わる名前）の一覧。
///
/// プロセスが強制終了されたときに残ったものを、呼び出し側が確認して削除するために使う。
pub fn stale_temp_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| MediaError::io(dir, e))? {
        let entry = entry.map_err(|e| MediaError::io(dir, e))?;
        let name = entry.file_name();
        if is_temp_name(&name) {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

fn is_temp_name(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|n| n.starts_with(TEMP_PREFIX) && n.ends_with(TEMP_SUFFIX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// フォルダの中の名前の一覧。
    fn names(dir: &Path) -> HashSet<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    fn write_bytes(bytes: &'static [u8]) -> impl FnOnce(&mut File) -> Result<()> {
        move |f| f.write_all(bytes).map_err(MediaError::io_no_path)
    }

    #[test]
    fn sequence_names() {
        assert_eq!(
            sequence_path(Path::new("/a/b/name.jpg"), 1),
            PathBuf::from("/a/b/name-1.jpg")
        );
        assert_eq!(
            sequence_path(Path::new("x.y.tif"), 12),
            PathBuf::from("x.y-12.tif")
        );
        assert_eq!(
            sequence_path(Path::new("noext"), 2),
            PathBuf::from("noext-2")
        );
    }

    #[test]
    fn resolve_follows_policy() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("name.jpg");
        assert_eq!(
            resolve_destination(&p, ConflictPolicy::Sequence).unwrap(),
            Destination::Write {
                path: p.clone(),
                replace_existing: false
            }
        );
        fs::write(&p, b"x").unwrap();
        fs::write(dir.path().join("name-1.jpg"), b"x").unwrap();
        assert_eq!(
            resolve_destination(&p, ConflictPolicy::Sequence).unwrap(),
            Destination::Write {
                path: dir.path().join("name-2.jpg"),
                replace_existing: false
            }
        );
        assert_eq!(
            resolve_destination(&p, ConflictPolicy::Overwrite).unwrap(),
            Destination::Write {
                path: p.clone(),
                replace_existing: true
            }
        );
        assert_eq!(
            resolve_destination(&p, ConflictPolicy::Skip).unwrap(),
            Destination::Skip {
                existing: p.clone()
            }
        );
        assert!(resolve_destination(Path::new("/"), ConflictPolicy::Sequence).is_err());
        // フォルダへの上書きは拒否する。
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        assert!(resolve_destination(&sub, ConflictPolicy::Overwrite).is_err());
    }

    #[test]
    fn writes_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("out.jpg");
        let none = ProtectedFiles::new();
        let r = write_atomically(&p, ConflictPolicy::Sequence, &none, write_bytes(b"one")).unwrap();
        assert_eq!(
            r,
            WriteOutcome::Written {
                path: p.clone(),
                replaced: false
            }
        );
        let r = write_atomically(&p, ConflictPolicy::Sequence, &none, write_bytes(b"two")).unwrap();
        assert_eq!(
            r,
            WriteOutcome::Written {
                path: dir.path().join("out-1.jpg"),
                replaced: false
            }
        );
        let r =
            write_atomically(&p, ConflictPolicy::Overwrite, &none, write_bytes(b"three")).unwrap();
        assert_eq!(
            r,
            WriteOutcome::Written {
                path: p.clone(),
                replaced: true
            }
        );
        let r = write_atomically(&p, ConflictPolicy::Skip, &none, write_bytes(b"four")).unwrap();
        assert_eq!(
            r,
            WriteOutcome::Skipped {
                existing: p.clone()
            }
        );
        assert_eq!(fs::read(&p).unwrap(), b"three");
        assert_eq!(fs::read(dir.path().join("out-1.jpg")).unwrap(), b"two");
        assert_eq!(
            names(dir.path()),
            HashSet::from(["out.jpg".to_owned(), "out-1.jpg".to_owned()])
        );
        assert!(stale_temp_files(dir.path()).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn published_file_has_normal_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("perm.png");
        write_atomically(
            &p,
            ConflictPolicy::Sequence,
            &ProtectedFiles::new(),
            write_bytes(b"x"),
        )
        .unwrap();
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        // 所有者は読み書きできる。
        assert_eq!(mode & 0o600, 0o600, "{mode:o}");
        // 普通に作ったファイル（0666 に umask を適用したもの）と同じ権限（一時ファイルの既定の
        // 0600 のままでも、umask を無視した 0666 でもない）。
        let normal = dir.path().join("normal.txt");
        fs::write(&normal, b"x").unwrap();
        let expected = fs::metadata(&normal).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, expected, "{mode:o} vs {expected:o}");
    }

    #[test]
    fn failure_midway_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("fail.tif");
        let r = write_atomically(&p, ConflictPolicy::Sequence, &ProtectedFiles::new(), |f| {
            f.write_all(b"partial data").unwrap();
            Err(MediaError::encode("途中で失敗"))
        });
        assert!(matches!(r, Err(MediaError::Encode { .. })));
        assert!(!p.exists());
        assert!(names(dir.path()).is_empty(), "{:?}", names(dir.path()));

        // 上書きの設定で既存のファイルがあるときも、失敗したら既存のファイルはそのまま。
        fs::write(&p, b"original").unwrap();
        let r = write_atomically(&p, ConflictPolicy::Overwrite, &ProtectedFiles::new(), |f| {
            f.write_all(b"partial").unwrap();
            Err(MediaError::encode("途中で失敗"))
        });
        assert!(r.is_err());
        assert_eq!(fs::read(&p).unwrap(), b"original");
        assert_eq!(names(dir.path()), HashSet::from(["fail.tif".to_owned()]));
    }

    #[test]
    fn panic_midway_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("panic.jpg");
        let path = p.clone();
        let r = std::panic::catch_unwind(move || {
            let _ = write_atomically(
                &path,
                ConflictPolicy::Sequence,
                &ProtectedFiles::new(),
                |f| {
                    f.write_all(b"partial").unwrap();
                    panic!("エンコーダーのパニック");
                },
            );
        });
        assert!(r.is_err());
        assert!(!p.exists());
        assert!(names(dir.path()).is_empty());
    }

    #[test]
    fn missing_directory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("no/such/dir/x.jpg");
        let r = write_atomically(
            &p,
            ConflictPolicy::Sequence,
            &ProtectedFiles::new(),
            write_bytes(b"x"),
        );
        assert!(matches!(r, Err(MediaError::Io { .. })));
    }

    #[test]
    fn protected_path_is_refused_even_with_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("DSC00001.JPG");
        fs::write(&original, b"original").unwrap();
        let protected: ProtectedFiles = [&original].into_iter().collect();
        assert_eq!(protected.len(), 1);
        for policy in [ConflictPolicy::Overwrite, ConflictPolicy::Sequence] {
            // 同じパス、`.` と `..` を含むパス。
            for dest in [
                original.clone(),
                dir.path().join("sub/../DSC00001.JPG"),
                dir.path().join("./DSC00001.JPG"),
            ] {
                if policy == ConflictPolicy::Sequence && dest != original {
                    continue;
                }
                let r = write_atomically(&dest, policy, &protected, write_bytes(b"new"));
                if policy == ConflictPolicy::Overwrite {
                    assert!(
                        matches!(r, Err(MediaError::ProtectedDestination { .. })),
                        "{dest:?}: {r:?}"
                    );
                } else {
                    // 連番の設定では別の名前になるので書き出せる（原本は変わらない）。
                    assert!(r.is_ok());
                }
            }
        }
        assert_eq!(fs::read(&original).unwrap(), b"original");
        // 欠落した（いまはない）原本のパスにも書き出さない。
        let missing = dir.path().join("missing.jpg");
        let protected: ProtectedFiles = [&missing].into_iter().collect();
        assert!(matches!(
            write_atomically(
                &missing,
                ConflictPolicy::Overwrite,
                &protected,
                write_bytes(b"x")
            ),
            Err(MediaError::ProtectedDestination { .. })
        ));
        assert!(!missing.exists());
    }

    #[test]
    fn hard_link_to_original_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("orig.tif");
        fs::write(&original, b"original").unwrap();
        let link = dir.path().join("link.tif");
        fs::hard_link(&original, &link).unwrap();
        let protected: ProtectedFiles = [&original].into_iter().collect();
        let r = write_atomically(
            &link,
            ConflictPolicy::Overwrite,
            &protected,
            write_bytes(b"new"),
        );
        match r {
            Err(MediaError::ProtectedDestination {
                destination,
                protected: p,
            }) => {
                assert_eq!(destination, link);
                assert_eq!(p, original);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(fs::read(&original).unwrap(), b"original");
        assert_eq!(fs::read(&link).unwrap(), b"original");
        // 関係のないファイルは上書きできる。
        let other = dir.path().join("other.tif");
        fs::write(&other, b"x").unwrap();
        write_atomically(
            &other,
            ConflictPolicy::Overwrite,
            &protected,
            write_bytes(b"y"),
        )
        .unwrap();
        assert_eq!(fs::read(&other).unwrap(), b"y");
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_to_original_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let original = real.join("a.jpg");
        fs::write(&original, b"original").unwrap();
        // ファイルへのシンボリックリンク。
        let file_link = dir.path().join("a-link.jpg");
        std::os::unix::fs::symlink(&original, &file_link).unwrap();
        // フォルダへのシンボリックリンク。
        let dir_link = dir.path().join("alias");
        std::os::unix::fs::symlink(&real, &dir_link).unwrap();

        let protected: ProtectedFiles = [&original].into_iter().collect();
        for dest in [file_link.clone(), dir_link.join("a.jpg")] {
            assert!(
                matches!(
                    write_atomically(
                        &dest,
                        ConflictPolicy::Overwrite,
                        &protected,
                        write_bytes(b"x")
                    ),
                    Err(MediaError::ProtectedDestination { .. })
                ),
                "{dest:?}"
            );
        }
        // 保護する側をリンク経由で登録しても、実体のパスへの書き出しを拒否する。
        let protected: ProtectedFiles = [dir_link.join("a.jpg")].into_iter().collect();
        assert!(matches!(
            write_atomically(
                &original,
                ConflictPolicy::Overwrite,
                &protected,
                write_bytes(b"x")
            ),
            Err(MediaError::ProtectedDestination { .. })
        ));
        assert_eq!(fs::read(&original).unwrap(), b"original");
    }

    #[test]
    fn relative_paths_are_compared_as_absolute() {
        let cwd = std::env::current_dir().unwrap();
        let protected: ProtectedFiles = [cwd.join("some-file-that-does-not-exist.jpg")]
            .into_iter()
            .collect();
        assert!(matches!(
            protected.check(Path::new("some-file-that-does-not-exist.jpg")),
            Err(MediaError::ProtectedDestination { .. })
        ));
        assert!(protected.check(Path::new("another.jpg")).is_ok());
    }

    #[test]
    fn sequence_exhaustion() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x");
        fs::write(&p, b"").unwrap();
        // 上限の判定だけを確かめるため、上限 - 1 個までの名前を作るのは重いので、最後の数個だけ
        // 作って next_free_sequence の開始位置で確かめる。
        for n in (MAX_SEQUENCE_NUMBER - 1)..=MAX_SEQUENCE_NUMBER {
            fs::write(sequence_path(&p, n), b"").unwrap();
        }
        assert!(matches!(
            next_free_sequence(&p, MAX_SEQUENCE_NUMBER - 1),
            Err(MediaError::SequenceExhausted { .. })
        ));
        assert_eq!(
            next_free_sequence(&p, MAX_SEQUENCE_NUMBER - 2).unwrap(),
            (
                MAX_SEQUENCE_NUMBER - 2,
                sequence_path(&p, MAX_SEQUENCE_NUMBER - 2)
            )
        );
    }

    /// `f` を別のスレッドで実行し、`limit` 以内に終わらなければ `None`（止まった）を返す。
    #[cfg(unix)]
    fn finishes_within<T: Send + 'static>(
        limit: std::time::Duration,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(limit).ok()
    }

    /// 名前付きパイプ（FIFO）を作る。`mkfifo` がなければ `false`。
    #[cfg(unix)]
    fn make_fifo(path: &Path) -> bool {
        std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .is_ok_and(|s| s.success())
    }

    /// 止まったスレッドが開こうとしている FIFO を、書き込み側で開いて解放する（後片付け）。
    #[cfg(unix)]
    fn release_fifo(path: &Path) {
        let p = path.to_path_buf();
        std::thread::spawn(move || {
            let _ = fs::OpenOptions::new().write(true).open(p);
        });
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_never_opened() {
        // FIFO を読み取りで開くと、書き込む側が現れるまで止まる。保護する一覧への追加・照合・
        // 上書きの設定での書き出しが、FIFO で止まらないこと（再現: 修正前は永久に止まった）。
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe.jpg");
        if !make_fifo(&fifo) {
            eprintln!("mkfifo がないため、スキップする");
            return;
        }
        let limit = Duration::from_secs(5);

        let f = fifo.clone();
        let inserted = finishes_within(limit, move || {
            let mut p = ProtectedFiles::new();
            p.insert(&f);
            p
        });
        let Some(protected) = inserted else {
            release_fifo(&fifo);
            panic!("ProtectedFiles::insert が FIFO で止まった");
        };
        // パスでは保護される。
        assert!(matches!(
            protected.check(&fifo),
            Err(MediaError::ProtectedDestination { .. })
        ));

        // 保護していない FIFO への上書きは、開かずに拒否する（通常のファイルではないため）。
        let f = fifo.clone();
        let r = finishes_within(limit, move || {
            write_atomically(
                &f,
                ConflictPolicy::Overwrite,
                &ProtectedFiles::new(),
                write_bytes(b"x"),
            )
        });
        let Some(r) = r else {
            release_fifo(&fifo);
            panic!("上書きの設定での書き出しが FIFO で止まった");
        };
        assert!(
            matches!(r, Err(MediaError::InvalidArgument { .. })),
            "{r:?}"
        );

        // 連番の設定では、別の名前に書き出す。
        let f = fifo.clone();
        let r = finishes_within(limit, move || {
            write_atomically(
                &f,
                ConflictPolicy::Sequence,
                &ProtectedFiles::new(),
                write_bytes(b"x"),
            )
        });
        let Some(r) = r else {
            release_fifo(&fifo);
            panic!("連番の設定での書き出しが FIFO で止まった");
        };
        assert_eq!(
            r.unwrap(),
            WriteOutcome::Written {
                path: dir.path().join("pipe-1.jpg"),
                replaced: false
            }
        );
        // FIFO はそのまま。
        use std::os::unix::fs::FileTypeExt;
        assert!(fs::symlink_metadata(&fifo).unwrap().file_type().is_fifo());
    }

    #[test]
    fn overwrite_refuses_non_regular_destinations() {
        // フォルダへのシンボリックリンクも、フォルダと同じく上書きしない。
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        assert!(matches!(
            resolve_destination(&sub, ConflictPolicy::Overwrite),
            Err(MediaError::InvalidArgument { .. })
        ));
        #[cfg(unix)]
        {
            let link = dir.path().join("sub-link");
            std::os::unix::fs::symlink(&sub, &link).unwrap();
            assert!(matches!(
                resolve_destination(&link, ConflictPolicy::Overwrite),
                Err(MediaError::InvalidArgument { .. })
            ));
            // リンク先のない（壊れた）シンボリックリンクは、リンクそのものを置き換える。
            let dangling = dir.path().join("dangling.jpg");
            std::os::unix::fs::symlink(dir.path().join("nowhere"), &dangling).unwrap();
            let r = write_atomically(
                &dangling,
                ConflictPolicy::Overwrite,
                &ProtectedFiles::new(),
                write_bytes(b"new"),
            )
            .unwrap();
            assert_eq!(
                r,
                WriteOutcome::Written {
                    path: dangling.clone(),
                    replaced: true
                }
            );
            assert!(fs::symlink_metadata(&dangling).unwrap().is_file());
            assert!(!dir.path().join("nowhere").exists());
        }
    }

    #[test]
    fn stale_temp_listing() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".genzo-export-abc.tmp"), b"").unwrap();
        fs::write(dir.path().join("keep.jpg"), b"").unwrap();
        let stale = stale_temp_files(dir.path()).unwrap();
        assert_eq!(stale, vec![dir.path().join(".genzo-export-abc.tmp")]);
    }
}
