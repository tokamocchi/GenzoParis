//! バックアップ・復元・整合性チェック（SYS-04、DATA-04、DATA-05。docs/04_architecture.md の 3.4 節）。
//!
//! - バックアップは `VACUUM INTO` で、アプリを使いながら一貫性のあるコピーを作る。
//!   一時ファイルに書き、`fsync` してから同じフォルダ内でファイル名を変更して公開する
//!   （途中で止まっても、不完全なバックアップを正しいものとして扱わないため）。
//! - 世代を管理する（既定 5 世代。古いものから削除する）。削除するのは、この関数が作る
//!   名前の形式（`<カタログ名>-backup-<カタログの印>-<UTC の時刻>.db`）に一致する、**同じカタログの**
//!   ファイルだけ（[`BackupOwner`]）。カタログの印はカタログの ID（[`Catalog::catalog_id`]）の先頭 16 文字で、
//!   同じファイル名の別のカタログが同じ保存先を使っても、互いのバックアップを消さない（F10）。印のない
//!   古い形式の名前（`<カタログ名>-backup-<UTC の時刻>.db`）は一覧には出すが、どのカタログのものか
//!   分からないため、世代の管理では消さない（ID のないカタログでは、古い形式で作って管理する）。
//! - 今作ったバックアップは、世代の管理で消さない（時計が既存のバックアップより前に戻っていても。F09）。
//! - 復元は、バックアップを検証してから **新しいファイル** へ書き出す（既存のファイルは上書きしない）。
//! - 詳細チェック（`integrity_check`・`foreign_key_check`・FTS の索引の確認）は
//!   [`Catalog::check_integrity`] で、アイドル時か手動で行う（起動時は行わない。DATA-05）。

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{Connection, OpenFlags};

use crate::catalog::{
    Catalog, SHUTDOWN_CLEAN, STATE_CACHE_GENERATION, STATE_CATALOG_ID, STATE_SHUTDOWN,
};
use crate::error::{CatalogError, Result};
use crate::schema::{self, CATALOG_SPEC};
use crate::util::timestamp_for_filename;

/// 保持するバックアップの世代数の既定値（DATA-04。要件の表で「仮置き」とされている値）。
pub const DEFAULT_BACKUP_GENERATIONS: usize = 5;

/// バックアップのファイル名の、カタログ名と時刻の間に入れる文字列。
const BACKUP_INFIX: &str = "-backup-";
/// バックアップのファイルの拡張子。
const BACKUP_EXTENSION: &str = ".db";
/// ファイル名に入れるカタログの印の長さ（カタログの ID の先頭。16 進数の小文字）。
const BACKUP_TAG_LEN: usize = 16;

/// 作ったバックアップの情報。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    /// バックアップのファイル。
    pub path: PathBuf,
    /// 作った時刻（ファイル名から読む）。
    pub created_at: DateTime<Utc>,
    /// ファイル名に入っているカタログの印（[`BackupOwner::tag`]）。古い形式の名前では `None`。
    pub catalog_tag: Option<String>,
}

/// バックアップのファイル名で、どのカタログのバックアップかを見分けるための名前（F10）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupOwner {
    /// カタログのファイル名から拡張子を除いたもの。
    pub stem: String,
    /// カタログの印（カタログの ID の先頭 16 文字）。ID のないカタログでは `None`（古い形式の名前を使う）。
    pub tag: Option<String>,
}

impl BackupOwner {
    /// カタログのファイルのパスと ID から作る。
    fn new(catalog_path: Option<&Path>, catalog_id: Option<&str>) -> Self {
        Self {
            stem: catalog_stem(catalog_path),
            tag: catalog_id.and_then(backup_tag_of),
        }
    }

    /// このカタログのバックアップで、世代の管理の対象か（印が同じもの）。
    pub fn owns(&self, info: &BackupInfo) -> bool {
        info.catalog_tag == self.tag
    }
}

/// カタログの ID から、ファイル名に入れる印を作る（16 進数の小文字でなければ `None`）。
fn backup_tag_of(catalog_id: &str) -> Option<String> {
    let tag = catalog_id.get(..BACKUP_TAG_LEN)?;
    is_backup_tag(tag).then(|| tag.to_owned())
}

