//! 検索（フィルター・並べ替え）と、表示範囲の詳細（LIB-07、LIB-08。docs/04_architecture.md の 3.2 節・3.6 節）。
//!
//! - [`Catalog::search`] は、条件に合う variant の id を **すべて** 順番どおりに返す
//!   （グリッドの仮想スクロールの方式。3.2 節）。同じ値のものは `variant.id` の順に並べる。
//!   撮影日時のない（NULL）ものは、昇順でも降順でも **最後** に並べる（3.1 節）。
//! - [`Catalog::variant_summaries`] は、表示範囲の id の配列から詳細を返す。
//! - テキスト検索（3.6 節）は、ファイル名とキャプションを NFKC ＋ 小文字化した列を対象に、
//!   3 文字以上の語を FTS5（trigram）、1〜2 文字の語を `LIKE` で探す。複数の語は AND。
//! - キーワードは子のキーワードを含めて探す（再帰 CTE）。フォルダは配下を含めるかを選べる。
//! - コレクション（LIB-10）・スマートコレクション（LIB-11）で絞り込める（[`Filter::collection`]。条件は
//!   検索のたびに SQL にする（`smart_sql.rs`）ので、写真の評価などが変われば結果も変わる）。
//! - 動画の長さ・fps・コーデック（video_meta）と、寸法（長辺の画素数）で絞り込める（VID-03）。索引は
//!   まだない（動画の件数が少ないうちは不要とみる。PoC-6 の実行計画で判断する）。
//! - 絞り込みの候補（カメラ・レンズ・動画のコーデックと件数。[`Catalog::facets`]）は、[`Filter`] の
//!   `cameras`・`lenses`・`codecs` にそのまま渡して一致する値を返す。件数は asset の数（[`Facets`]）。
//! - 利用者の入力はすべてバインドする値として渡し、SQL の文字列に埋め込まない。

use chrono::{DateTime, Utc};
use genzo_model::{
    AssetId, AssetKind, CaptureTime, CollectionId, ColorLabel, FileId, FileStatus, Flag, FolderId,
    KeywordId, Orientation, Rating, SmartRule, StackId, VariantId,
};
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};

use crate::catalog::Catalog;
use crate::error::{CatalogError, Result};
use crate::register::{CaptureColumns, capture_from_columns};
use crate::stack::StackBadge;
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
    /// 長辺の画素数（`asset.width` と `asset.height` の大きいほう。向きによらない）の下限（この値を含む）。
    /// 写真にも動画にも効く。指定すると寸法の分からないものは除く。
    pub long_edge_min: Option<u32>,
    /// 長辺の画素数の上限（この値を含む）。指定すると寸法の分からないものは除く。
    pub long_edge_max: Option<u32>,
    /// 動画の長さ（秒）の下限（この値を含む。VID-03）。動画の条件（長さ・fps・コーデック）を指定すると、
    /// 写真と、その値の分からない動画は除く。
    pub duration_min_s: Option<f64>,
    /// 動画の長さ（秒）の上限（この値を含む）。
    pub duration_max_s: Option<f64>,
    /// 動画のフレームレートの下限（この値を含む）。
    pub fps_min: Option<f64>,
    /// 動画のフレームレートの上限（この値を含む）。
    pub fps_max: Option<f64>,
    /// 動画のコーデック（ffprobe の `codec_name`。`hevc`・`h264` など。ASCII の大文字・小文字を区別しない。
    /// いずれかに一致）。
    pub codecs: Option<Vec<String>>,
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
    /// コレクション（LIB-10・LIB-11）。手動のコレクションはメンバー、スマートコレクションは条件に一致する
    /// もの、コレクションセットは中のコレクションのいずれかに一致するもの。削除されたコレクションは
    /// どれにも一致しない。
    pub collection: Option<CollectionId>,
    /// スマートコレクションの条件（保存していないもの。条件の編集中の件数など。LIB-11）。
    pub smart_rule: Option<SmartRule>,
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
    /// スタック（LIB-12）に入っていれば、その印（位置・写真の数・折りたたみ）。
    pub stack: Option<StackBadge>,
}

