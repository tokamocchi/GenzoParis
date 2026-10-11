//! サムネイル DB（thumbs.db）の L0 サムネイル（docs/04_architecture.md の 4 章・4.1 節）。
//!
//! - variant ごとに、長辺 320px の JPEG の BLOB と、それを作ったときのキャッシュキー（16 進数）を持つ。
//! - 置き換えは 1 つのトランザクションで行う（生成の途中で終了しても、壊れた行を使わない）。
//! - カタログとは別の DB なので外部キーは使えない。存在しない variant の行は、アイドル時の
//!   回収のジョブで [`ThumbStore::collect_garbage`] により削除する。
//! - 失っても作り直せるため `synchronous = NORMAL` とする（3.4 節。DATA-03b）。
//!
//! L1 プレビューの索引も同じ DB に置く（[`crate::PreviewCache`]）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use genzo_model::VariantId;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::error::{CatalogError, Result};
use crate::hash::is_hex64;
use crate::schema::{self, Durability, THUMBS_SPEC};
use crate::util::{now_utc_string, parse_db_utc};

/// 回収で一度に削除する行の数（1 つのトランザクションを長くしすぎないため）。
const GC_DELETE_BATCH: usize = 1000;

/// thumbs.db を開く（なければ作る）。スキーマの確認と移行、接続の設定を行う。
pub(crate) fn open_thumbs_connection(path: &Path) -> Result<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let mut conn = Connection::open_with_flags(path, flags)?;
    conn.busy_timeout(schema::BUSY_TIMEOUT)?;
    schema::check_identity(&conn, &THUMBS_SPEC, path)?;
    schema::configure_connection(&conn, Durability::Normal, false)?;
    schema::use_immediate_transactions(&mut conn);
    schema::migrate(&mut conn, &THUMBS_SPEC, Some(path), None)?;
    Ok(conn)
}

/// JPEG の先頭の印（SOI マーカー）があるか。
pub(crate) fn looks_like_jpeg(data: &[u8]) -> bool {
    data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF
}

/// キャッシュキーの 16 進数（`CacheKey::hex`）として正しいかを確かめる。
pub(crate) fn validate_cache_key(key: &str) -> Result<()> {
    if is_hex64(key) {
        Ok(())
    } else {
        Err(CatalogError::InvalidInput(format!(
            "キャッシュキーが 16 進数の小文字 64 文字ではありません: {key:?}"
        )))
    }
}

/// 保存されたサムネイル。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    /// 作ったときのキャッシュキー（16 進数）。
    pub cache_key: String,
    /// JPEG のデータ。
    pub jpeg: Vec<u8>,
    /// 保存した日時。
    pub updated_at: DateTime<Utc>,
}