fn is_backup_tag(s: &str) -> bool {
    s.len() == BACKUP_TAG_LEN && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// バックアップのファイル名（`n` は同じミリ秒に作ったものの連番。0 なら付けない）。
pub(crate) fn backup_file_name(owner: &BackupOwner, at: DateTime<Utc>, n: u32) -> String {
    let mut name = format!("{}{BACKUP_INFIX}", owner.stem);
    if let Some(tag) = &owner.tag {
        name.push_str(tag);
        name.push('-');
    }
    name.push_str(&timestamp_for_filename(at));
    if n > 0 {
        name.push_str(&format!("-{n}"));
    }
    name.push_str(BACKUP_EXTENSION);
    name
}

/// `conn` の DB を `dest` に `VACUUM INTO` で書き出す。
///
/// `dest` が既にあればエラー。一時ファイルに書いて `fsync` し、名前を変えて公開する。
pub(crate) fn vacuum_into_file(conn: &Connection, dest: &Path) -> Result<()> {
    vacuum_into_file_with(conn, dest, |_| Ok(()))
}

/// [`vacuum_into_file`] と同じ。ただし、公開（名前の変更）の前に一時ファイルに `prepare` を行う。
/// `prepare` が失敗した場合は公開せず、一時ファイルを削除する。
fn vacuum_into_file_with(
    conn: &Connection,
    dest: &Path,
    prepare: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    if dest.exists() {
        return Err(CatalogError::AlreadyExists(dest.to_path_buf()));
    }
    let file_name = dest.file_name().ok_or_else(|| {
        CatalogError::InvalidInput(format!("ファイル名がありません: {}", dest.display()))
    })?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(file_name);
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = dest.with_file_name(tmp_name);
    // 前回の失敗で残った一時ファイルは消してよい（このプロセスの ID の名前のものだけ）。
    let _ = fs::remove_file(&tmp);

    let tmp_str = tmp.to_str().ok_or_else(|| {
        CatalogError::InvalidInput(format!(
            "UTF-8 で表せないパスにはバックアップを作れません: {}",
            tmp.display()
        ))
    })?;
    let result = (|| -> Result<()> {
        conn.execute("VACUUM INTO ?1", [tmp_str])?;
        prepare(&tmp)?;
        sync_file(&tmp)?;
        if dest.exists() {
            return Err(CatalogError::AlreadyExists(dest.to_path_buf()));
        }
        fs::rename(&tmp, dest).map_err(|e| CatalogError::io(dest, e))?;
        sync_parent_dir(dest);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// ファイルの内容をストレージへ書き出す（`fsync`）。
///
/// std の `sync_all` は Apple で F_FULLFSYNC に対応しないファイルシステム（SMB の NAS など）では
/// 毎回失敗し、自動バックアップが一度も作られなくなるため、fsync に戻す同期を使う（指摘 F19。
/// [`genzo_model::fs_sync::sync_file_full`]）。
pub(crate) fn sync_file(path: &Path) -> Result<()> {
    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| CatalogError::io(path, e))?;
    genzo_model::fs_sync::sync_file_full(&f)
        .map(|_| ())
        .map_err(|e| CatalogError::io(path, e))
}

/// 名前の変更をストレージへ書き出す（Unix ではフォルダの `fsync` が必要。失敗は無視する）。
pub(crate) fn sync_parent_dir(path: &Path) {
    sync_parent_dir_with(path, |dir| {
        genzo_model::fs_sync::sync_file_full(dir).map(|_| ())
    });
}

/// [`sync_parent_dir`] の、作り直せるキャッシュ用（Apple でも F_FULLFSYNC を使わない。指摘 F19）。
pub(crate) fn sync_parent_dir_fast(path: &Path) {
    sync_parent_dir_with(path, genzo_model::fs_sync::sync_file_fast);
}

fn sync_parent_dir_with(path: &Path, sync: impl FnOnce(&fs::File) -> std::io::Result<()>) {
    #[cfg(unix)]
    if let Some(parent) = path.parent()
        && let Ok(dir) = fs::File::open(parent)
    {
        let _ = sync(&dir);
    }
    #[cfg(not(unix))]
    let _ = (path, sync);
}

/// カタログのファイル `catalog_path` のバックアップを `dest` に作る。
///
/// 読み取り専用の別の接続を使うため、書き込み用の接続（DB 書き込みスレッド）を止めずに
/// 実行できる（WAL では読み取りと書き込みが並行できる）。
pub fn backup_catalog_file(catalog_path: &Path, dest: &Path) -> Result<()> {
    let conn = open_for_reading(catalog_path)?;
    schema::check_identity(&conn, &CATALOG_SPEC, catalog_path)?;
    vacuum_into_file(&conn, dest)
}

fn open_for_reading(path: &Path) -> Result<Connection> {
    if !path.exists() {
        return Err(CatalogError::NotFound(format!(
            "ファイル {}",
            path.display()
        )));
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(schema::BUSY_TIMEOUT)?;
    Ok(conn)
}

/// バックアップのファイル名の、カタログ名の部分（カタログのファイル名から拡張子を除いたもの）。
fn catalog_stem(catalog_path: Option<&Path>) -> String {
    catalog_path
        .and_then(Path::file_stem)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "catalog".to_owned())
}

/// `dir` に、`owner` のカタログの新しいバックアップを作り、`generations` を超えた古いものを削除する。
///
/// `conn` はバックアップ元の接続。
fn create_backup_with(
    conn: &Connection,
    owner: &BackupOwner,
    dir: &Path,
    generations: usize,
) -> Result<BackupInfo> {
    if generations == 0 {
        return Err(CatalogError::InvalidInput(
            "バックアップの世代数は 1 以上にしてください".to_owned(),
        ));
    }
    fs::create_dir_all(dir).map_err(|e| CatalogError::io(dir, e))?;
    let now = Utc::now();
    // 同じミリ秒に 2 回作った場合は、連番を付けて区別する。
    let mut path = dir.join(backup_file_name(owner, now, 0));
    let mut n = 1;
    while path.exists() {
        path = dir.join(backup_file_name(owner, now, n));
        n += 1;
    }
    vacuum_into_file(conn, &path)?;
    // 今作ったものは消さない（時計が既存のバックアップより前に戻っていると、ファイル名の時刻では
    // 今作ったものが最も古い扱いになるため。F09）。世代数は今作ったものを含めて数える。
    prune_backups_keeping(dir, owner, generations, Some(&path))?;
    if !path.is_file() {
        return Err(CatalogError::io(
            &path,
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "作ったバックアップが見つかりません",
            ),
        ));
    }
    Ok(BackupInfo {
        path,
        created_at: now,
        catalog_tag: owner.tag.clone(),
    })
}

/// カタログのファイルから、世代を管理したバックアップを作る（DATA-04）。
///
/// 読み取り専用の別の接続を使う（[`backup_catalog_file`] と同じ）。
pub fn create_backup(catalog_path: &Path, dir: &Path, generations: usize) -> Result<BackupInfo> {
    let conn = open_for_reading(catalog_path)?;
    schema::check_identity(&conn, &CATALOG_SPEC, catalog_path)?;
    let id = crate::catalog::get_app_state(&conn, STATE_CATALOG_ID)?;
    let owner = BackupOwner::new(Some(catalog_path), id.as_deref());
    create_backup_with(&conn, &owner, dir, generations)
}

/// ファイル名を、`stem` のカタログのバックアップとして解析する。形式に合わなければ `None`。
/// 戻り値は (作った時刻, カタログの印。古い形式の名前では `None`)。
fn parse_backup_name(name: &str, stem: &str) -> Option<(DateTime<Utc>, Option<String>)> {
    let rest = name
        .strip_prefix(stem)?
        .strip_prefix(BACKUP_INFIX)?
        .strip_suffix(BACKUP_EXTENSION)?;
    // 新しい形式は "<印 16 文字>-<時刻>"。古い形式は "<時刻>"（時刻は 9 文字目が 'T' なので印と区別できる）。
    let (tag, rest) = match rest.split_once('-') {
        Some((tag, rest)) if is_backup_tag(tag) => (Some(tag.to_owned()), rest),
        _ => (None, rest),
    };
    // "20261009T010203456Z" または "20261009T010203456Z-1"
    let (ts, suffix) = match rest.split_once('-') {
        Some((ts, n)) => (ts, Some(n)),
        None => (rest, None),
    };
    if let Some(n) = suffix
        && (n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let body = ts.strip_suffix('Z')?;
    if body.len() != 18 {
        return None;
    }
    let naive = NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M%S%3f").ok()?;
    Some((naive.and_utc(), tag))
}

/// `dir` にある `owner` のカタログのバックアップを、新しい順に返す。
///
/// 同じカタログの印のものと、印のない古い形式の名前のもの（どのカタログのものか分からない）を返す。
/// 印の違うもの（同じファイル名の別のカタログのもの）は返さない（F10）。
pub fn list_backups(dir: &Path, owner: &BackupOwner) -> Result<Vec<BackupInfo>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(CatalogError::io(dir, e)),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| CatalogError::io(dir, e))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some((created_at, catalog_tag)) = parse_backup_name(name, &owner.stem) else {
            continue;
        };
        if catalog_tag.is_some() && catalog_tag != owner.tag {
            continue;
        }
        let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
        if is_file {
            found.push((
                name.to_owned(),
                BackupInfo {
                    path: entry.path(),
                    created_at,
                    catalog_tag,
                },
            ));
        }
    }
    // 時刻（同じなら名前）の新しい順。
    found.sort_by(|a, b| {
        b.1.created_at
            .cmp(&a.1.created_at)
            .then_with(|| natural_suffix(&b.0).cmp(&natural_suffix(&a.0)))
    });
    Ok(found.into_iter().map(|(_, info)| info).collect())
}