/// 組み立てた検索の SQL とバインドする値（スマートコレクションの条件の変換（`smart_sql.rs`）も使う）。
pub(crate) struct SearchSql {
    pub(crate) sql: String,
    pub(crate) values: Vec<Value>,
    /// `WITH RECURSIVE` に並べる共通表式（キーワード・フォルダの階層）。
    pub(crate) ctes: Vec<String>,
    /// asset の列（撮影日時・カメラ・レンズ・種別・寸法）を使うか（使う場合だけ asset を結合する）。
    pub(crate) needs_asset: bool,
    /// 共通表式の名前の通し番号。
    cte_seq: usize,
}

impl SearchSql {
    fn new() -> Self {
        Self {
            sql: String::new(),
            values: Vec::new(),
            ctes: Vec::new(),
            needs_asset: false,
            cte_seq: 0,
        }
    }

    /// 値を追加して、その位置のプレースホルダ（`?N`）を返す。
    pub(crate) fn bind(&mut self, v: impl Into<Value>) -> String {
        self.values.push(v.into());
        format!("?{}", self.values.len())
    }

    /// 値の列を追加して、`(?1, ?2, ...)` を返す。
    pub(crate) fn bind_list<T: Into<Value>>(
        &mut self,
        items: impl IntoIterator<Item = T>,
    ) -> String {
        let placeholders: Vec<String> = items.into_iter().map(|v| self.bind(v)).collect();
        format!("({})", placeholders.join(", "))
    }

    /// 重ならない共通表式の名前（`kw0`・`ft1` など）。
    pub(crate) fn cte_name(&mut self, prefix: &str) -> String {
        let n = self.cte_seq;
        self.cte_seq += 1;
        format!("{prefix}{n}")
    }

    /// キーワード（子のキーワードを含む）が付いた variant の条件（`v.id IN (...)`）。
    pub(crate) fn keyword_cond(&mut self, keyword: KeywordId) -> String {
        let p = self.bind(keyword.get());
        let name = self.cte_name("kw");
        // UNION（重複を除く）なので、外部のツールで親子が循環していても止まる。
        self.ctes.push(format!(
            "{name}(id) AS (SELECT {p} UNION SELECT k.id FROM keyword k JOIN {name} ON k.parent_id = {name}.id)"
        ));
        format!(
            "v.id IN (SELECT vk.variant_id FROM variant_keyword vk WHERE vk.keyword_id IN (SELECT id FROM {name}))"
        )
    }

    /// フォルダ（配下を含めるか）の条件（`v.asset_id IN (...)`）。
    pub(crate) fn folder_cond(&mut self, folder: FolderFilter) -> String {
        let p = self.bind(folder.folder_id.get());
        if folder.include_subfolders {
            let name = self.cte_name("ft");
            self.ctes.push(format!(
                "{name}(id) AS (SELECT {p} UNION SELECT f.id FROM folder f JOIN {name} t ON f.parent_id = t.id)"
            ));
            format!(
                "v.asset_id IN (SELECT fl.asset_id FROM file fl WHERE fl.folder_id IN (SELECT id FROM {name}))"
            )
        } else {
            format!("v.asset_id IN (SELECT fl.asset_id FROM file fl WHERE fl.folder_id = {p})")
        }
    }

    /// テキスト（3.6 節）の条件。検索語がなければ `None`。
    pub(crate) fn text_cond(&mut self, text: &str) -> Option<String> {
        let tq = parse_text_query(text);
        let mut parts = Vec::new();
        if let Some(m) = tq.fts_match {
            let p = self.bind(m);
            parts.push(format!(
                "v.asset_id IN (SELECT rowid FROM asset_fts WHERE asset_fts MATCH {p})"
            ));
        }
        if !tq.like_patterns.is_empty() {
            let likes: Vec<String> = tq
                .like_patterns
                .into_iter()
                .map(|pat| {
                    let p = self.bind(pat);
                    format!("t.text_norm LIKE {p} ESCAPE '{LIKE_ESCAPE}'")
                })
                .collect();
            parts.push(format!(
                "v.asset_id IN (SELECT t.asset_id FROM asset_text t WHERE {})",
                likes.join(" AND ")
            ));
        }
        match parts.len() {
            0 => None,
            1 => parts.pop(),
            _ => Some(format!("({})", parts.join(" AND "))),
        }
    }
}

