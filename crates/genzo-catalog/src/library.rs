//! 選別とメタデータの更新（LIB-04、LIB-14、LIB-15、LIB-16）と、asset の詳細の読み取り（説明の
//! メタデータの一括編集は `iptc`）。
//!
//! 評価・フラグ・カラーラベルは variant に付ける（04 の 3.1 節。仮想コピーごとに違う評価を
//! 付けられる）。キャプションと撮影日時は asset に付ける。一括の変更は 1 つのトランザクションで行う。

use chrono::{DateTime, Utc};
use genzo_model::{
    AssetId, AssetKind, CaptureTime, ColorLabel, Flag, GpsCoord, MetadataField, Orientation,
    Rating, VariantId, VideoMetadata, normalize_metadata_value,
};
use rusqlite::{OptionalExtension, params};

use crate::catalog::Catalog;
use crate::error::{CatalogError, Result};
use crate::register::{CaptureColumns, capture_from_columns, load_capture, refresh_asset_text};
use crate::util::{i64_to_u32, ids_to_json, parse_db_utc, parse_enum};

/// asset の撮影日時の列を書く（変えた行の数）。
pub(crate) fn write_capture(
    conn: &rusqlite::Connection,
    asset_id: i64,
    capture: &CaptureTime,
) -> Result<usize> {
    Ok(conn
        .prepare_cached(
            "UPDATE asset SET captured_at_raw = ?2, captured_offset = ?3, tz_source = ?4,
                 tz_assumed = ?5, time_correction_s = ?6, captured_at_utc = ?7, tz_basis = ?8
             WHERE id = ?1",
        )?
        .execute(params![
            asset_id,
            capture.raw,
            capture.offset,
            capture.tz_source.as_str(),
            capture.tz_assumed,
            capture.correction_s,
            capture.utc_db_string(),
            capture.basis,
        ])?)
}

/// 行の緯度・経度の列（`lat`・`lon` 番目）を座標にする（範囲内の両方の値があるときだけ。片方だけ・範囲外は
/// `None`）。
pub(crate) fn gps_columns(
    row: &rusqlite::Row<'_>,
    lat: usize,
    lon: usize,
) -> rusqlite::Result<Option<GpsCoord>> {
    let lat: Option<f64> = row.get(lat)?;
    let lon: Option<f64> = row.get(lon)?;
    Ok(lat.zip(lon).and_then(|(lat, lon)| GpsCoord::new(lat, lon)))
}

/// asset の詳細（メタデータの表示用。LIB-14）。
#[derive(Debug, Clone, PartialEq)]
pub struct AssetRecord {
    /// ID。
    pub id: AssetId,
    /// 写真か動画か。
    pub kind: AssetKind,
    /// 撮影日時。
    pub capture: CaptureTime,
    /// カメラ。
    pub camera: Option<String>,
    /// カメラの本体のシリアル番号（LIB-16 の機材の区別。記録しないカメラ・版 11 より前に登録したものは `None`）。
    pub camera_serial: Option<String>,
    /// レンズ。
    pub lens: Option<String>,
    /// ISO 感度。
    pub iso: Option<u32>,
    /// 絞り値。
    pub aperture: Option<f64>,
    /// シャッター速度（秒）。
    pub shutter_s: Option<f64>,
    /// 焦点距離（mm）。
    pub focal_mm: Option<f64>,
    /// 幅（画素。向きを適用する前）。
    pub width: Option<u32>,
    /// 高さ（画素。向きを適用する前）。
    pub height: Option<u32>,
    /// 向き。
    pub orientation: Orientation,
    /// GPS の座標（カメラ（ファイル）の記録の値。後から付けた位置は [`AssetRecord::assigned_gps`]）。
    pub gps: Option<GpsCoord>,
    /// 後から付けた位置（LIB-21。カタログの中だけに持つ。付けていなければ `None`）。使う位置は
    /// [`AssetRecord::effective_gps`]。
    pub assigned_gps: Option<GpsCoord>,
    /// キャプション。
    pub caption: Option<String>,
    /// 登録した日時。
    pub created_at: DateTime<Utc>,
    /// 動画の情報（動画の場合）。
    pub video: Option<VideoMetadata>,
}

