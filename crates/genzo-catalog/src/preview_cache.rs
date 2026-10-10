//! L1 プレビューのファイルキャッシュ（docs/04_architecture.md の 4 章・4.1 節。PRV-04、SCL-04）。
//!
//! - 配置: `previews/ab/cd/<キー>.jpg`（キーはキャッシュキーのダイジェストの 16 進数。
//!   先頭の 2 文字・次の 2 文字でフォルダを分け、1 つのフォルダのファイル数を抑える）。
//! - 公開: 同じフォルダの一時ファイルに書き、`fsync` してからファイル名を変更する（原子的な置き換え。
//!   生成の途中で終了しても、壊れたプレビューを使わない）。
//! - 最後に使った日時を thumbs.db の索引テーブル（`preview`）に記録し、上限容量を超えたら
//!   古いものから削除する（LRU）。
//! - 索引を失っても（thumbs.db を消した場合など）、[`PreviewCache::reconcile`] で
//!   フォルダの内容から作り直せる。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use genzo_model::VariantId;
use rusqlite::{Connection, OptionalExtension, params};

use crate::backup::sync_parent_dir;
use crate::error::{CatalogError, Result};
use crate::thumbs::{looks_like_jpeg, open_thumbs_connection, validate_cache_key};
use crate::util::{now_utc_string, u64_to_i64};

/// L1 プレビューの上限容量の既定値（バイト）。
///
/// 仮置き: 要件 SCL-04 の「既定値は 20GB 程度」（10 進の GB として 20 × 10⁹ バイト）。
/// 容量の予算（SCL-08）の内訳と合わせて、設定で変更できるようにする。
pub const DEFAULT_PREVIEW_CAPACITY_BYTES: u64 = 20_000_000_000;

/// 回収で一度に調べる行の数。
const EVICT_BATCH: i64 = 256;

/// [`PreviewCache::reconcile`] が一時ファイルを「生成の途中で終了したもの」として削除するまでの、
/// 最後の更新からの経過時間。
///
/// 仮置き: 10 分。突き合わせはアイドル時にも行い、その間もバックグラウンドのジョブが別の
/// [`PreviewCache`]（別の接続）でプレビューを書いている場合がある（04 の 6.1 節）。書き込み中の
/// 一時ファイルを消すと、その公開（名前の変更）が失敗する。プレビュー 1 件（数百 KB〜数 MB）の
/// 書き込みと `fsync` は通常 1 秒未満で終わる見込みなので、それより十分に長い値として置いた。
pub const PREVIEW_TEMP_FILE_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// 一時ファイルの名前の連番（同じプロセスの中での衝突を防ぐ）。
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 回収の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EvictionReport {
    /// 削除したプレビューの数。
    pub removed: usize,
    /// 空けた容量（バイト）。
    pub freed_bytes: u64,
    /// 削除できなかったプレビューの数（他のプログラムが開いている場合など）。索引には残し、
    /// 次の回収でもう一度試す。
    pub failed: usize,
}

/// 索引とフォルダの内容の突き合わせの結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReconcileReport {
    /// フォルダにあって索引になかったため、索引に加えた数。
    pub added: usize,
    /// 索引にあってファイルがなかったため、索引から除いた数。
    pub dropped: usize,
    /// 削除した一時ファイル（生成の途中で終了したもの）の数。
    pub temp_files_removed: usize,
}

/// L1 プレビューのファイルキャッシュ。
pub struct PreviewCache {
    root: PathBuf,
    conn: Connection,
    capacity_bytes: u64,
}

impl std::fmt::Debug for PreviewCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewCache")
            .field("root", &self.root)
            .field("capacity_bytes", &self.capacity_bytes)
            .finish_non_exhaustive()
    }
}