/// フィルターと並べ替えから SQL を組み立てる。コレクション（[`Filter::collection`]）は、スマート
/// コレクションの条件・セットの中身を `conn` で読んで SQL にする（`smart_sql.rs`）。
pub(crate) fn build_search_sql(
    conn: &Connection,
    filter: &Filter,
    sort: &Sort,
) -> Result<SearchSql> {
    let mut q = SearchSql::new();
    let mut conds: Vec<String> = Vec::new();
    // asset の列（撮影日時・カメラ・レンズ・種別）を使う場合だけ asset を結合する（q.needs_asset）。
    // asset の ID は variant.asset_id で足りる（外部キーで asset の存在は保証される）。
    // 評価・フラグだけの検索で、50 万件の variant ごとに asset を引かないため（PERF-07）。

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
        q.needs_asset = true;
    }
    if let Some(until) = filter.captured_until {
        let p = q.bind(utc_to_db_string(until));
        conds.push(format!("a.captured_at_utc < {p}"));
        q.needs_asset = true;
    }
    if let Some(cameras) = &filter.cameras {
        let list = q.bind_list(cameras.iter().cloned());
        conds.push(format!("a.camera IN {list}"));
        q.needs_asset = true;
    }
    if let Some(lenses) = &filter.lenses {
        let list = q.bind_list(lenses.iter().cloned());
        conds.push(format!("a.lens IN {list}"));
        q.needs_asset = true;
    }
    if let Some(kind) = filter.kind {
        let p = q.bind(kind.as_str().to_owned());
        conds.push(format!("a.kind = {p}"));
        q.needs_asset = true;
    }
    if let Some(min) = filter.long_edge_min {
        let p = q.bind(i64::from(min));
        conds.push(format!("max(a.width, a.height) >= {p}"));
        q.needs_asset = true;
    }
    if let Some(max) = filter.long_edge_max {
        let p = q.bind(i64::from(max));
        conds.push(format!("max(a.width, a.height) <= {p}"));
        q.needs_asset = true;
    }
    // 動画の情報（VID-03）。video_meta の行のある asset だけが一致する（写真は除く）。
    let mut video_conds: Vec<String> = Vec::new();
    for (value, cond) in [
        (filter.duration_min_s, "vm.duration_s >="),
        (filter.duration_max_s, "vm.duration_s <="),
        (filter.fps_min, "vm.fps >="),
        (filter.fps_max, "vm.fps <="),
    ] {
        if let Some(x) = value {
            let p = q.bind(x);
            video_conds.push(format!("{cond} {p}"));
        }
    }
    if let Some(codecs) = &filter.codecs {
        let list = q.bind_list(codecs.iter().map(|c| c.to_ascii_lowercase()));
        video_conds.push(format!("lower(vm.codec) IN {list}"));
    }
    if !video_conds.is_empty() {
        conds.push(format!(
            "v.asset_id IN (SELECT vm.asset_id FROM video_meta vm WHERE {})",
            video_conds.join(" AND ")
        ));
    }
    if let Some(text) = &filter.text
        && let Some(c) = q.text_cond(text)
    {
        conds.push(c);
    }
    if let Some(folder) = filter.folder {
        let c = q.folder_cond(folder);
        conds.push(c);
    }
    for kw in &filter.keywords {
        let c = q.keyword_cond(*kw);
        conds.push(c);
    }
    if let Some(statuses) = &filter.file_statuses {
        let list = q.bind_list(statuses.iter().map(|s| s.as_str().to_owned()));
        conds.push(format!("pf.status IN {list}"));
    }
    if filter.masters_only {
        conds.push("v.is_master = 1".to_owned());
    }
    // コレクション（LIB-10・LIB-11）と、保存していない条件（スマートコレクションの編集中の件数など）。
    if let Some(id) = filter.collection {
        let c = crate::smart_sql::collection_predicate(conn, &mut q, id, &mut Vec::new())?;
        conds.push(c);
    }
    if let Some(rule) = &filter.smart_rule {
        let c = crate::smart_sql::rule_predicate(conn, &mut q, rule, &mut Vec::new())?;
        conds.push(c);
    }

    let dir = match sort.direction {
        SortDirection::Ascending => "ASC",
        SortDirection::Descending => "DESC",
    };
    let order = match sort.key {
        SortKey::CaptureTime => format!("a.captured_at_utc {dir} NULLS LAST, v.id ASC"),
        SortKey::FileName => format!("pf.name_key {dir} NULLS LAST, v.id ASC"),
        SortKey::Rating => format!("v.rating {dir}, v.id ASC"),
        SortKey::ImportOrder => format!("v.asset_id {dir}, v.id ASC"),
    };
    q.needs_asset |= sort.key == SortKey::CaptureTime;
    let needs_primary_file = sort.key == SortKey::FileName || filter.file_statuses.is_some();

    let mut sql = String::new();
    if !q.ctes.is_empty() {
        sql.push_str("WITH RECURSIVE ");
        sql.push_str(&q.ctes.join(", "));
        sql.push(' ');
    }
    sql.push_str("SELECT v.id FROM variant v");
    if q.needs_asset {
        sql.push_str(" JOIN asset a ON a.id = v.asset_id");
    }
    if needs_primary_file {
        sql.push_str(" LEFT JOIN file pf ON pf.asset_id = v.asset_id AND pf.role = 'primary'");
    }
    if !conds.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conds.join(" AND "));
    }
    sql.push_str(" ORDER BY ");
    sql.push_str(&order);
    q.sql = sql;
    Ok(q)
}