impl AssetRecord {
    /// 使う位置（LIB-21）: 後から付けた位置があればそれ、なければカメラの位置。
    pub fn effective_gps(&self) -> Option<GpsCoord> {
        self.assigned_gps.or(self.gps)
    }
}

impl Catalog {
    /// 評価を一括で変更する（1 つのトランザクション）。変更した variant の数を返す。
    pub fn set_rating(&mut self, variant_ids: &[VariantId], rating: Rating) -> Result<usize> {
        self.update_variants(variant_ids, "rating", i64::from(rating).into())
    }

    /// フラグを一括で変更する。変更した variant の数を返す。
    pub fn set_flag(&mut self, variant_ids: &[VariantId], flag: Flag) -> Result<usize> {
        self.update_variants(variant_ids, "flag", i64::from(flag).into())
    }

    /// カラーラベルを一括で変更する（`None` でラベルなし）。変更した variant の数を返す。
    pub fn set_color_label(
        &mut self,
        variant_ids: &[VariantId],
        label: Option<ColorLabel>,
    ) -> Result<usize> {
        let value = match label {
            Some(l) => rusqlite::types::Value::Text(l.as_str().to_owned()),
            None => rusqlite::types::Value::Null,
        };
        self.update_variants(variant_ids, "color_label", value)
    }

    fn update_variants(
        &mut self,
        variant_ids: &[VariantId],
        column: &str,
        value: rusqlite::types::Value,
    ) -> Result<usize> {
        // column は呼び出し元の固定の文字列だけ（利用者の入力ではない）。
        let sql = format!(
            "UPDATE variant SET {column} = ?2 WHERE id IN (SELECT value FROM json_each(?1))"
        );
        let tx = self.conn.transaction()?;
        let n = tx.execute(&sql, params![ids_to_json(variant_ids), value])?;
        tx.commit()?;
        Ok(n)
    }

    /// キャプションを変更する（`None` または空白だけで削除）。テキスト検索の索引も更新する。
    ///
    /// 値は genzo-model の [`normalize_metadata_value`] で整える（前後の空白・制御文字を除く。上限
    /// [`genzo_model::MAX_METADATA_CHARS`] 文字を超えれば [`CatalogError::InvalidInput`]）。ほかの説明の
    /// メタデータとまとめて変えるときは [`Catalog::set_asset_metadata`]（LIB-15）。
    pub fn set_caption(&mut self, asset_id: AssetId, caption: Option<&str>) -> Result<()> {
        let caption = normalize_metadata_value(MetadataField::Caption, caption)
            .map_err(|e| CatalogError::InvalidInput(e.to_string()))?;
        let tx = self.conn.transaction()?;
        let n = tx.execute(
            "UPDATE asset SET caption = ?2 WHERE id = ?1",
            params![asset_id.get(), caption],
        )?;
        if n == 0 {
            return Err(CatalogError::NotFound(format!("asset {asset_id}")));
        }
        refresh_asset_text(&tx, asset_id.get())?;
        tx.commit()?;
        Ok(())
    }

    /// 撮影日時を変更する（既定のタイムゾーンの変更による推定し直しなど。修正後の値は
    /// [`CaptureTime::with_default_offset`] などで求める）。利用者の修正（LIB-16）は、履歴を残して元に戻せる
    /// [`Catalog::adjust_capture_times`] で行う。
    pub fn set_capture_time(&mut self, asset_id: AssetId, capture: &CaptureTime) -> Result<()> {
        let n = write_capture(&self.conn, asset_id.get(), capture)?;
        if n == 0 {
            return Err(CatalogError::NotFound(format!("asset {asset_id}")));
        }
        Ok(())
    }

    /// asset のメタデータが空か: 寸法・撮影日時の文字列・カメラ・レンズ・ISO・絞り・シャッター速度・
    /// 焦点距離・GPS がすべてなく、動画の情報もない（読めずに `error` で登録したまま。
    /// implementation_status の No.45）。読めたファイルは寸法が必ずあるので、空とみなすことはない。
    /// asset がなければ [`CatalogError::NotFound`]。
    pub fn asset_metadata_is_empty(&self, asset_id: AssetId) -> Result<bool> {
        asset_metadata_is_empty(&self.conn, asset_id.get())
    }

