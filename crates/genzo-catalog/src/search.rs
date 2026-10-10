//! 検索（フィルター・並べ替え）と、表示範囲の詳細（LIB-07、LIB-08。docs/04_architecture.md の 3.2 節・3.6 節）。
//!
//! - [`Catalog::search`] は、条件に合う variant の id を **すべて** 順番どおりに返す
//!   （グリッドの仮想スクロールの方式。3.2 節）。同じ値のものは `variant.id` の順に並べる。
//!   撮影日時のない（NULL）ものは、昇順でも降順でも **最後** に並べる（3.1 節）。
//! - [`Catalog::variant_summaries`] は、表示範囲の id の配列から詳細を返す。
//! - テキスト検索（3.6 節）は、ファイル名とキャプションを NFKC ＋ 小文字化した列を対象に、
//!   3 文字以上の語を FTS5（trigram）、1〜2 文字の語を `LIKE` で探す。複数の語は AND。
//! - キーワードは子のキーワードを含めて探す（再帰 CTE）。フォルダは配下を含めるかを選べる。
//! - 利用者の入力はすべてバインドする値として渡し、SQL の文字列に埋め込まない。

use chrono::{DateTime, Utc};
use genzo_model::{
    AssetId, AssetKind, CaptureTime, ColorLabel, FileId, FileStatus, Flag, FolderId, KeywordId,
    Orientation, Rating, VariantId,
};
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};

use crate::catalog::Catalog;
use crate::error::{CatalogError, Result};
use crate::register::capture_from_columns;
use crate::text::{LIKE_ESCAPE, parse_text_query};
use crate::util::{i64_to_u32, ids_to_json, parse_enum, utc_to_db_string};

/// フォルダでの絞り込み。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderFilter {
    /// フォルダ。
    pub folder_id: FolderId,
    /// 配下のフォルダも含めるか。
    pub include_subfolders: bool,
}

/// フィルターの条件（LIB-07）。`None`・空のものは条件にしない。すべての条件を AND で組み合わせる。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Filter {
    /// 評価の下限（この値を含む）。
    pub rating_min: Option<Rating>,
    /// 評価の上限（この値を含む）。
    pub rating_max: Option<Rating>,
    /// フラグ（いずれかに一致）。
    pub flags: Option<Vec<Flag>>,
    /// カラーラベル（いずれかに一致。`None` の要素は「ラベルなし」）。
    pub color_labels: Option<Vec<Option<ColorLabel>>>,
    /// 撮影日時（UTC）の下限（この時刻を含む）。指定すると撮影日時のないものは除く。
    pub captured_from: Option<DateTime<Utc>>,
    /// 撮影日時（UTC）の上限（この時刻を含まない）。指定すると撮影日時のないものは除く。
    pub captured_until: Option<DateTime<Utc>>,
    /// カメラ（`asset.camera` の完全一致。いずれかに一致）。
    pub cameras: Option<Vec<String>>,
    /// レンズ（`asset.lens` の完全一致。いずれかに一致）。
    pub lenses: Option<Vec<String>>,
    /// 種別（写真 / 動画）。
    pub kind: Option<AssetKind>,
    /// テキスト（ファイル名とキャプション。3.6 節）。
    pub text: Option<String>,
    /// フォルダ。
    pub folder: Option<FolderFilter>,
    /// キーワード（それぞれ子のキーワードを含めて一致するものを、すべて持つ）。
    pub keywords: Vec<KeywordId>,
    /// 主となるファイルの状態（いずれかに一致）。
    pub file_statuses: Option<Vec<FileStatus>>,
    /// マスターだけにする（仮想コピーを除く）。
    pub masters_only: bool,
}

/// 並べ替えの項目（LIB-08）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    /// 撮影日時（補正後の UTC）。撮影日時がないものは常に最後。
    #[default]
    CaptureTime,
    /// ファイル名（主となるファイルの比較キー。NFC ＋ 小文字化）。
    FileName,
    /// 評価。
    Rating,
    /// 登録順（asset の登録の順）。
    ImportOrder,
}