/// 同じ時刻のバックアップの連番（なければ 0）。
fn natural_suffix(name: &str) -> u64 {
    name.strip_suffix(BACKUP_EXTENSION)
        .and_then(|s| s.rsplit_once('-'))
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(0)
}

/// `owner` のカタログのバックアップ（[`BackupOwner::owns`]）のうち、`generations` を超えた古いものを
/// 削除し、削除したファイルを返す。印のない古い形式の名前のものは（ID のないカタログ以外では）消さない。
pub fn prune_backups(dir: &Path, owner: &BackupOwner, generations: usize) -> Result<Vec<PathBuf>> {
    prune_backups_keeping(dir, owner, generations, None)
}

/// [`prune_backups`] と同じ。ただし `keep` のファイルは（ファイル名の時刻によらず）残し、世代数に含めて
/// 数える。
fn prune_backups_keeping(
    dir: &Path,
    owner: &BackupOwner,
    generations: usize,
    keep: Option<&Path>,
) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    let others = list_backups(dir, owner)?
        .into_iter()
        .filter(|b| owner.owns(b) && keep != Some(b.path.as_path()));
    let room = generations.saturating_sub(usize::from(keep.is_some()));
    for info in others.skip(room) {
        fs::remove_file(&info.path).map_err(|e| CatalogError::io(&info.path, e))?;
        removed.push(info.path);
    }
    Ok(removed)
}