impl Catalog {
    /// 条件に合う variant の id を、並べ替えた順にすべて返す（3.2 節）。
    pub fn search(&self, filter: &Filter, sort: &Sort) -> Result<Vec<VariantId>> {
        let q = build_search_sql(&self.conn, filter, sort)?;
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
        let q = build_search_sql(&self.conn, filter, &Sort::default())?;
        let sql = format!("SELECT count(*) FROM ({})", q.sql);
        let n: i64 = self
            .conn
            .query_row(&sql, params_from_iter(q.values.iter()), |row| row.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// 検索の実行計画（`EXPLAIN QUERY PLAN` の各行の説明）。PoC-6 の確認用。
    pub fn explain_search(&self, filter: &Filter, sort: &Sort) -> Result<Vec<String>> {
        let q = build_search_sql(&self.conn, filter, sort)?;
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

    /// 登録されているカメラの一覧と件数（フィルターの選択肢用。asset の件数。[`Catalog::facets`] の
    /// `cameras` と同じ）。
    pub fn distinct_cameras(&self) -> Result<Vec<(String, u64)>> {
        facet_values(&self.conn, FACET_CAMERAS_SQL)
    }

    /// 登録されているレンズの一覧と件数（フィルターの選択肢用。asset の件数。[`Catalog::facets`] の
    /// `lenses` と同じ）。
    pub fn distinct_lenses(&self) -> Result<Vec<(String, u64)>> {
        facet_values(&self.conn, FACET_LENSES_SQL)
    }

    /// 登録されている動画のコーデックの一覧と件数（ASCII の小文字にそろえた値と asset の件数。
    /// [`Catalog::facets`] の `codecs` と同じ）。
    pub fn distinct_codecs(&self) -> Result<Vec<(String, u64)>> {
        facet_values(&self.conn, FACET_CODECS_SQL)
    }

    /// 絞り込みの候補（カメラ・レンズ・動画のコーデックと、それぞれの件数。[`Facets`]）。
    ///
    /// カメラ・レンズは索引だけを読むので、50 万件でも asset の表の行を引かない（PERF-07。テストで
    /// 実行計画を確かめる）。
    pub fn facets(&self) -> Result<Facets> {
        Ok(Facets {
            cameras: facet_values(&self.conn, FACET_CAMERAS_SQL)?,
            lenses: facet_values(&self.conn, FACET_LENSES_SQL)?,
            codecs: facet_values(&self.conn, FACET_CODECS_SQL)?,
        })
    }

    /// 絞り込みの候補の問い合わせの実行計画（問い合わせごとの `EXPLAIN QUERY PLAN` の各行の説明。
    /// カメラ・レンズ・コーデックの順）。PoC-6 の確認用。
    pub fn explain_facets(&self) -> Result<[Vec<String>; 3]> {
        let plan = |sql: &str| -> Result<Vec<String>> {
            let mut stmt = self.conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(3))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        };
        Ok([
            plan(FACET_CAMERAS_SQL)?,
            plan(FACET_LENSES_SQL)?,
            plan(FACET_CODECS_SQL)?,
        ])
    }
}

/// 絞り込みの候補（[`Catalog::facets`]）。値と件数の組を、値の順（バイト列の順）に並べたもの。
///
/// - **件数は asset（写真・動画 1 つ）の数**で、仮想コピー（variant）は数えない。variant を数えると、
///   variant と asset を結合して 50 万件の行を引くことになり、索引だけを読む問い合わせにできないため
///   （PERF-07）。そのため、仮想コピーがあると、その値で絞り込んだ検索の件数（variant の数）は、ここの
///   件数より多くなる。
/// - 値が NULL・空の文字列のものは候補にしない。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Facets {
    /// カメラ（`asset.camera` の値そのもの。[`Filter::cameras`] にそのまま渡すと一致する）。
    pub cameras: Vec<(String, u64)>,
    /// レンズ（`asset.lens` の値そのもの。[`Filter::lenses`] にそのまま渡すと一致する）。
    pub lenses: Vec<(String, u64)>,
    /// 動画のコーデック（`video_meta.codec` を ASCII の小文字にそろえてまとめたもの。[`Filter::codecs`]
    /// は大文字・小文字を区別しないので、この値で一致する）。
    pub codecs: Vec<(String, u64)>,
}

/// カメラの候補。`asset_camera` の索引だけを順に読む（表の行を引かない。`camera > ''` で NULL と空の
/// 文字列を除く）。
const FACET_CAMERAS_SQL: &str =
    "SELECT camera, count(*) FROM asset WHERE camera > '' GROUP BY camera ORDER BY camera";

/// レンズの候補（`asset_lens` の索引だけを順に読む）。
const FACET_LENSES_SQL: &str =
    "SELECT lens, count(*) FROM asset WHERE lens > '' GROUP BY lens ORDER BY lens";

/// 動画のコーデックの候補。video_meta（動画の行だけ）を読む。コーデックの索引はまだない（検索の
/// コーデックの条件と同じく、動画の件数が少ないうちは不要とみる。モジュールの doc）。
const FACET_CODECS_SQL: &str = "SELECT lower(codec) AS c, count(*) FROM video_meta \
     WHERE codec > '' GROUP BY c ORDER BY c";

/// 候補の問い合わせ（値と件数の 2 列）を実行する。
fn facet_values(conn: &Connection, sql: &str) -> Result<Vec<(String, u64)>> {
    let mut stmt = conn.prepare_cached(sql)?;
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
                EXISTS(SELECT 1 FROM file s WHERE s.asset_id = a.id AND s.role = 'sidecar_jpeg'),
                sm.stack_id, st.collapsed,
                (SELECT count(*) FROM stack_member c WHERE c.stack_id = sm.stack_id),
                (SELECT count(*) FROM stack_member o
                  WHERE o.stack_id = sm.stack_id
                    AND (o.position < sm.position
                         OR (o.position = sm.position AND o.variant_id < sm.variant_id))) + 1,
                a.tz_basis
         FROM json_each(?1) j
         JOIN variant v ON v.id = j.value
         JOIN asset a ON a.id = v.asset_id
         LEFT JOIN file pf ON pf.asset_id = a.id AND pf.role = 'primary'
         LEFT JOIN stack_member sm ON sm.variant_id = v.id
         LEFT JOIN stack st ON st.id = sm.stack_id
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
        let stack = match row.get::<_, Option<i64>>(27)? {
            Some(stack_id) => Some(StackBadge {
                stack_id: StackId::new(stack_id),
                collapsed: row.get::<_, Option<bool>>(28)?.unwrap_or(true),
                count: i64_to_u32(row.get(29)?, "スタックの写真の数")?,
                position: i64_to_u32(row.get(30)?, "スタックの中の位置")?,
            }),
            None => None,
        };
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
            capture: capture_from_columns(CaptureColumns {
                raw: row.get(10)?,
                offset: row.get(11)?,
                tz_source: &tz_source,
                tz_assumed: row.get(13)?,
                basis: row.get(31)?,
                correction_s: row.get(14)?,
                utc: utc.as_deref(),
            })?,
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
            stack,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 表のない接続（コレクションを使わない条件の SQL の組み立てには表が要らない）。
    fn mem() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn user_text_is_bound_not_embedded() {
        let filter = Filter {
            text: Some("'; DROP TABLE asset; -- 京都".to_owned()),
            cameras: Some(vec!["x' OR 1=1".to_owned()]),
            ..Default::default()
        };
        let q = build_search_sql(&mem(), &filter, &Sort::default()).unwrap();
        assert!(!q.sql.contains("DROP"));
        assert!(!q.sql.contains("OR 1=1"));
        assert!(!q.sql.contains("京都"));
        assert!(q.values.len() >= 3);
    }

    #[test]
    fn null_capture_times_sort_last_in_both_directions() {
        for direction in [SortDirection::Ascending, SortDirection::Descending] {
            let q = build_search_sql(
                &mem(),
                &Filter::default(),
                &Sort::new(SortKey::CaptureTime, direction),
            )
            .unwrap();
            assert!(q.sql.contains("NULLS LAST, v.id ASC"), "{}", q.sql);
        }
    }

    #[test]
    fn asset_is_joined_only_when_its_columns_are_used() {
        let joined = |filter: &Filter, sort: Sort| {
            build_search_sql(&mem(), filter, &sort)
                .unwrap()
                .sql
                .contains("JOIN asset a")
        };
        let rating = Sort::new(SortKey::Rating, SortDirection::Descending);
        // 評価・フラグ・テキスト・フォルダ・キーワードと、評価順・登録順・ファイル名順では結合しない。
        let no_asset_columns = Filter {
            rating_min: Rating::new(3),
            flags: Some(vec![Flag::Picked]),
            text: Some("京都 海".to_owned()),
            folder: Some(FolderFilter {
                folder_id: FolderId::new(1),
                include_subfolders: true,
            }),
            keywords: vec![KeywordId::new(1)],
            ..Default::default()
        };
        assert!(!joined(&no_asset_columns, rating));
        for key in [SortKey::ImportOrder, SortKey::FileName] {
            assert!(!joined(
                &Filter::default(),
                Sort::new(key, SortDirection::Ascending)
            ));
        }
        // 撮影日時順・撮影日時・カメラ・レンズ・種別・寸法では結合する。
        assert!(joined(&Filter::default(), Sort::default()));
        for filter in [
            Filter {
                captured_from: Some(DateTime::<Utc>::UNIX_EPOCH),
                ..Default::default()
            },
            Filter {
                captured_until: Some(DateTime::<Utc>::UNIX_EPOCH),
                ..Default::default()
            },
            Filter {
                cameras: Some(vec!["X".to_owned()]),
                ..Default::default()
            },
            Filter {
                lenses: Some(vec!["Y".to_owned()]),
                ..Default::default()
            },
            Filter {
                kind: Some(AssetKind::Video),
                ..Default::default()
            },
            Filter {
                long_edge_min: Some(3840),
                ..Default::default()
            },
        ] {
            assert!(joined(&filter, rating), "{filter:?}");
        }
        // 動画の情報だけの条件では asset を結合しない（video_meta の副問い合わせ）。
        let video_only = Filter {
            duration_max_s: Some(10.0),
            codecs: Some(vec!["HEVC".to_owned()]),
            ..Default::default()
        };
        assert!(!joined(&video_only, rating));
        let q = build_search_sql(&mem(), &video_only, &rating).unwrap();
        assert!(
            !q.sql.contains("HEVC") && !q.sql.contains("hevc"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn primary_file_is_joined_only_when_needed() {
        let q = build_search_sql(&mem(), &Filter::default(), &Sort::default()).unwrap();
        assert!(!q.sql.contains("JOIN file pf"));
        let q = build_search_sql(
            &mem(),
            &Filter::default(),
            &Sort::new(SortKey::FileName, SortDirection::Ascending),
        )
        .unwrap();
        assert!(q.sql.contains("JOIN file pf"));
    }
}