/// 並べ替えの向き。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortDirection {
    /// 昇順。
    #[default]
    Ascending,
    /// 降順。
    Descending,
}

/// 並べ替え。同じ値のものは、向きにかかわらず `variant.id` の昇順に並べる（3.2 節）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sort {
    /// 項目。
    pub key: SortKey,
    /// 向き。
    pub direction: SortDirection,
}

impl Sort {
    /// 項目と向きから作る。
    pub const fn new(key: SortKey, direction: SortDirection) -> Self {
        Self { key, direction }
    }
}

/// 表示範囲の 1 件の詳細（グリッドのセルに表示する内容）。
#[derive(Debug, Clone, PartialEq)]
pub struct VariantSummary {
    /// variant。
    pub variant_id: VariantId,
    /// asset。
    pub asset_id: AssetId,
    /// マスターか。
    pub is_master: bool,
    /// 仮想コピーの名前。
    pub variant_name: Option<String>,
    /// 写真か動画か。
    pub kind: AssetKind,
    /// 主となるファイル。
    pub file_id: FileId,
    /// 主となるファイルの名前（表示用）。
    pub file_name: String,
    /// 主となるファイルのフォルダ。
    pub folder_id: FolderId,
    /// 主となるファイルの状態。
    pub file_status: FileStatus,
    /// 主となるファイルのリビジョン（キャッシュキー用）。
    pub file_revision: u32,
    /// RAW と同時に記録された JPEG があるか。
    pub has_sidecar_jpeg: bool,
    /// 評価。
    pub rating: Rating,
    /// フラグ。
    pub flag: Flag,
    /// カラーラベル。
    pub color_label: Option<ColorLabel>,
    /// 撮影日時。
    pub capture: CaptureTime,
    /// カメラ。
    pub camera: Option<String>,
    /// レンズ。
    pub lens: Option<String>,
    /// 幅（画素。向きを適用する前）。
    pub width: Option<u32>,
    /// 高さ（画素。向きを適用する前）。
    pub height: Option<u32>,
    /// 向き。
    pub orientation: Orientation,
    /// 現像設定のハッシュ。
    pub develop_hash: String,
    /// 処理バージョン。
    pub process_version: u32,
}

/// 組み立てた検索の SQL とバインドする値。
struct SearchSql {
    sql: String,
    values: Vec<Value>,
}

impl SearchSql {
    /// 値を追加して、その位置のプレースホルダ（`?N`）を返す。
    fn bind(&mut self, v: impl Into<Value>) -> String {
        self.values.push(v.into());
        format!("?{}", self.values.len())
    }

    /// 値の列を追加して、`(?1, ?2, ...)` を返す。
    fn bind_list<T: Into<Value>>(&mut self, items: impl IntoIterator<Item = T>) -> String {
        let placeholders: Vec<String> = items.into_iter().map(|v| self.bind(v)).collect();
        format!("({})", placeholders.join(", "))
    }
}