/// 復元の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    /// 復元したファイル。
    pub path: PathBuf,
    /// バックアップのスキーマの版（古ければ、次に [`Catalog::open`] したときに移行される）。
    pub schema_version: u32,
}

/// バックアップを検証してから、新しいファイル `dest` へ復元する。
///
/// `dest` が既にあればエラー（既存のカタログを上書きしない）。復元したファイルを
/// [`Catalog::open`] で開いて使う。バックアップのファイルは変更しない。
///
/// バックアップは使用中のカタログから作るため、「使用中」の印（DATA-05）を持っている。
/// そのまま開くと「前回は正常に終了しなかった」と報告してしまうので、検証を通った復元先の
/// コピーには「正常に終了した」印を付けてから公開する（レビューで再現した問題の修正）。
pub fn restore_backup(backup: &Path, dest: &Path) -> Result<RestoreReport> {
    if dest.exists() {
        return Err(CatalogError::AlreadyExists(dest.to_path_buf()));
    }
    let conn = open_for_reading(backup)?;
    let version = schema::check_identity(&conn, &CATALOG_SPEC, backup)?;
    if version == 0 {
        return Err(CatalogError::NotOurDatabase(backup.to_path_buf()));
    }
    let problems = integrity_messages(&conn)?;
    if !problems.is_empty() {
        return Err(CatalogError::IntegrityCheckFailed(problems));
    }
    vacuum_into_file_with(&conn, dest, mark_restored_copy_clean)?;
    Ok(RestoreReport {
        path: dest.to_path_buf(),
        schema_version: version,
    })
}

