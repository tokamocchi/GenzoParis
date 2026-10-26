//! AI マスクのラスター（DEV-20。スキーマの版 14 の `ai_mask`。docs/04_architecture.md の 7.2 節）。
//!
//! - 推論で作った重みのラスター（8 ビット、行優先）を、内容の SHA-256 をキーにして保存する
//!   （[`Catalog::put_ai_mask`]）。現像設定（genzo-model の `MaskShape::Ai`）はこの SHA-256 で参照する。設定の
//!   JSON にラスターを入れないのは、履歴の 1 件ごとに設定の全体を写すため。
//! - 同じ内容は 1 行（同じ SHA-256 の行があれば、何もしない）。仮想コピー・スナップショット・履歴・設定の貼り付け
//!   （別の asset への貼り付けを含む）は同じ参照を写すので、行は 1 つの asset のものではない: asset を消しても
//!   消さず、推論した写真の記録（`asset_id`）を NULL にする（`ON DELETE SET NULL`）。参照のなくなった行の回収は
//!   [`Catalog::delete_unreferenced_ai_masks`]（variant の今の設定・履歴・スナップショットのどれにも SHA-256 が
//!   現れない行を消す）。
//! - 符号化（`encoding`）はここでは解釈しない（`raw`: ラスターそのまま、`png`: 8 ビットのグレースケールの PNG。
//!   符号化と復号は genzo-api）。SHA-256 は符号化する前の幅・高さと値のもの（genzo-model の `ai_mask_sha256`）。

use chrono::{DateTime, Utc};
use genzo_model::AssetId;
use rusqlite::{OptionalExtension, params};

use crate::catalog::Catalog;
use crate::error::{CatalogError, Result};
use crate::util::{i64_to_u32, now_utc_string, parse_db_utc};

/// 符号化の名前。
pub const AI_MASK_ENCODINGS: [&str; 2] = ["raw", "png"];

/// 保存する AI マスクのラスター。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiMaskInsert {
    /// ラスターの内容のアドレス（符号化する前の幅・高さと値の SHA-256。genzo-model の `ai_mask_sha256`。小文字の
    /// 16 進数 64 文字）。
    pub sha256: String,
    /// 推論した写真。
    pub asset_id: AssetId,
    /// 種類（`subject`・`sky`・`person`。genzo-model の `AiMaskKind::as_str`）。
    pub kind: String,
    /// 推論に使ったモデルの ID。
    pub model: String,
    /// 幅（画素）。
    pub width: u32,
    /// 高さ（画素）。
    pub height: u32,
    /// `data` の符号化（[`AI_MASK_ENCODINGS`]）。
    pub encoding: String,
    /// 符号化したラスター。
    pub data: Vec<u8>,
}

/// 保存した AI マスクのラスター。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiMaskRecord {
    /// 内容の SHA-256。
    pub sha256: String,
    /// 推論した写真（その asset を消した後は `None`）。
    pub asset_id: Option<AssetId>,
    /// 種類。
    pub kind: String,
    /// モデルの ID。
    pub model: String,
    /// 幅（画素）。
    pub width: u32,
    /// 高さ（画素）。
    pub height: u32,
    /// 符号化。
    pub encoding: String,
    /// 符号化したラスター。
    pub data: Vec<u8>,
    /// 作った日時（UTC）。
    pub created_at: DateTime<Utc>,
}

/// AI マスクのラスターの要約（ラスターの中身なし）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiMaskInfo {
    /// 内容の SHA-256。
    pub sha256: String,
    /// 種類。
    pub kind: String,
    /// モデルの ID。
    pub model: String,
    /// 幅（画素）。
    pub width: u32,
    /// 高さ（画素）。
    pub height: u32,
    /// 符号化したラスターのバイト数。
    pub bytes: u64,
    /// 作った日時（UTC）。
    pub created_at: DateTime<Utc>,
}

fn check_sha256(s: &str) -> Result<()> {
    let ok = s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if ok {
        Ok(())
    } else {
        Err(CatalogError::InvalidInput(format!(
            "AI マスクの SHA-256 は小文字の 16 進数 64 文字で指定してください（{s:?}）"
        )))
    }
}