    /// asset の撮影日時を読む。
    pub fn capture_time(&self, asset_id: AssetId) -> Result<CaptureTime> {
        load_capture(&self.conn, asset_id.get())
    }

    /// asset の詳細を読む。
    pub fn asset(&self, asset_id: AssetId) -> Result<AssetRecord> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT a.kind, a.captured_at_raw, a.captured_offset, a.tz_source, a.tz_assumed,
                    a.time_correction_s, a.captured_at_utc, a.camera, a.lens, a.iso, a.aperture,
                    a.shutter, a.focal, a.width, a.height, a.orientation, a.gps_lat, a.gps_lon,
                    a.caption, a.created_at,
                    m.asset_id, m.duration_s, m.fps, m.codec, m.bit_depth, m.color_transfer,
                    m.color_primaries, a.tz_basis, a.camera_serial, al.lat, al.lon
             FROM asset a LEFT JOIN video_meta m ON m.asset_id = a.id
             LEFT JOIN asset_location al ON al.asset_id = a.id
             WHERE a.id = ?1",
        )?;
        let mut rows = stmt.query([asset_id.get()])?;
        let Some(row) = rows.next()? else {
            return Err(CatalogError::NotFound(format!("asset {asset_id}")));
        };
        let kind: String = row.get(0)?;
        let tz_source: String = row.get(3)?;
        let utc: Option<String> = row.get(6)?;
        let to_u32 = |v: Option<i64>, what: &str| v.map(|x| i64_to_u32(x, what)).transpose();
        let orientation: i64 = row.get(15)?;
        let gps = gps_columns(row, 16, 17)?;
        let assigned_gps = gps_columns(row, 29, 30)?;
        let created_at: String = row.get(19)?;
        let video = match row.get::<_, Option<i64>>(20)? {
            Some(_) => Some(VideoMetadata {
                duration_s: row.get(21)?,
                fps: row.get(22)?,
                codec: row.get(23)?,
                bit_depth: to_u32(row.get(24)?, "bit_depth")?,
                color_transfer: row.get(25)?,
                color_primaries: row.get(26)?,
                width: to_u32(row.get(13)?, "width")?,
                height: to_u32(row.get(14)?, "height")?,
                // 動画の作成日時の元の文字列は、撮影日時の元の文字列として保存している。
                creation_time: row.get(1)?,
                // 機材のタグはメーカーと機種を分けて保存していない（表示名は camera）。
                make: None,
                model: None,
            }),
            None => None,
        };
        Ok(AssetRecord {
            id: asset_id,
            kind: parse_enum(&kind)?,
            capture: capture_from_columns(CaptureColumns {
                raw: row.get(1)?,
                offset: row.get(2)?,
                tz_source: &tz_source,
                tz_assumed: row.get(4)?,
                basis: row.get(27)?,
                correction_s: row.get(5)?,
                utc: utc.as_deref(),
            })?,
            camera: row.get(7)?,
            camera_serial: row.get(28)?,
            lens: row.get(8)?,
            iso: to_u32(row.get(9)?, "iso")?,
            aperture: row.get(10)?,
            shutter_s: row.get(11)?,
            focal_mm: row.get(12)?,
            width: to_u32(row.get(13)?, "width")?,
            height: to_u32(row.get(14)?, "height")?,
            orientation: u16::try_from(orientation)
                .ok()
                .and_then(Orientation::from_exif)
                .ok_or_else(|| {
                    CatalogError::Corrupt(format!("向きの値が不正です: {orientation}"))
                })?,
            gps,
            assigned_gps,
            caption: row.get(18)?,
            created_at: parse_db_utc(&created_at)?,
            video,
        })
    }

    /// variant の asset。
    pub fn asset_of_variant(&self, variant_id: VariantId) -> Result<AssetId> {
        self.conn
            .prepare_cached("SELECT asset_id FROM variant WHERE id = ?1")?
            .query_row([variant_id.get()], |row| row.get(0))
            .optional()?
            .map(AssetId::new)
            .ok_or_else(|| CatalogError::NotFound(format!("variant {variant_id}")))
    }

    /// variant の asset の一覧（重複を除き、asset の ID の順。存在しない variant は飛ばす）。
    pub fn assets_of_variants(&self, variant_ids: &[VariantId]) -> Result<Vec<AssetId>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT asset_id FROM variant
             WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY asset_id",
        )?;
        let rows = stmt.query_map([ids_to_json(variant_ids)], |row| row.get::<_, i64>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(AssetId::new(r?));
        }
        Ok(out)
    }

    /// variant の評価・フラグ・カラーラベル。
    pub fn variant_marks(
        &self,
        variant_id: VariantId,
    ) -> Result<(Rating, Flag, Option<ColorLabel>)> {
        let (rating, flag, label): (i64, i64, Option<String>) = self
            .conn
            .prepare_cached("SELECT rating, flag, color_label FROM variant WHERE id = ?1")?
            .query_row([variant_id.get()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()?
            .ok_or_else(|| CatalogError::NotFound(format!("variant {variant_id}")))?;
        Ok((
            Rating::try_from(rating).map_err(|e| CatalogError::Corrupt(e.to_string()))?,
            Flag::try_from(flag).map_err(|e| CatalogError::Corrupt(e.to_string()))?,
            label.as_deref().map(parse_enum).transpose()?,
        ))
    }

    /// asset・variant・ファイルの件数（計測・表示用）。
    pub fn counts(&self) -> Result<CatalogCounts> {
        let q = |sql: &str| -> Result<u64> {
            let n: i64 = self.conn.query_row(sql, [], |row| row.get(0))?;
            Ok(u64::try_from(n).unwrap_or(0))
        };
        Ok(CatalogCounts {
            assets: q("SELECT count(*) FROM asset")?,
            variants: q("SELECT count(*) FROM variant")?,
            files: q("SELECT count(*) FROM file")?,
            history_entries: q("SELECT count(*) FROM history_entry")?,
        })
    }

    /// すべての variant の id（サムネイル DB の回収用。4.1 節）。
    pub fn all_variant_ids(&self) -> Result<Vec<VariantId>> {
        let mut stmt = self.conn.prepare("SELECT id FROM variant ORDER BY id")?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(VariantId::new(r?));
        }
        Ok(out)
    }
}