/// 復元先の一時ファイルに「正常に終了した」印を付ける（公開の前に呼ぶ）。
fn mark_restored_copy_clean(tmp: &Path) -> Result<()> {
    let conn = Connection::open_with_flags(
        tmp,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(schema::BUSY_TIMEOUT)?;
    conn.execute(
        "UPDATE app_state SET value = ?2 WHERE key = ?1",
        [STATE_SHUTDOWN, SHUTDOWN_CLEAN],
    )?;
    // 復元したカタログは、キャッシュの世代を新しくする（バックアップの後に使った ID が再利用されるため、
    // 外部のキャッシュ（thumbs.db・L1 プレビュー）を捨てさせる）。
    conn.execute(
        "INSERT INTO app_state(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [
            STATE_CACHE_GENERATION,
            crate::catalog::new_cache_generation().as_str(),
        ],
    )?;
    conn.close().map_err(|(_, e)| CatalogError::Sqlite(e))
}

/// `PRAGMA integrity_check` の結果（問題がなければ空）。
fn integrity_messages(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("PRAGMA integrity_check")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for r in rows {
        let msg = r?;
        if msg != "ok" {
            out.push(msg);
        }
    }
    Ok(out)
}

/// 外部キーの違反（`PRAGMA foreign_key_check` の 1 行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKeyViolation {
    /// 違反している行のテーブル。
    pub table: String,
    /// 違反している行の rowid（WITHOUT ROWID のテーブルでは `None`）。
    pub rowid: Option<i64>,
    /// 参照先のテーブル。
    pub parent: String,
}

/// 詳細チェックの結果（DATA-05）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IntegrityReport {
    /// `PRAGMA integrity_check` の問題（なければ空）。
    pub integrity_errors: Vec<String>,
    /// `PRAGMA foreign_key_check` の違反。
    pub foreign_key_violations: Vec<ForeignKeyViolation>,
    /// テキスト検索の索引（FTS5）と内容のテーブルの不一致（なければ `None`）。
    pub fts_error: Option<String>,
}

impl IntegrityReport {
    /// 問題がないか。
    pub fn is_ok(&self) -> bool {
        self.integrity_errors.is_empty()
            && self.foreign_key_violations.is_empty()
            && self.fts_error.is_none()
    }
}

