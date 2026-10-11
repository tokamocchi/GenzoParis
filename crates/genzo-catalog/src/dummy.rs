//! 性能の確認用のダミーデータ（PoC-6。docs/05_poc_and_roadmap.md の PoC-6、SCL-01）。
//!
//! 通常の登録の経路（[`Catalog::register_batch`]）でダミーの asset を大量に登録し、評価・フラグ・
//! ラベル・キャプション・キーワード・仮想コピー・現像の履歴を、実際の使い方に近い割合で付ける。
//! 乱数は種（`seed`）から決まるので、同じ設定なら同じデータになる。
//!
//! 割合などの既定値はすべて仮置き（実際のカタログの統計がないため）。PoC-6 で見直す。

use chrono::{Duration, TimeZone, Utc};
use genzo_model::{
    AssetKind, CaptureTime, ColorLabel, DevelopSettings, PhotoMetadata, TzSource, VideoMetadata,
};
use rusqlite::params;

use crate::catalog::Catalog;
use crate::develop::{encode, save_develop_tx};
use crate::error::{CatalogError, Result};
use crate::hash::FileFacts;
use crate::keyword::ensure_keyword_tx;
use crate::register::{MediaMetadata, RegisterFile, RegisterStatus, refresh_asset_text};
use crate::util::now_utc_string;

/// ダミーデータの設定。
#[derive(Debug, Clone, PartialEq)]
pub struct DummySpec {
    /// asset の数（PoC-6 では 50 万。SCL-01）。
    pub assets: usize,
    /// 乱数の種。
    pub seed: u64,
    /// 動画の割合。仮置き: 5%。
    pub video_ratio: f64,
    /// RAW に JPEG が付いている写真の割合。仮置き: 20%。
    pub raw_jpeg_pair_ratio: f64,
    /// 仮想コピーを持つ asset の割合。仮置き: 10%（SCL-01 の「variant は asset の 1.1 倍程度」）。
    pub virtual_copy_ratio: f64,
    /// 現像した variant の割合。仮置き: 30%。
    pub edited_ratio: f64,
    /// 現像した variant の履歴の件数（「読み込み」を除く）。仮置き: 5 件。
    pub history_per_edited: usize,
    /// 撮影日時がない asset の割合。仮置き: 1%。
    pub no_capture_time_ratio: f64,
    /// 直前の写真と同じ撮影日時にする（連写）割合。仮置き: 10%。
    pub burst_ratio: f64,
    /// キャプションを付ける割合。仮置き: 20%。
    pub caption_ratio: f64,
    /// キーワードを付ける割合。仮置き: 30%。
    pub keyword_ratio: f64,
    /// 1 フォルダあたりの asset の数。仮置き: 500。
    pub assets_per_folder: usize,
    /// 1 トランザクションで登録する数。仮置き: 5000。
    pub batch_size: usize,
}

impl Default for DummySpec {
    fn default() -> Self {
        Self {
            assets: 1000,
            seed: 1,
            video_ratio: 0.05,
            raw_jpeg_pair_ratio: 0.2,
            virtual_copy_ratio: 0.1,
            edited_ratio: 0.3,
            history_per_edited: 5,
            no_capture_time_ratio: 0.01,
            burst_ratio: 0.1,
            caption_ratio: 0.2,
            keyword_ratio: 0.3,
            assets_per_folder: 500,
            batch_size: 5000,
        }
    }
}

/// 作ったダミーデータの件数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DummyReport {
    /// 新しく登録した asset の数。
    pub assets: usize,
    /// 作った仮想コピーの数。
    pub virtual_copies: usize,
    /// 追加した現像の履歴の数（「読み込み」を除く）。
    pub history_entries: usize,
    /// 付けたキーワードの数。
    pub keyword_tags: usize,
}

/// 種から決まる乱数（SplitMix64）。
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 0 以上 1 未満。
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn chance(&mut self, p: f64) -> bool {
        self.next_f64() < p
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next_u64() % items.len() as u64) as usize]
    }
}

const CAMERAS: &[(&str, &str)] = &[
    ("SONY", "ILCE-7M4"),
    ("FUJIFILM", "X-T5"),
    ("Canon", "Canon EOS R5"),
    ("NIKON CORPORATION", "NIKON Z 6_2"),
];
const LENSES: &[&str] = &[
    "FE 24-70mm F2.8 GM II",
    "FE 50mm F1.2 GM",
    "XF16-55mmF2.8 R LM WR",
    "RF24-105mm F4 L IS USM",
];
const CAPTIONS: &[&str] = &[
    "京都旅行の初日",
    "海の夕焼け",
    "夕焼けと富士山",
    "家族写真",
    "Tokyo night walk",
    "子どもの運動会",
    "桜の京都",
    "北海道の雪景色",
];
const KEYWORDS: &[(&str, &[&str])] = &[
    ("場所", &["京都", "東京", "北海道", "沖縄"]),
    ("人物", &["家族", "友人"]),
    ("イベント", &["旅行", "運動会", "結婚式"]),
];