/// カタログの件数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CatalogCounts {
    /// asset の数。
    pub assets: u64,
    /// variant の数（仮想コピーを含む）。
    pub variants: u64,
    /// ファイルの数。
    pub files: u64,
    /// 履歴の件数。
    pub history_entries: u64,
}

/// asset のメタデータが空か: 寸法・撮影日時の文字列・カメラ・レンズ・ISO・絞り・シャッター速度・焦点距離・
/// GPS がすべてなく、動画の情報もない（読めずに `error` で登録したまま。implementation_status の No.45）。
///
/// 読めたファイルは寸法が必ずある（写真はデコーダ、動画は ffprobe の値）ので、読めたファイルを空と
/// みなすことはない。asset がなければ [`CatalogError::NotFound`]。
pub(crate) fn asset_metadata_is_empty(conn: &rusqlite::Connection, asset_id: i64) -> Result<bool> {
    conn.prepare_cached(
        "SELECT width IS NULL AND height IS NULL AND captured_at_raw IS NULL AND camera IS NULL
                AND lens IS NULL AND iso IS NULL AND aperture IS NULL AND shutter IS NULL
                AND focal IS NULL AND gps_lat IS NULL AND gps_lon IS NULL
                AND NOT EXISTS (SELECT 1 FROM video_meta v WHERE v.asset_id = asset.id)
         FROM asset WHERE id = ?1",
    )?
    .query_row([asset_id], |row| row.get::<_, bool>(0))
    .optional()?
    .ok_or_else(|| CatalogError::NotFound(format!("asset {asset_id}")))
}