/// サムネイル DB（L0）。
pub struct ThumbStore {
    conn: Connection,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for ThumbStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThumbStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ThumbStore {
    /// thumbs.db を開く（なければ作る）。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        Ok(Self {
            conn: open_thumbs_connection(path)?,
            path: Some(path.to_path_buf()),
        })
    }

    /// メモリ上に作る（テスト用）。
    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        schema::configure_connection(&conn, Durability::Normal, true)?;
        schema::use_immediate_transactions(&mut conn);
        schema::migrate(&mut conn, &THUMBS_SPEC, None, None)?;
        Ok(Self { conn, path: None })
    }

    /// 閉じる。
    pub fn close(self) -> Result<()> {
        self.conn.close().map_err(|(_, e)| CatalogError::Sqlite(e))
    }

    /// 接続の設定を読む（テスト・診断用）。
    pub fn connection_settings(&self) -> Result<crate::ConnectionSettings> {
        crate::catalog::connection_settings(&self.conn)
    }

    /// variant のサムネイルを保存する（あれば置き換える。1 つのトランザクション）。
    pub fn put(&mut self, variant_id: VariantId, cache_key: &str, jpeg: &[u8]) -> Result<()> {
        validate_cache_key(cache_key)?;
        if !looks_like_jpeg(jpeg) {
            return Err(CatalogError::InvalidInput(
                "サムネイルのデータが JPEG ではありません".to_owned(),
            ));
        }
        let tx = self.conn.transaction()?;
        tx.prepare_cached(
            "INSERT INTO thumb(variant_id, cache_key, jpeg, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(variant_id) DO UPDATE SET
                 cache_key = excluded.cache_key, jpeg = excluded.jpeg, updated_at = excluded.updated_at",
        )?
        .execute(params![variant_id.get(), cache_key, jpeg, now_utc_string()])?;
        tx.commit()?;
        Ok(())
    }

    /// variant のサムネイルを読む。
    pub fn get(&self, variant_id: VariantId) -> Result<Option<Thumbnail>> {
        let row = self
            .conn
            .prepare_cached("SELECT cache_key, jpeg, updated_at FROM thumb WHERE variant_id = ?1")?
            .query_row([variant_id.get()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .optional()?;
        row.map(|(cache_key, jpeg, updated_at)| {
            Ok(Thumbnail {
                cache_key,
                jpeg,
                updated_at: parse_db_utc(&updated_at)?,
            })
        })
        .transpose()
    }

    /// キャッシュキーが一致する場合だけ、サムネイルの JPEG を返す（キーが変わったものは使わない）。
    pub fn get_if_current(
        &self,
        variant_id: VariantId,
        cache_key: &str,
    ) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .prepare_cached("SELECT jpeg FROM thumb WHERE variant_id = ?1 AND cache_key = ?2")?
            .query_row(params![variant_id.get(), cache_key], |row| row.get(0))
            .optional()?)
    }

    /// variant のサムネイルのキャッシュキー（なければ `None`）。
    pub fn cache_key(&self, variant_id: VariantId) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT cache_key FROM thumb WHERE variant_id = ?1")?
            .query_row([variant_id.get()], |row| row.get(0))
            .optional()?)
    }

    /// variant のサムネイルを削除する（variant を削除したとき）。削除した数を返す。
    pub fn remove(&mut self, variant_ids: &[VariantId]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let n = tx.execute(
            "DELETE FROM thumb WHERE variant_id IN (SELECT value FROM json_each(?1))",
            [crate::util::ids_to_json(variant_ids)],
        )?;
        tx.commit()?;
        Ok(n)
    }

    /// 存在しない variant の行を削除する（回収のジョブ。4.1 節）。削除した数を返す。
    ///
    /// `alive` はカタログに存在する variant の id の集合（[`crate::Catalog::all_variant_ids`]）。
    pub fn collect_garbage(&mut self, alive: &HashSet<VariantId>) -> Result<usize> {
        let ids: Vec<i64> = {
            let mut stmt = self.conn.prepare("SELECT variant_id FROM thumb")?;
            stmt.query_map([], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let dead: Vec<VariantId> = ids
            .into_iter()
            .map(VariantId::new)
            .filter(|v| !alive.contains(v))
            .collect();
        let mut removed = 0;
        for chunk in dead.chunks(GC_DELETE_BATCH) {
            removed += self.remove(chunk)?;
        }
        // 存在しない variant の作り直し待ちの印も外す。
        let pending: Vec<VariantId> = self
            .regen_pending()?
            .into_iter()
            .filter(|v| !alive.contains(v))
            .collect();
        for v in pending {
            self.clear_regen_pending(v)?;
        }
        Ok(removed)
    }

    /// キャッシュの付随情報（`cache_meta`）の値を読む（キャッシュを作ったカタログの世代など）。
    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM cache_meta WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// キャッシュの付随情報（`cache_meta`）の値を書く。
    pub fn set_meta(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO cache_meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// すべてのサムネイルと作り直し待ちの印を削除する（キャッシュの持ち主のカタログが変わったとき）。
    /// 削除したサムネイルの数を返す。
    pub fn clear(&mut self) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let n = tx.execute("DELETE FROM thumb", [])?;
        tx.execute("DELETE FROM regen_pending", [])?;
        tx.commit()?;
        Ok(n)
    }

    /// 現像結果からの作り直しを待つ印を付ける（作り直しの依頼が終了で取り消されても、次の起動で
    /// 作り直せるように）。
    pub fn mark_regen_pending(&mut self, variant_ids: &[VariantId]) -> Result<()> {
        if variant_ids.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO regen_pending(variant_id, marked_at) VALUES (?1, ?2)
                 ON CONFLICT(variant_id) DO UPDATE SET marked_at = excluded.marked_at",
            )?;
            let now = now_utc_string();
            for v in variant_ids {
                stmt.execute(params![v.get(), now])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 作り直し待ちの印を外す（作り直した・作る必要がなくなったとき）。
    pub fn clear_regen_pending(&mut self, variant_id: VariantId) -> Result<()> {
        self.conn.execute(
            "DELETE FROM regen_pending WHERE variant_id = ?1",
            [variant_id.get()],
        )?;
        Ok(())
    }

    /// 作り直し待ちの印のある variant（古い順）。
    pub fn regen_pending(&self) -> Result<Vec<VariantId>> {
        let mut stmt = self
            .conn
            .prepare("SELECT variant_id FROM regen_pending ORDER BY marked_at, variant_id")?;
        let ids = stmt
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids.into_iter().map(VariantId::new).collect())
    }

    /// 保存しているサムネイルの数。
    pub fn count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM thumb", [], |row| row.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// サムネイルの JPEG の合計の大きさ（バイト。SCL-03 の確認用）。
    pub fn total_bytes(&self) -> Result<u64> {
        let n: i64 = self.conn.query_row(
            "SELECT ifnull(sum(length(jpeg)), 0) FROM thumb",
            [],
            |row| row.get(0),
        )?;
        Ok(u64::try_from(n).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_signature_check() {
        assert!(looks_like_jpeg(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(!looks_like_jpeg(&[0xFF, 0xD8]));
        assert!(!looks_like_jpeg(b"\x89PNG"));
        assert!(validate_cache_key(&"a".repeat(64)).is_ok());
        assert!(validate_cache_key("../../etc/passwd").is_err());
    }
}
