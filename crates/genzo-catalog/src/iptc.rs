//! 説明のメタデータ（LIB-15。genzo-model の [`IptcMetadata`]）の読み書き（スキーマの版 6）。
//!
//! - キャプションは `asset.caption`（MVP から）、そのほかの項目は `asset_iptc`（asset と 1 対 1。どれかの
//!   項目に値がある asset だけが行を持つ）。どちらも asset に付き、仮想コピーも同じ値（04 の 3.1 節）。
//! - 値は書く前に genzo-model の [`normalize_metadata_value`] で整える（空なら削除）。
//! - 複数の asset への変更は 1 つのトランザクションで行い、テキスト検索の対象の項目
//!   （[`MetadataField::searchable`]）を変えたら、テキスト検索の索引（`asset_text`。3.6 節）も作り直す。
//! - 元のファイルには書かない（01 の LIB-15「カタログ内で編集する」）。

use genzo_model::{AssetId, IptcMetadata, MetadataField, normalize_metadata_value};
use rusqlite::{Connection, OptionalExtension, params_from_iter};

use crate::catalog::Catalog;
use crate::error::{CatalogError, Result};
use crate::register::refresh_asset_text;
use crate::util::ids_to_json;

/// `asset_iptc` の列（キャプション以外の項目。[`MetadataField::ALL`] の順）。
pub(crate) fn iptc_columns() -> impl Iterator<Item = MetadataField> {
    MetadataField::ALL
        .into_iter()
        .filter(|&f| f != MetadataField::Caption)
}

/// 複数の asset の、1 つの項目の値のまとめ（[`Catalog::metadata_summary`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataFieldSummary {
    /// 項目。
    pub field: MetadataField,
    /// すべての asset で同じ値ならその値（すべてなしなら `None`）。値が異なれば `None`。
    pub value: Option<String>,
    /// asset によって値が異なるか（値のあるものとないものが混ざる場合を含む）。
    pub mixed: bool,
}

/// 複数の asset の説明のメタデータのまとめ（画面の一括編集で「複数の値」を出すため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataSummary {
    /// 対象の asset の数（存在しないものは数えない）。
    pub assets: u64,
    /// 項目ごとのまとめ（[`MetadataField::ALL`] の順）。
    pub fields: Vec<MetadataFieldSummary>,
}