/// フィルターと並べ替えから SQL を組み立てる。
fn build_search_sql(filter: &Filter, sort: &Sort) -> SearchSql {
    let mut q = SearchSql {
        sql: String::new(),
        values: Vec::new(),
    };
    let mut ctes: Vec<String> = Vec::new();
    let mut conds: Vec<String> = Vec::new();

    if let Some(min) = filter.rating_min {
        let p = q.bind(i64::from(min));
        conds.push(format!("v.rating >= {p}"));
    }
    if let Some(max) = filter.rating_max {
        let p = q.bind(i64::from(max));
        conds.push(format!("v.rating <= {p}"));
    }
    if let Some(flags) = &filter.flags {
        let list = q.bind_list(flags.iter().map(|f| i64::from(*f)));
        conds.push(format!("v.flag IN {list}"));
    }
    if let Some(labels) = &filter.color_labels {
        let named: Vec<&str> = labels.iter().flatten().map(|l| l.as_str()).collect();
        let include_none = labels.iter().any(Option::is_none);
        let mut parts = Vec::new();
        if !named.is_empty() {
            let list = q.bind_list(named.into_iter().map(str::to_owned));
            parts.push(format!("v.color_label IN {list}"));
        }
        if include_none {
            parts.push("v.color_label IS NULL".to_owned());
        }
        if parts.is_empty() {
            // 空の一覧は「どれにも一致しない」。
            conds.push("0".to_owned());
        } else {
            conds.push(format!("({})", parts.join(" OR ")));
        }
    }
    if let Some(from) = filter.captured_from {
        let p = q.bind(utc_to_db_string(from));
        conds.push(format!("a.captured_at_utc >= {p}"));
    }
    if let Some(until) = filter.captured_until {
        let p = q.bind(utc_to_db_string(until));
        conds.push(format!("a.captured_at_utc < {p}"));
    }
    if let Some(cameras) = &filter.cameras {
        let list = q.bind_list(cameras.iter().cloned());
        conds.push(format!("a.camera IN {list}"));
    }
    if let Some(lenses) = &filter.lenses {
        let list = q.bind_list(lenses.iter().cloned());
        conds.push(format!("a.lens IN {list}"));
    }
    if let Some(kind) = filter.kind {
        let p = q.bind(kind.as_str().to_owned());
        conds.push(format!("a.kind = {p}"));
    }
    if let Some(text) = &filter.text {
        let tq = parse_text_query(text);
        if let Some(m) = tq.fts_match {
            let p = q.bind(m);
            conds.push(format!(
                "a.id IN (SELECT rowid FROM asset_fts WHERE asset_fts MATCH {p})"
            ));
        }
        if !tq.like_patterns.is_empty() {
            let likes: Vec<String> = tq
                .like_patterns
                .into_iter()
                .map(|pat| {
                    let p = q.bind(pat);
                    format!("t.text_norm LIKE {p} ESCAPE '{LIKE_ESCAPE}'")
                })
                .collect();
            conds.push(format!(
                "a.id IN (SELECT t.asset_id FROM asset_text t WHERE {})",
                likes.join(" AND ")
            ));
        }
    }
    if let Some(folder) = filter.folder {
        let p = q.bind(folder.folder_id.get());
        if folder.include_subfolders {
            ctes.push(format!(
                "folder_tree(id) AS (SELECT {p} UNION SELECT f.id FROM folder f JOIN folder_tree t ON f.parent_id = t.id)"
            ));
            conds.push(
                "a.id IN (SELECT fl.asset_id FROM file fl WHERE fl.folder_id IN (SELECT id FROM folder_tree))"
                    .to_owned(),
            );
        } else {
            conds.push(format!(
                "a.id IN (SELECT fl.asset_id FROM file fl WHERE fl.folder_id = {p})"
            ));
        }
    }
    for (i, kw) in filter.keywords.iter().enumerate() {
        let p = q.bind(kw.get());
        ctes.push(format!(
            "kw{i}(id) AS (SELECT {p} UNION SELECT k.id FROM keyword k JOIN kw{i} ON k.parent_id = kw{i}.id)"
        ));
        conds.push(format!(
            "v.id IN (SELECT vk.variant_id FROM variant_keyword vk WHERE vk.keyword_id IN (SELECT id FROM kw{i}))"
        ));
    }
    if let Some(statuses) = &filter.file_statuses {
        let list = q.bind_list(statuses.iter().map(|s| s.as_str().to_owned()));
        conds.push(format!("pf.status IN {list}"));
    }
    if filter.masters_only {
        conds.push("v.is_master = 1".to_owned());
    }

    let dir = match sort.direction {
        SortDirection::Ascending => "ASC",
        SortDirection::Descending => "DESC",
    };
    let order = match sort.key {
        SortKey::CaptureTime => format!("a.captured_at_utc {dir} NULLS LAST, v.id ASC"),
        SortKey::FileName => format!("pf.name_key {dir} NULLS LAST, v.id ASC"),
        SortKey::Rating => format!("v.rating {dir}, v.id ASC"),
        SortKey::ImportOrder => format!("a.id {dir}, v.id ASC"),
    };
    let needs_primary_file = sort.key == SortKey::FileName || filter.file_statuses.is_some();

    let mut sql = String::new();
    if !ctes.is_empty() {
        sql.push_str("WITH RECURSIVE ");
        sql.push_str(&ctes.join(", "));
        sql.push(' ');
    }
    sql.push_str("SELECT v.id FROM variant v JOIN asset a ON a.id = v.asset_id");
    if needs_primary_file {
        sql.push_str(" LEFT JOIN file pf ON pf.asset_id = a.id AND pf.role = 'primary'");
    }
    if !conds.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conds.join(" AND "));
    }
    sql.push_str(" ORDER BY ");
    sql.push_str(&order);
    q.sql = sql;
    q
}