impl PreviewCache {
    /// プレビューのフォルダ `root`（`previews/`）と、索引を置く thumbs.db を開く。
    pub fn open(
        root: impl AsRef<Path>,
        thumbs_db: impl AsRef<Path>,
        capacity_bytes: u64,
    ) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root).map_err(|e| CatalogError::io(&root, e))?;
        Ok(Self {
            root,
            conn: open_thumbs_connection(thumbs_db.as_ref())?,
            capacity_bytes,
        })
    }

    /// 上限容量（バイト）。
    pub fn capacity_bytes(&self) -> u64 {
        self.capacity_bytes
    }

    /// 上限容量を変える（超えていればすぐに回収する）。
    pub fn set_capacity_bytes(&mut self, capacity_bytes: u64) -> Result<EvictionReport> {
        self.capacity_bytes = capacity_bytes;
        self.evict_to(capacity_bytes, None)
    }

    /// プレビューのフォルダ。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// キーのファイルのパス（`root/ab/cd/<キー>.jpg`）。キーは 16 進数の小文字 64 文字。
    pub fn path_for(&self, cache_key: &str) -> Result<PathBuf> {
        validate_cache_key(cache_key)?;
        Ok(self
            .root
            .join(&cache_key[0..2])
            .join(&cache_key[2..4])
            .join(format!("{cache_key}.jpg")))
    }

    /// プレビューを公開する（一時ファイル → 同じフォルダ内でのリネーム）。
    ///
    /// 索引に記録し、上限容量を超えたら古いものから削除する（今回のものは削除しない）。
    pub fn put(
        &mut self,
        cache_key: &str,
        variant_id: Option<VariantId>,
        jpeg: &[u8],
    ) -> Result<PathBuf> {
        let path = self.path_for(cache_key)?;
        if !looks_like_jpeg(jpeg) {
            return Err(CatalogError::InvalidInput(
                "プレビューのデータが JPEG ではありません".to_owned(),
            ));
        }
        let dir = path.parent().expect("キーのパスにはフォルダがある");
        fs::create_dir_all(dir).map_err(|e| CatalogError::io(dir, e))?;
        let tmp = dir.join(format!(
            ".{cache_key}.{}.{}.tmp",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let write = || -> std::io::Result<()> {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            f.write_all(jpeg)?;
            f.sync_all()?;
            Ok(())
        };
        if let Err(e) = write() {
            let _ = fs::remove_file(&tmp);
            return Err(CatalogError::io(&tmp, e));
        }
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(CatalogError::io(&path, e));
        }
        sync_parent_dir(&path);
        let size = u64_to_i64(jpeg.len() as u64, "プレビューの大きさ")?;
        self.record_use(cache_key, variant_id, Some(size))?;
        self.evict_to(self.capacity_bytes, Some(cache_key))?;
        Ok(path)
    }

    /// プレビューのファイルのパスを返し、最後に使った日時を記録する。なければ `None`。
    ///
    /// 索引にあってファイルがなければ索引から除く。ファイルがあって索引になければ索引に加える。
    pub fn get(&mut self, cache_key: &str) -> Result<Option<PathBuf>> {
        let path = self.path_for(cache_key)?;
        match fs::metadata(&path) {
            Ok(meta) if meta.is_file() => {
                let size = u64_to_i64(meta.len(), "プレビューの大きさ")?;
                self.record_use(cache_key, None, Some(size))?;
                Ok(Some(path))
            }
            _ => {
                self.conn
                    .execute("DELETE FROM preview WHERE cache_key = ?1", [cache_key])?;
                Ok(None)
            }
        }
    }

    /// プレビューがあるか（使った記録はしない）。
    pub fn contains(&self, cache_key: &str) -> Result<bool> {
        Ok(self.path_for(cache_key)?.is_file())
    }

    /// プレビューを削除する。削除したら `true`。
    pub fn remove(&mut self, cache_key: &str) -> Result<bool> {
        let path = self.path_for(cache_key)?;
        let existed = match fs::remove_file(&path) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(CatalogError::io(&path, e)),
        };
        let rows = self
            .conn
            .execute("DELETE FROM preview WHERE cache_key = ?1", [cache_key])?;
        Ok(existed || rows > 0)
    }

    /// 索引上の合計の大きさ（バイト）。
    pub fn total_bytes(&self) -> Result<u64> {
        let n: i64 =
            self.conn
                .query_row("SELECT ifnull(sum(size), 0) FROM preview", [], |row| {
                    row.get(0)
                })?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// 索引上のプレビューの数。
    pub fn count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM preview", [], |row| row.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// 使った記録を付ける（索引の行を作るか、最後に使った日時と順番を新しくする）。
    fn record_use(
        &self,
        cache_key: &str,
        variant_id: Option<VariantId>,
        size: Option<i64>,
    ) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO preview(cache_key, variant_id, size, last_used_at, use_seq)
                 VALUES (?1, ?2, ifnull(?3, 0), ?4, (SELECT ifnull(max(use_seq), 0) + 1 FROM preview))
                 ON CONFLICT(cache_key) DO UPDATE SET
                     variant_id = coalesce(excluded.variant_id, variant_id),
                     size = coalesce(?3, size),
                     last_used_at = excluded.last_used_at,
                     use_seq = excluded.use_seq",
            )?
            .execute(params![
                cache_key,
                variant_id.map(VariantId::get),
                size,
                now_utc_string()
            ])?;
        Ok(())
    }

    /// 合計が `capacity` 以下になるまで、最後に使った日時の古いものから削除する。
    ///
    /// `protect` のキーは削除しない（公開した直後のものなど）。削除できないファイル（Windows で
    /// 他のプログラムが開いているものなど）は飛ばして次の古いものへ進み、
    /// [`EvictionReport::failed`] に数える（1 つのファイルのために回収全体や [`PreviewCache::put`] を
    /// 失敗させないため。レビューで再現）。
    pub fn evict_to(&mut self, capacity: u64, protect: Option<&str>) -> Result<EvictionReport> {
        let mut report = EvictionReport::default();
        let mut total = self.total_bytes()?;
        // 飛ばした行を再び読まないよう、(use_seq, cache_key) の位置で区切って先へ読み進める。
        let mut after: (i64, String) = (i64::MIN, String::new());
        while total > capacity {
            let batch: Vec<(String, i64, i64)> = {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT cache_key, size, use_seq FROM preview
                     WHERE cache_key IS NOT ?1 AND (use_seq, cache_key) > (?3, ?4)
                     ORDER BY use_seq, cache_key LIMIT ?2",
                )?;
                stmt.query_map(params![protect, EVICT_BATCH, after.0, after.1], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .collect::<rusqlite::Result<_>>()?
            };
            if batch.is_empty() {
                break;
            }
            for (key, size, seq) in batch {
                if total <= capacity {
                    break;
                }
                after = (seq, key.clone());
                // 索引のキーは書き込み時に検証している。不正なキーの行（外から書き換えられた場合）は、
                // パスを組み立てずに索引から除くだけにする。
                if let Ok(path) = self.path_for(&key) {
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(_) => {
                            report.failed += 1;
                            continue;
                        }
                    }
                }
                self.conn
                    .execute("DELETE FROM preview WHERE cache_key = ?1", [&key])?;
                let size = u64::try_from(size).unwrap_or(0);
                total = total.saturating_sub(size);
                report.removed += 1;
                report.freed_bytes += size;
            }
        }
        Ok(report)
    }

    /// 索引とフォルダの内容を突き合わせる（起動時やアイドル時。索引を失った場合の作り直し）。
    ///
    /// 生成の途中で残った一時ファイルも削除する。ただし、最後の更新から
    /// [`PREVIEW_TEMP_FILE_MIN_AGE`] が経っていないものは、書き込み中の可能性があるので残す。
    pub fn reconcile(&mut self) -> Result<ReconcileReport> {
        let mut report = ReconcileReport::default();
        let mut on_disk = std::collections::HashMap::new();
        for l1 in read_dir_names(&self.root)? {
            let d1 = self.root.join(&l1);
            if !is_hex_dir(&l1) || !d1.is_dir() {
                continue;
            }
            for l2 in read_dir_names(&d1)? {
                let d2 = d1.join(&l2);
                if !is_hex_dir(&l2) || !d2.is_dir() {
                    continue;
                }
                for name in read_dir_names(&d2)? {
                    let p = d2.join(&name);
                    if name.starts_with('.') && name.ends_with(".tmp") {
                        if is_stale_temp_file(&p) && fs::remove_file(&p).is_ok() {
                            report.temp_files_removed += 1;
                        }
                        continue;
                    }
                    let Some(key) = name.strip_suffix(".jpg") else {
                        continue;
                    };
                    if validate_cache_key(key).is_err() || !key.starts_with(&format!("{l1}{l2}")) {
                        continue;
                    }
                    if let Ok(meta) = fs::metadata(&p) {
                        on_disk.insert(key.to_owned(), meta.len());
                    }
                }
            }
        }
        let indexed: Vec<String> = {
            let mut stmt = self.conn.prepare("SELECT cache_key FROM preview")?;
            stmt.query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let tx = self.conn.transaction()?;
        for key in &indexed {
            if !on_disk.contains_key(key) {
                tx.execute("DELETE FROM preview WHERE cache_key = ?1", [key])?;
                report.dropped += 1;
            }
        }
        let known: std::collections::HashSet<&String> = indexed.iter().collect();
        let now = now_utc_string();
        for (key, size) in &on_disk {
            if known.contains(key) {
                continue;
            }
            tx.execute(
                "INSERT INTO preview(cache_key, variant_id, size, last_used_at, use_seq)
                 VALUES (?1, NULL, ?2, ?3, (SELECT ifnull(max(use_seq), 0) + 1 FROM preview))",
                params![key, u64_to_i64(*size, "プレビューの大きさ")?, now],
            )?;
            report.added += 1;
        }
        tx.commit()?;
        Ok(report)
    }

    /// 最後に使った日時（UTC の文字列。テスト・診断用）。
    pub fn last_used_at(&self, cache_key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT last_used_at FROM preview WHERE cache_key = ?1",
                [cache_key],
                |row| row.get(0),
            )
            .optional()?)
    }
}

fn read_dir_names(dir: &Path) -> Result<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(CatalogError::io(dir, e)),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| CatalogError::io(dir, e))?;
        if let Some(name) = entry.file_name().to_str() {
            out.push(name.to_owned());
        }
    }
    Ok(out)
}

/// 一時ファイルが、最後の更新から [`PREVIEW_TEMP_FILE_MIN_AGE`] 以上経っているか。
///
/// 更新日時が読めない、または未来の日時（時計の変更など）の場合は、書き込み中の可能性を
/// 否定できないので `false`（削除しない）とする。
fn is_stale_temp_file(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age >= PREVIEW_TEMP_FILE_MIN_AGE)
}

fn is_hex_dir(name: &str) -> bool {
    name.len() == 2 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