impl Catalog {
    /// 詳細チェック（`integrity_check`・`foreign_key_check`・FTS の索引の確認）を行う（DATA-05）。
    ///
    /// DB 全体を読むため時間がかかる（5GB で数十秒の見込み）。起動時ではなく、
    /// アイドル時のバックグラウンドか手動で実行する。問題が見つかった場合は、
    /// バックアップからの復元（[`restore_backup`]）を案内する（6.3 節）。
    pub fn check_integrity(&self) -> Result<IntegrityReport> {
        let integrity_errors = integrity_messages(&self.conn)?;
        let mut stmt = self
            .conn
            .prepare("SELECT \"table\", rowid, parent FROM pragma_foreign_key_check")?;
        let foreign_key_violations = stmt
            .query_map([], |row| {
                Ok(ForeignKeyViolation {
                    table: row.get(0)?,
                    rowid: row.get(1)?,
                    parent: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let fts_error = self.check_text_index()?;
        Ok(IntegrityReport {
            integrity_errors,
            foreign_key_violations,
            fts_error,
        })
    }

    /// テキスト検索の索引（FTS5）と内容のテーブルの一致だけを確かめる（[`Catalog::check_integrity`] の
    /// 一部。DB 全体の走査（`integrity_check`）をしないので、書き込み用の接続で短く済む）。不一致なら
    /// その説明を返す。
    ///
    /// 外部内容テーブルの FTS5 では、rank = 1 で内容のテーブルとの一致も確かめる。読み取り専用の接続では
    /// 実行できない（INSERT の形の命令のため）ので `None` を返す。
    pub fn check_text_index(&self) -> Result<Option<String>> {
        if self.is_read_only() {
            return Ok(None);
        }
        Ok(
            match self.conn.execute(
                "INSERT INTO asset_fts(asset_fts, rank) VALUES ('integrity-check', 1)",
                [],
            ) {
                Ok(_) => None,
                Err(e) => Some(e.to_string()),
            },
        )
    }

    /// テキスト検索の索引を内容のテーブルから作り直す（FTS の不一致が見つかったときの修復）。
    pub fn rebuild_text_index(&mut self) -> Result<()> {
        self.conn
            .execute("INSERT INTO asset_fts(asset_fts) VALUES ('rebuild')", [])?;
        Ok(())
    }

    /// このカタログのバックアップを `dest` に作る（`VACUUM INTO`。この接続を使う）。
    pub fn backup_to(&self, dest: &Path) -> Result<()> {
        vacuum_into_file(&self.conn, dest)
    }

    /// このカタログの、世代を管理したバックアップを `dir` に作る（DATA-04）。
    ///
    /// ファイル名は `<カタログ名>-backup-<カタログの印>-<UTC の時刻>.db`。このカタログの
    /// バックアップのうち、`generations` を超えた古いものは削除する（今作ったものは消さない）。
    pub fn create_backup(&self, dir: &Path, generations: usize) -> Result<BackupInfo> {
        create_backup_with(&self.conn, &self.backup_owner()?, dir, generations)
    }

    /// このカタログのバックアップのファイル名に使う名前（カタログのファイル名から拡張子を除いたもの）。
    pub fn backup_stem(&self) -> String {
        catalog_stem(self.path())
    }

    /// このカタログのバックアップを見分けるための名前（[`list_backups`]・[`prune_backups`] に渡す。F10）。
    pub fn backup_owner(&self) -> Result<BackupOwner> {
        Ok(BackupOwner::new(self.path(), self.catalog_id()?.as_deref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_names_are_parsed_strictly() {
        let (t, tag) =
            parse_backup_name("catalog-backup-20261009T010203456Z.db", "catalog").unwrap();
        assert_eq!(timestamp_for_filename(t), "20261009T010203456Z");
        assert_eq!(tag, None);
        // カタログの印の付いた名前。
        let (t2, tag) = parse_backup_name(
            "catalog-backup-0123456789abcdef-20261009T010203456Z-3.db",
            "catalog",
        )
        .unwrap();
        assert_eq!(t2, t);
        assert_eq!(tag.as_deref(), Some("0123456789abcdef"));
        // 印の形が違うもの（大文字・長さの違い）は対象にしない。
        for bad in [
            "catalog-backup-0123456789ABCDEF-20261009T010203456Z.db",
            "catalog-backup-0123456789abcde-20261009T010203456Z.db",
            "catalog-backup-0123456789abcdef0-20261009T010203456Z.db",
        ] {
            assert!(parse_backup_name(bad, "catalog").is_none(), "{bad}");
        }
        let owner = BackupOwner {
            stem: "catalog".to_owned(),
            tag: backup_tag_of("0123456789abcdef0123456789abcdef"),
        };
        assert_eq!(owner.tag.as_deref(), Some("0123456789abcdef"));
        assert_eq!(
            backup_file_name(&owner, t, 2),
            "catalog-backup-0123456789abcdef-20261009T010203456Z-2.db"
        );
        assert_eq!(backup_tag_of("短い"), None);
        assert_eq!(backup_tag_of("0123456789ABCDEF0123"), None);
        assert!(parse_backup_name("catalog-backup-20261009T010203456Z-2.db", "catalog").is_some());
        // 他のカタログ、形式の違うファイルは対象にしない。
        assert!(parse_backup_name("other-backup-20261009T010203456Z.db", "catalog").is_none());
        assert!(
            parse_backup_name("catalog-backup-20261009T010203456Z.db.tmp", "catalog").is_none()
        );
        assert!(parse_backup_name("catalog-backup-2026.db", "catalog").is_none());
        assert!(parse_backup_name("catalog-backup-20261009T010203456Z-x.db", "catalog").is_none());
        assert!(parse_backup_name("catalog-backup-20261009T010203456Z-.db", "catalog").is_none());
        assert!(parse_backup_name("catalog.db", "catalog").is_none());
        assert_eq!(
            natural_suffix("catalog-backup-20261009T010203456Z-12.db"),
            12
        );
        assert_eq!(natural_suffix("catalog-backup-20261009T010203456Z.db"), 0);
    }
}