impl Catalog {
    /// 条件に合う variant の id を、並べ替えた順にすべて返す（3.2 節）。
    pub fn search(&self, filter: &Filter, sort: &Sort) -> Result<Vec<VariantId>> {
        let q = build_search_sql(filter, sort);
        let mut stmt = self.conn.prepare(&q.sql)?;
        let rows = stmt.query_map(params_from_iter(q.values.iter()), |row| {
            row.get::<_, i64>(0)
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(VariantId::new(r?));
        }
        Ok(out)
    }

    /// 条件に合う variant の数。
    pub fn count(&self, filter: &Filter) -> Result<u64> {
        let q = build_search_sql(filter, &Sort::default());
        let sql = format!("SELECT count(*) FROM ({})", q.sql);
        let n: i64 = self
            .conn
            .query_row(&sql, params_from_iter(q.values.iter()), |row| row.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// 検索の実行計画（`EXPLAIN QUERY PLAN` の各行の説明）。PoC-6 の確認用。
    pub fn explain_search(&self, filter: &Filter, sort: &Sort) -> Result<Vec<String>> {
        let q = build_search_sql(filter, sort);
        let sql = format!("EXPLAIN QUERY PLAN {}", q.sql);
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(q.values.iter()), |row| {
            row.get::<_, String>(3)
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 表示範囲の詳細を、`ids` と同じ順で返す。存在しない id（検索の後に削除されたものなど）は飛ばす。
    pub fn variant_summaries(&self, ids: &[VariantId]) -> Result<Vec<VariantSummary>> {
        variant_summaries(&self.conn, ids)
    }

    /// 登録されているカメラの一覧と件数（フィルターの選択肢用。asset の件数）。
    pub fn distinct_cameras(&self) -> Result<Vec<(String, u64)>> {
        distinct_values(&self.conn, "camera")
    }

    /// 登録されているレンズの一覧と件数（フィルターの選択肢用。asset の件数）。
    pub fn distinct_lenses(&self) -> Result<Vec<(String, u64)>> {
        distinct_values(&self.conn, "lens")
    }
}

fn distinct_values(conn: &Connection, column: &str) -> Result<Vec<(String, u64)>> {
    // column は呼び出し元の固定の文字列だけ（利用者の入力ではない）。
    let sql = format!(
        "SELECT {column}, count(*) FROM asset WHERE {column} IS NOT NULL GROUP BY {column} ORDER BY {column}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut out = Vec::new();
    for r in rows {
        let (v, n) = r?;
        out.push((v, u64::try_from(n).unwrap_or(0)));
    }
    Ok(out)
}

fn variant_summaries(conn: &Connection, ids: &[VariantId]) -> Result<Vec<VariantSummary>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT v.id, v.asset_id, v.is_master, v.name, v.rating, v.flag, v.color_label,
                v.develop_hash, v.process_version,
                a.kind, a.captured_at_raw, a.captured_offset, a.tz_source, a.tz_assumed,
                a.time_correction_s, a.captured_at_utc, a.camera, a.lens, a.width, a.height,
                a.orientation,
                pf.id, pf.name, pf.folder_id, pf.status, pf.revision,
                EXISTS(SELECT 1 FROM file s WHERE s.asset_id = a.id AND s.role = 'sidecar_jpeg')
         FROM json_each(?1) j
         JOIN variant v ON v.id = j.value
         JOIN asset a ON a.id = v.asset_id
         LEFT JOIN file pf ON pf.asset_id = a.id AND pf.role = 'primary'
         ORDER BY j.key",
    )?;
    let mut rows = stmt.query([ids_to_json(ids)])?;
    let mut out = Vec::with_capacity(ids.len());
    while let Some(row) = rows.next()? {
        let variant_id: i64 = row.get(0)?;
        let rating: i64 = row.get(4)?;
        let flag: i64 = row.get(5)?;
        let color_label: Option<String> = row.get(6)?;
        let kind: String = row.get(9)?;
        let tz_source: String = row.get(12)?;
        let utc: Option<String> = row.get(15)?;
        let orientation: i64 = row.get(20)?;
        let file_id: Option<i64> = row.get(21)?;
        let Some(file_id) = file_id else {
            return Err(CatalogError::Corrupt(format!(
                "variant {variant_id} の asset に主となるファイルがありません"
            )));
        };
        let status: String = row.get(24)?;
        out.push(VariantSummary {
            variant_id: VariantId::new(variant_id),
            asset_id: AssetId::new(row.get(1)?),
            is_master: row.get(2)?,
            variant_name: row.get(3)?,
            kind: parse_enum(&kind)?,
            file_id: FileId::new(file_id),
            file_name: row.get(22)?,
            folder_id: FolderId::new(row.get(23)?),
            file_status: parse_enum(&status)?,
            file_revision: i64_to_u32(row.get(25)?, "revision")?,
            has_sidecar_jpeg: row.get(26)?,
            rating: Rating::try_from(rating).map_err(|e| CatalogError::Corrupt(e.to_string()))?,
            flag: Flag::try_from(flag).map_err(|e| CatalogError::Corrupt(e.to_string()))?,
            color_label: color_label.as_deref().map(parse_enum).transpose()?,
            capture: capture_from_columns(
                row.get(10)?,
                row.get(11)?,
                &tz_source,
                row.get(13)?,
                row.get(14)?,
                utc.as_deref(),
            )?,
            camera: row.get(16)?,
            lens: row.get(17)?,
            width: row
                .get::<_, Option<i64>>(18)?
                .map(|w| i64_to_u32(w, "width"))
                .transpose()?,
            height: row
                .get::<_, Option<i64>>(19)?
                .map(|h| i64_to_u32(h, "height"))
                .transpose()?,
            orientation: u16::try_from(orientation)
                .ok()
                .and_then(Orientation::from_exif)
                .ok_or_else(|| {
                    CatalogError::Corrupt(format!("向きの値が不正です: {orientation}"))
                })?,
            develop_hash: row.get(7)?,
            process_version: i64_to_u32(row.get(8)?, "process_version")?,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_text_is_bound_not_embedded() {
        let filter = Filter {
            text: Some("'; DROP TABLE asset; -- 京都".to_owned()),
            cameras: Some(vec!["x' OR 1=1".to_owned()]),
            ..Default::default()
        };
        let q = build_search_sql(&filter, &Sort::default());
        assert!(!q.sql.contains("DROP"));
        assert!(!q.sql.contains("OR 1=1"));
        assert!(!q.sql.contains("京都"));
        assert!(q.values.len() >= 3);
    }

    #[test]
    fn null_capture_times_sort_last_in_both_directions() {
        for direction in [SortDirection::Ascending, SortDirection::Descending] {
            let q = build_search_sql(
                &Filter::default(),
                &Sort::new(SortKey::CaptureTime, direction),
            );
            assert!(q.sql.contains("NULLS LAST, v.id ASC"), "{}", q.sql);
        }
    }

    #[test]
    fn primary_file_is_joined_only_when_needed() {
        let q = build_search_sql(&Filter::default(), &Sort::default());
        assert!(!q.sql.contains("JOIN file pf"));
        let q = build_search_sql(
            &Filter::default(),
            &Sort::new(SortKey::FileName, SortDirection::Ascending),
        );
        assert!(q.sql.contains("JOIN file pf"));
    }
}