impl Catalog {
    /// asset の説明のメタデータ（キャプションを含む）。asset がなければ [`CatalogError::NotFound`]。
    pub fn asset_metadata(&self, asset_id: AssetId) -> Result<IptcMetadata> {
        let caption: Option<String> = self
            .conn
            .prepare_cached("SELECT caption FROM asset WHERE id = ?1")?
            .query_row([asset_id.get()], |row| row.get(0))
            .optional()?
            .ok_or_else(|| CatalogError::NotFound(format!("asset {asset_id}")))?;
        let mut out = IptcMetadata {
            caption,
            ..IptcMetadata::default()
        };
        let cols: Vec<MetadataField> = iptc_columns().collect();
        let sql = format!(
            "SELECT {} FROM asset_iptc WHERE asset_id = ?1",
            cols.iter()
                .map(|f| f.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let row: Option<Vec<Option<String>>> = self
            .conn
            .prepare_cached(&sql)?
            .query_row([asset_id.get()], |row| {
                (0..cols.len())
                    .map(|i| row.get::<_, Option<String>>(i))
                    .collect()
            })
            .optional()?;
        if let Some(values) = row {
            for (f, v) in cols.into_iter().zip(values) {
                out.set(f, v);
            }
        }
        Ok(out)
    }

    /// 複数の asset の説明のメタデータを変える（1 つのトランザクション。LIB-15 の一括編集）。
    ///
    /// `changes` は（項目、値）の並びで、値は [`normalize_metadata_value`] で整えてから書く（`None`・空白
    /// だけなら削除）。同じ項目が複数あれば後のものを使う。並んでいない項目は変えない。存在しない asset は
    /// 飛ばす。変えた（存在する）asset の数を返す。値が不正（長すぎる・著作権の状態の値が不正）なら
    /// [`CatalogError::InvalidInput`] で、何も変えない。
    pub fn set_asset_metadata(
        &mut self,
        asset_ids: &[AssetId],
        changes: &[(MetadataField, Option<&str>)],
    ) -> Result<usize> {
        // 値を先に整える（不正な値があれば何も書かない）。同じ項目は後のものを使う。
        let normalized = normalize_changes(changes)?;
        let tx = self.conn.transaction()?;
        let n = set_asset_metadata_tx(&tx, asset_ids, &normalized)?;
        tx.commit()?;
        Ok(n)
    }

    /// 複数の asset の説明のメタデータのまとめ（項目ごとに、共通の値か、異なるか）。存在しない asset は
    /// 数えない。asset が 1 つもなければ、どの項目も値なし・異ならない。
    pub fn metadata_summary(&self, asset_ids: &[AssetId]) -> Result<MetadataSummary> {
        // 項目ごとに: 値のある数・異なる値の数・最小の値（異なる値が 1 つなら、その値）。
        let mut select = vec!["count(*)".to_owned()];
        for f in MetadataField::ALL {
            let col = if f == MetadataField::Caption {
                "a.caption".to_owned()
            } else {
                format!("ip.{}", f.as_str())
            };
            select.push(format!("count({col}), count(DISTINCT {col}), min({col})"));
        }
        let sql = format!(
            "SELECT {} FROM asset AS a LEFT JOIN asset_iptc AS ip ON ip.asset_id = a.id
             WHERE a.id IN (SELECT value FROM json_each(?1))",
            select.join(", ")
        );
        let ids = ids_to_json(asset_ids);
        self.conn
            .prepare_cached(&sql)?
            .query_row([ids], |row| {
                let total: i64 = row.get(0)?;
                let mut fields = Vec::with_capacity(MetadataField::ALL.len());
                for (i, f) in MetadataField::ALL.into_iter().enumerate() {
                    let base = 1 + i * 3;
                    let with_value: i64 = row.get(base)?;
                    let distinct: i64 = row.get(base + 1)?;
                    let min: Option<String> = row.get(base + 2)?;
                    let same = distinct <= 1 && (with_value == 0 || with_value == total);
                    fields.push(MetadataFieldSummary {
                        field: f,
                        value: if same { min } else { None },
                        mixed: !same,
                    });
                }
                Ok(MetadataSummary {
                    assets: u64::try_from(total).unwrap_or(0),
                    fields,
                })
            })
            .map_err(CatalogError::from)
    }
}

/// 説明のメタデータの変更を整える（[`normalize_metadata_value`]。同じ項目は後のものを使う）。値が不正なら
/// [`CatalogError::InvalidInput`]。
pub(crate) fn normalize_changes(
    changes: &[(MetadataField, Option<&str>)],
) -> Result<Vec<(MetadataField, Option<String>)>> {
    let mut normalized: Vec<(MetadataField, Option<String>)> = Vec::new();
    for &(field, value) in changes {
        let v = normalize_metadata_value(field, value)
            .map_err(|e| CatalogError::InvalidInput(e.to_string()))?;
        normalized.retain(|(f, _)| *f != field);
        normalized.push((field, v));
    }
    Ok(normalized)
}

/// トランザクションの中で、複数の asset の説明のメタデータを変える（`normalized` は [`normalize_changes`] で
/// 整えたもの）。変えた（存在する）asset の数を返す（[`Catalog::set_asset_metadata`]・取り込みの一括の付与）。
pub(crate) fn set_asset_metadata_tx(
    tx: &Connection,
    asset_ids: &[AssetId],
    normalized: &[(MetadataField, Option<String>)],
) -> Result<usize> {
    let ids = ids_to_json(asset_ids);
    let existing: Vec<i64> = {
        let mut stmt = tx.prepare_cached(
            "SELECT id FROM asset WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY id",
        )?;
        stmt.query_map([&ids], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    if existing.is_empty() || normalized.is_empty() {
        return Ok(existing.len());
    }
    let existing_json = ids_to_json(&existing);
    if let Some((_, caption)) = normalized
        .iter()
        .find(|(f, _)| *f == MetadataField::Caption)
    {
        tx.prepare_cached(
            "UPDATE asset SET caption = ?2 WHERE id IN (SELECT value FROM json_each(?1))",
        )?
        .execute(rusqlite::params![existing_json, caption])?;
    }
    let iptc: Vec<&(MetadataField, Option<String>)> = normalized
        .iter()
        .filter(|(f, _)| *f != MetadataField::Caption)
        .collect();
    if !iptc.is_empty() {
        if iptc.iter().any(|(_, v)| v.is_some()) {
            tx.prepare_cached(
                "INSERT OR IGNORE INTO asset_iptc(asset_id)
                 SELECT value FROM json_each(?1)",
            )?
            .execute([&existing_json])?;
        }
        // 列の名前は MetadataField の固定の文字列だけ（利用者の入力ではない）。
        let sets = iptc
            .iter()
            .enumerate()
            .map(|(i, (f, _))| format!("{} = ?{}", f.as_str(), i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE asset_iptc SET {sets} WHERE asset_id IN (SELECT value FROM json_each(?1))"
        );
        let mut values: Vec<Option<String>> = vec![Some(existing_json.clone())];
        values.extend(iptc.iter().map(|(_, v)| v.clone()));
        tx.execute(&sql, params_from_iter(values))?;
        // すべての値がなくなった行は消す（値を持つ asset だけが行を持つ）。
        let all_null = iptc_columns()
            .map(|f| format!("{} IS NULL", f.as_str()))
            .collect::<Vec<_>>()
            .join(" AND ");
        tx.execute(
            &format!(
                "DELETE FROM asset_iptc
                 WHERE asset_id IN (SELECT value FROM json_each(?1)) AND {all_null}"
            ),
            [&existing_json],
        )?;
    }
    if normalized.iter().any(|(f, _)| f.searchable()) {
        for &a in &existing {
            refresh_asset_text(tx, a)?;
        }
    }
    Ok(existing.len())
}