impl Catalog {
    /// AI マスクのラスターを保存する。同じ SHA-256 の行があれば何もしない（`Ok(false)`）。asset がなければ
    /// [`CatalogError::NotFound`]、値が不正なら [`CatalogError::InvalidInput`]。
    pub fn put_ai_mask(&mut self, m: &AiMaskInsert) -> Result<bool> {
        check_sha256(&m.sha256)?;
        if !AI_MASK_ENCODINGS.contains(&m.encoding.as_str()) {
            return Err(CatalogError::InvalidInput(format!(
                "AI マスクの符号化 {:?} は扱えません（raw・png）",
                m.encoding
            )));
        }
        if !["subject", "sky", "person"].contains(&m.kind.as_str()) {
            return Err(CatalogError::InvalidInput(format!(
                "AI マスクの種類 {:?} は扱えません（subject・sky・person）",
                m.kind
            )));
        }
        if m.width == 0 || m.height == 0 {
            return Err(CatalogError::InvalidInput(format!(
                "AI マスクの大きさが不正です（{} × {}）",
                m.width, m.height
            )));
        }
        if m.encoding == "raw" && m.data.len() as u64 != u64::from(m.width) * u64::from(m.height) {
            return Err(CatalogError::InvalidInput(format!(
                "AI マスクのラスターの長さ {} が {} × {} に合いません",
                m.data.len(),
                m.width,
                m.height
            )));
        }
        let tx = self.conn.transaction()?;
        let exists: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM asset WHERE id = ?1",
                [m.asset_id.get()],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(CatalogError::NotFound(format!(
                "asset {} がありません",
                m.asset_id.get()
            )));
        }
        // 同じ内容（SHA-256）があれば何もしない（INSERT OR IGNORE は CHECK の違反も無視するので使わない）。
        let present: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM ai_mask WHERE sha256 = ?1",
                [&m.sha256],
                |r| r.get(0),
            )
            .optional()?;
        if present.is_some() {
            return Ok(false);
        }
        let n = tx.execute(
            "INSERT INTO ai_mask (sha256, asset_id, kind, model, width, height, encoding, data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                m.sha256,
                m.asset_id.get(),
                m.kind,
                m.model,
                i64::from(m.width),
                i64::from(m.height),
                m.encoding,
                m.data,
                now_utc_string(),
            ],
        )?;
        tx.commit()?;
        Ok(n == 1)
    }

    /// SHA-256 `sha256` の AI マスクのラスター（なければ `None`）。
    pub fn ai_mask(&self, sha256: &str) -> Result<Option<AiMaskRecord>> {
        self.conn
            .query_row(
                "SELECT sha256, asset_id, kind, model, width, height, encoding, data, created_at
                 FROM ai_mask WHERE sha256 = ?1",
                [sha256],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Vec<u8>>(7)?,
                        r.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()?
            .map(
                |(sha256, asset_id, kind, model, width, height, encoding, data, created_at)| {
                    Ok(AiMaskRecord {
                        sha256,
                        asset_id: asset_id.map(AssetId::new),
                        kind,
                        model,
                        width: i64_to_u32(width, "ai_mask.width")?,
                        height: i64_to_u32(height, "ai_mask.height")?,
                        encoding,
                        data,
                        created_at: parse_db_utc(&created_at)?,
                    })
                },
            )
            .transpose()
    }

    /// asset `asset_id` の AI マスクのラスターの要約（作った順）。
    pub fn ai_masks_of_asset(&self, asset_id: AssetId) -> Result<Vec<AiMaskInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT sha256, kind, model, width, height, length(data), created_at
             FROM ai_mask WHERE asset_id = ?1 ORDER BY created_at, sha256",
        )?;
        let rows = stmt.query_map([asset_id.get()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (sha256, kind, model, width, height, bytes, created_at) = row?;
            out.push(AiMaskInfo {
                sha256,
                kind,
                model,
                width: i64_to_u32(width, "ai_mask.width")?,
                height: i64_to_u32(height, "ai_mask.height")?,
                bytes: u64::try_from(bytes).unwrap_or(0),
                created_at: parse_db_utc(&created_at)?,
            });
        }
        Ok(out)
    }

    /// AI マスクのラスターの数と、符号化したラスターのバイト数の合計。
    pub fn ai_mask_totals(&self) -> Result<(u64, u64)> {
        let (n, bytes): (i64, Option<i64>) =
            self.conn
                .query_row("SELECT count(*), sum(length(data)) FROM ai_mask", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
        Ok((
            u64::try_from(n).unwrap_or(0),
            u64::try_from(bytes.unwrap_or(0)).unwrap_or(0),
        ))
    }

    /// どの variant の今の設定・履歴・スナップショットにも参照（SHA-256）が現れない AI マスクのラスターを消す
    /// （カタログの保守。消した数を返す）。
    ///
    /// 参照の有無は、設定の JSON の中に SHA-256 の文字列が含まれるか（`instr`）で見る。ラスターの SHA-256 は
    /// 64 文字の 16 進数なので、ほかの値と偶然一致することは実質ない。
    pub fn delete_unreferenced_ai_masks(&mut self) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let n = tx.execute(
            "DELETE FROM ai_mask WHERE NOT EXISTS (
                 SELECT 1 FROM variant v WHERE v.develop_json IS NOT NULL AND instr(v.develop_json, ai_mask.sha256) > 0
             ) AND NOT EXISTS (
                 SELECT 1 FROM history_entry h WHERE h.develop_json IS NOT NULL AND instr(h.develop_json, ai_mask.sha256) > 0
             ) AND NOT EXISTS (
                 SELECT 1 FROM snapshot s WHERE s.develop_json IS NOT NULL AND instr(s.develop_json, ai_mask.sha256) > 0
             )",
            [],
        )?;
        tx.commit()?;
        Ok(n)
    }
}