/// ダミーの asset を登録する（PoC-6 用）。
///
/// 同じボリューム ID（種から決める）に登録するので、同じ設定で 2 回呼んでも件数は増えない
/// （登録の冪等性。2 回目は評価などの付与も行わない）。
pub fn populate_dummy(catalog: &mut Catalog, spec: &DummySpec) -> Result<DummyReport> {
    if spec.batch_size == 0 || spec.assets_per_folder == 0 {
        return Err(CatalogError::InvalidInput(
            "batch_size と assets_per_folder は 1 以上にしてください".to_owned(),
        ));
    }
    // 名前や撮影日時など、登録する内容を決める乱数と、評価などの付与に使う乱数を分ける
    // （2 回目の呼び出しでは付与を行わないため、同じ乱数を使うと登録する内容が変わってしまう）。
    let mut rng = SplitMix64(spec.seed);
    let mut decor = SplitMix64(spec.seed ^ 0xD1B5_4A32_D192_ED03);
    let volume =
        catalog.ensure_volume(&format!("dummy-volume-{}", spec.seed), Some("dummy"), None)?;

    // キーワードの階層を作る。
    let mut keyword_ids = Vec::new();
    {
        let tx = catalog.conn.transaction()?;
        for (parent, children) in KEYWORDS {
            let p = ensure_keyword_tx(&tx, None, parent)?;
            for c in *children {
                keyword_ids.push(ensure_keyword_tx(&tx, Some(p), c)?);
            }
        }
        tx.commit()?;
    }

    let base = Utc
        .with_ymd_and_hms(2015, 1, 1, 0, 0, 0)
        .single()
        .expect("固定の日時は有効");
    // 10 年に asset を均等に散らす。
    let step_s = (10 * 365 * 24 * 3600) / (spec.assets.max(1) as i64);
    let mut report = DummyReport::default();
    let mut previous_time: Option<chrono::DateTime<Utc>> = None;
    let mut folders = std::collections::HashMap::new();
    let mut i = 0usize;
    while i < spec.assets {
        let end = (i + spec.batch_size).min(spec.assets);
        let mut requests = Vec::with_capacity((end - i) * 2);
        for n in i..end {
            let folder_index = n / spec.assets_per_folder;
            let folder = match folders.get(&folder_index) {
                Some(&f) => f,
                None => {
                    let year = 2015 + (folder_index % 10);
                    let f = catalog
                        .ensure_folder(volume, &format!("dummy/{year}/{folder_index:05}"))?;
                    folders.insert(folder_index, f);
                    f
                }
            };
            let is_video = rng.chance(spec.video_ratio);
            let time = if rng.chance(spec.no_capture_time_ratio) {
                None
            } else if rng.chance(spec.burst_ratio) && previous_time.is_some() {
                previous_time
            } else {
                let jitter = (rng.next_u64() % 3600) as i64;
                Some(base + Duration::seconds(step_s * n as i64 + jitter))
            };
            if time.is_some() {
                previous_time = time;
            }
            let capture = match time {
                Some(t) => CaptureTime {
                    raw: Some(t.format("%Y:%m:%d %H:%M:%S").to_string()),
                    offset: Some("+00:00".to_owned()),
                    tz_source: TzSource::Exif,
                    tz_assumed: Some("+00:00".to_owned()),
                    correction_s: 0,
                    utc: Some(t),
                },
                None => CaptureTime::unknown(),
            };
            let facts = |name: &str| FileFacts {
                size: 20_000_000 + (n as u64 % 1000) * 1000,
                mtime_ns: 1_700_000_000_000_000_000 + n as i64,
                quick_hash: blake3::hash(format!("dummy:{}:{name}", spec.seed).as_bytes())
                    .to_hex()
                    .to_string(),
            };
            if is_video {
                let name = format!("DJI_{n:07}.MP4");
                requests.push(RegisterFile {
                    folder_id: folder,
                    facts: facts(&name),
                    name,
                    kind: AssetKind::Video,
                    metadata: MediaMetadata::Video(VideoMetadata {
                        duration_s: Some(10.0 + (n % 60) as f64),
                        fps: Some(59.94),
                        codec: Some("hevc".to_owned()),
                        bit_depth: Some(10),
                        color_transfer: Some("arib-std-b67".to_owned()),
                        color_primaries: Some("bt2020".to_owned()),
                        width: Some(3840),
                        height: Some(2160),
                        creation_time: capture.raw.clone(),
                    }),
                    capture,
                    error: None,
                });
            } else {
                let (make, model) = *rng.pick(CAMERAS);
                let meta = PhotoMetadata {
                    make: Some(make.to_owned()),
                    model: Some(model.to_owned()),
                    lens: Some((*rng.pick(LENSES)).to_owned()),
                    iso: Some(100 << (rng.next_u64() % 6)),
                    aperture: Some(2.8),
                    shutter_s: Some(1.0 / 250.0),
                    focal_mm: Some(35.0),
                    width: Some(7008),
                    height: Some(4672),
                    ..Default::default()
                };
                let name = format!("DSC{n:07}.ARW");
                if rng.chance(spec.raw_jpeg_pair_ratio) {
                    let jpg = format!("DSC{n:07}.JPG");
                    requests.push(RegisterFile::photo(
                        folder,
                        jpg.clone(),
                        facts(&jpg),
                        meta.clone(),
                        capture.clone(),
                    ));
                }
                requests.push(RegisterFile::photo(
                    folder,
                    name.clone(),
                    facts(&name),
                    meta,
                    capture,
                ));
            }
        }
        let outcomes = catalog.register_batch(&requests)?;

        // 評価・フラグ・ラベル・キャプション・キーワード・仮想コピー・現像の履歴を付ける。
        let now = now_utc_string();
        let tx = catalog.conn.transaction()?;
        for o in outcomes
            .iter()
            .filter(|o| o.status == RegisterStatus::Added && !o.paired)
        {
            report.assets += 1;
            let rating = match decor.next_u64() % 10 {
                0..=4 => 0,
                5..=6 => 1,
                7 => 2,
                8 => 3,
                _ => 4 + (decor.next_u64() % 2) as i64,
            };
            let flag = match decor.next_u64() % 10 {
                0 => -1,
                1..=2 => 1,
                _ => 0,
            };
            let label = if decor.chance(0.1) {
                Some(decor.pick(ColorLabel::ALL).as_str())
            } else {
                None
            };
            tx.prepare_cached(
                "UPDATE variant SET rating = ?2, flag = ?3, color_label = ?4 WHERE id = ?1",
            )?
            .execute(params![o.master_variant_id.get(), rating, flag, label])?;
            if decor.chance(spec.caption_ratio) {
                tx.prepare_cached("UPDATE asset SET caption = ?2 WHERE id = ?1")?
                    .execute(params![o.asset_id.get(), *decor.pick(CAPTIONS)])?;
                refresh_asset_text(&tx, o.asset_id.get())?;
            }
            if decor.chance(spec.keyword_ratio) {
                let kw = *decor.pick(&keyword_ids);
                tx.prepare_cached(
                    "INSERT OR IGNORE INTO variant_keyword(variant_id, keyword_id) VALUES (?1, ?2)",
                )?
                .execute(params![o.master_variant_id.get(), kw.get()])?;
                report.keyword_tags += 1;
            }
            let mut variants = vec![o.master_variant_id];
            if decor.chance(spec.virtual_copy_ratio) {
                let develop = crate::develop::new_variant_develop();
                let id = crate::develop::insert_variant(
                    &tx,
                    o.asset_id.get(),
                    false,
                    Some("コピー 1"),
                    &develop,
                    crate::develop::HISTORY_LABEL_VIRTUAL_COPY,
                    &now,
                )?;
                variants.push(genzo_model::VariantId::new(id));
                report.virtual_copies += 1;
            }
            for v in variants {
                if !decor.chance(spec.edited_ratio) {
                    continue;
                }
                for h in 0..spec.history_per_edited {
                    let settings = DevelopSettings {
                        exposure_ev: ((decor.next_u64() % 41) as f32 - 20.0) / 10.0,
                        contrast: (h as f32) * 5.0,
                        ..Default::default()
                    };
                    let develop = encode(&settings)?;
                    save_develop_tx(&tx, v, &develop, "露光量", &now)?;
                    report.history_entries += 1;
                }
            }
        }
        tx.commit()?;
        i = end;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_is_deterministic_and_in_range() {
        let mut a = SplitMix64(42);
        let mut b = SplitMix64(42);
        for _ in 0..100 {
            let x = a.next_f64();
            assert_eq!(x, b.next_f64());
            assert!((0.0..1.0).contains(&x));
        }
        // 公表されている SplitMix64 の値（種 0 の最初の出力。Vigna の参照実装）。
        assert_eq!(SplitMix64(0).next_u64(), 0xE220_A839_7B1D_CDAF);
    }
}
