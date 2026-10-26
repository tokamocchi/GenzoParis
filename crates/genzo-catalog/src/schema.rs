//! 接続の設定と、番号付きのマイグレーション（docs/04_architecture.md の 3.4 節・DATA-08）。
//!
//! - スキーマの版は `PRAGMA user_version` に、GenzoParis の DB であることの印は
//!   `PRAGMA application_id` に保存する（どちらもマイグレーションと同じトランザクションで書く）。
//! - 既存の DB を新しいスキーマへ移行する前に、`VACUUM INTO` で自動的にバックアップを作る。
//! - DB の版がアプリより新しい場合はエラーにする（古いアプリで新しいカタログを壊さないため）。
//! - マイグレーションは 1 版ずつ 1 つのトランザクションで適用する。テーブルの作り直しに備えて、
//!   適用中は外部キーの確認を止め、各版のコミット前に `PRAGMA foreign_key_check` で確認する
//!   （SQLite の説明書の ALTER TABLE の「12 の手順」と同じ考え方）。
//! - ビュー（版 5 の公開ビュー。ORG-07）は、各版の SQL の前に外し、後で同じ定義で作り直す
//!   （[`apply_migrations`] の doc）。「12 の手順」で表を作り直すと、その表を参照するビューがあれば
//!   `ALTER TABLE ... RENAME` が「error in view ...: no such table」で失敗するため。
//! - 構造の一覧（[`describe_schema`]）は、`genzo catalog schema`・画面の表示と、文書
//!   （docs/catalog_schema.md）が実際のスキーマと食い違わないことのテストに使う。

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, Transaction};

use crate::backup::vacuum_into_file;
use crate::error::{CatalogError, Result};
use crate::util::timestamp_for_filename;

/// カタログ（catalog.db）の `PRAGMA application_id`（ASCII の "GZCT"）。
pub const CATALOG_APPLICATION_ID: i32 = 0x475A_4354;

/// サムネイル DB（thumbs.db）の `PRAGMA application_id`（ASCII の "GZTH"）。
pub const THUMBS_APPLICATION_ID: i32 = 0x475A_5448;

/// 書き込みの競合で待つ時間（`busy_timeout`）。
///
/// 仮置き: 書き込みは DB 書き込みスレッドに集め（04 の 1.3 節）、読み取りは WAL で待たないため、
/// 待つのはチェックポイントやバックアップとの競合だけの見込み。PoC-6 で見直す。
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 接続ごとのページキャッシュの大きさ（KiB。`PRAGMA cache_size` に負の値で渡す）。
///
/// 仮置き: 64MiB。メモリの予算（通常時 4GB。SCL-05）に対して小さく、50 万件の索引の
/// 主要な部分が載る程度として置いた。PoC-6 で計測して見直す。
pub const CACHE_SIZE_KIB: i64 = 64 * 1024;

/// 1 つのマイグレーション（版 `version` へ上げる手順）。
#[derive(Clone, Copy)]
pub(crate) struct Migration {
    /// 適用後の版（1 から順に 1 ずつ増やす）。
    pub version: u32,
    /// 内容の説明（ログ・テスト用）。
    #[allow(dead_code)]
    pub description: &'static str,
    /// 実行する SQL（複数の文を含んでよい）。
    pub sql: &'static str,
    /// SQL の後に実行する Rust の処理（正規化した列の作り直しなど。SQL だけで書けない手順用）。
    pub post: Option<fn(&Transaction<'_>) -> Result<()>>,
}

/// カタログのマイグレーションの列。新しい版は末尾に追加し、既存の版の内容は変えない。
pub(crate) const CATALOG_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "初期スキーマ（04 の 3.1 節・3.5 節・3.6 節・6.4 節）",
        sql: include_str!("sql/catalog_0001_initial.sql"),
        post: None,
    },
    Migration {
        version: 2,
        description: "外部キーの子の列の索引（variant.history_pos・stack.top_variant_id）",
        sql: include_str!("sql/catalog_0002_fk_child_indexes.sql"),
        post: None,
    },
    Migration {
        version: 3,
        description: "ファイル名（file.name_key）だけの索引（implementation_status の No.11・27）",
        sql: include_str!("sql/catalog_0003_file_name_key_index.sql"),
        post: None,
    },
    Migration {
        version: UTC_ONLY_SCHEMA_VERSION,
        description: "tz_source に utc_only を足す（asset の作り直し。implementation_status の No.32）",
        sql: include_str!("sql/catalog_0004_tz_source_utc_only.sql"),
        post: None,
    },
    Migration {
        version: PUBLIC_VIEWS_SCHEMA_VERSION,
        description: "外部から読むための公開ビュー（photos_v1・photo_keywords_v1。ORG-07）",
        sql: include_str!("sql/catalog_0005_public_views.sql"),
        post: None,
    },
    Migration {
        version: IPTC_SCHEMA_VERSION,
        description: "説明のメタデータ（asset_iptc。LIB-15）と photos_v1 のタイトル・作成者・著作権表示の列",
        sql: include_str!("sql/catalog_0006_asset_iptc.sql"),
        post: None,
    },
    Migration {
        version: COLLECTIONS_SCHEMA_VERSION,
        description: "キーワードの書き出しの印・コレクションの名前の比較キーと重複の禁止・公開ビュー photo_collections_v1（LIB-09〜11）",
        sql: include_str!("sql/catalog_0007_collections.sql"),
        post: Some(migrate_collection_names),
    },
    Migration {
        version: STABLE_IDS_SCHEMA_VERSION,
        description: "フォルダ・キーワード・コレクションの ID を使い回さない（AUTOINCREMENT。3 つの表の作り直し）",
        sql: include_str!("sql/catalog_0008_stable_ids.sql"),
        post: Some(migrate_id_sequences),
    },
    Migration {
        version: STACKS_SCHEMA_VERSION,
        description: "スタック（LIB-12）: stack の作り直し（AUTOINCREMENT・折りたたみの状態）・抜けた後の整えのトリガー・公開ビュー photo_stacks_v1",
        sql: include_str!("sql/catalog_0009_stacks.sql"),
        post: Some(migrate_stacks),
    },
    Migration {
        version: CONTENT_IDENTITY_SCHEMA_VERSION,
        description: "内容による同一性（ORG-03・IMP-04・IMP-06）: クイックハッシュ・全体のハッシュの索引と公開ビュー photo_files_v1",
        sql: include_str!("sql/catalog_0010_content_identity.sql"),
        post: None,
    },
    Migration {
        version: CAPTURE_TIME_EDITS_SCHEMA_VERSION,
        description: "撮影日時の一括修正（LIB-16）: 機材のシリアル番号・UTC の計算に使ったオフセットの列、修正の履歴の表、photos_v1 の camera_serial",
        sql: include_str!("sql/catalog_0011_capture_time_edits.sql"),
        post: None,
    },
    Migration {
        version: VOLUMES_SCHEMA_VERSION,
        description: "外付けドライブの識別・見つからない写真（FILE-03・FILE-04）: ボリュームのファイルシステムの種類と最後につながった日時、フォルダの状態、状態が ok でないファイルの索引",
        sql: include_str!("sql/catalog_0012_volumes_and_missing.sql"),
        post: None,
    },
    Migration {
        version: LOCATIONS_SCHEMA_VERSION,
        description: "位置情報の後付け（LIB-21）: 後から付けた位置（asset_location）、位置の変更の履歴の表、photos_v1 の使う位置の列（location_lat・location_lon・location_source）",
        sql: include_str!("sql/catalog_0013_asset_locations.sql"),
        post: None,
    },
    Migration {
        version: AI_MASKS_SCHEMA_VERSION,
        description: "AI マスク（DEV-20）: 推論で作った重みのラスター（ai_mask。内容の SHA-256 で現像設定から参照する）",
        sql: include_str!("sql/catalog_0014_ai_masks.sql"),
        post: None,
    },
];

/// AI マスク（DEV-20）を使い始めたカタログのスキーマの版（推論で作った重みのラスター `ai_mask`。現像設定の
/// `MaskShape::Ai` が内容の SHA-256 で参照する。asset を消しても残り、asset_id を NULL にする）。既存の設定・表は変えない。
pub const AI_MASKS_SCHEMA_VERSION: u32 = 14;

/// 位置情報の後付け（LIB-21）を使い始めたカタログのスキーマの版（後から付けた位置 `asset_location`、変更の
/// 履歴 `location_edit`・`location_edit_item`、公開ビュー `photos_v1` の `location_lat`・`location_lon`・
/// `location_source`）。
///
/// 既存の asset には行を作らない（付けた位置がないのと同じ）。`asset.gps_lat`・`gps_lon` はカメラ（ファイル）の
/// 値のまま変えない。
pub const LOCATIONS_SCHEMA_VERSION: u32 = 13;

/// 外付けドライブの識別（FILE-03）・見つからない写真の検出（FILE-04）を使い始めたカタログのスキーマの版
/// （`volume.fs_type`・`volume.last_seen_at`、`folder.status`・`folder.status_at`、索引 `file_not_ok`）。
///
/// 版 11 までのボリュームの ID（パスの先頭から作った `path:` の形）は移行では変えない（genzo-api がカタログを
/// 開いたときに、見えるフォルダから OS のボリューム ID へ移す）。
pub const VOLUMES_SCHEMA_VERSION: u32 = 12;

/// 撮影日時の一括修正（LIB-16）を使い始めたカタログのスキーマの版（`asset.camera_serial`・`asset.tz_basis`、
/// 修正の履歴 `capture_time_edit`・`capture_time_edit_item`、公開ビュー `photos_v1` の `camera_serial`）。
///
/// 既存の asset の 2 つの列は NULL のまま（`tz_basis` が NULL なら撮影日時の計算は版 10 と同じ。シリアル番号は
/// ファイルを読み直したときに入る）。
pub const CAPTURE_TIME_EDITS_SCHEMA_VERSION: u32 = 11;

/// 内容による同一性（ORG-03）・重複の検出（IMP-04）を使い始めたカタログのスキーマの版（索引
/// `file_quick_hash`・`file_full_hash` と、公開ビュー `photo_files_v1`）。
///
/// 既存のファイルの全体のハッシュ（`file.full_hash`）は、移行では計算しない（ファイルを読むため）。移行の後に
/// genzo-api がバックグラウンドのジョブで計算する。
pub const CONTENT_IDENTITY_SCHEMA_VERSION: u32 = 10;

/// スタック（LIB-12）を使い始めたカタログのスキーマの版（`stack` の作り直しと `collapsed`、トリガー
/// `stack_member_after_delete`、公開ビュー `photo_stacks_v1`）。
pub const STACKS_SCHEMA_VERSION: u32 = 9;

/// 版 9 の SQL の後の処理: 既存のスタック（版 8 まではアプリが作らないが、外部のツールで作られた行がありうる）を
/// アプリの約束にそろえる。写真が 1 枚以下のスタックは消し、残りは一番上の写真（`top_variant_id`。メンバーで
/// なければ位置の最初のもの）を先頭に、`(position, variant_id)` の順で位置を 0 から付け直す。
///
/// マイグレーションの間は外部キーの確認を止めているため、`ON DELETE CASCADE` が働かない。スタックを消すときは、
/// 先にメンバーの行を消す。
fn migrate_stacks(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "DELETE FROM stack_member WHERE stack_id IN (
             SELECT stack_id FROM stack_member GROUP BY stack_id HAVING count(*) < 2);
         DELETE FROM stack WHERE NOT EXISTS (SELECT 1 FROM stack_member m WHERE m.stack_id = stack.id);",
    )?;
    let stacks: Vec<(i64, Option<i64>)> = {
        let mut stmt = tx.prepare("SELECT id, top_variant_id FROM stack ORDER BY id")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut members_of = tx.prepare(
        "SELECT variant_id FROM stack_member WHERE stack_id = ?1 ORDER BY position, variant_id",
    )?;
    let mut set_position = tx
        .prepare("UPDATE stack_member SET position = ?3 WHERE stack_id = ?1 AND variant_id = ?2")?;
    let mut set_top = tx.prepare("UPDATE stack SET top_variant_id = ?2 WHERE id = ?1")?;
    for (stack, top) in stacks {
        let mut members: Vec<i64> = members_of
            .query_map([stack], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if let Some(i) = top.and_then(|t| members.iter().position(|&m| m == t)) {
            let t = members.remove(i);
            members.insert(0, t);
        }
        for (pos, &v) in members.iter().enumerate() {
            set_position.execute(rusqlite::params![stack, v, pos as i64])?;
        }
        set_top.execute(rusqlite::params![stack, members[0]])?;
    }
    Ok(())
}

/// フォルダ・キーワード・コレクションの ID を、削除した後に使い回さない（`AUTOINCREMENT`）ようにした
/// カタログのスキーマの版。
///
/// 版 7 までは、最大の ID の行を削除すると次に作る行が同じ ID になり、スマートコレクションの条件
/// （`keyword_id`・`collection_id`・`folder_id`）や画面のターゲットコレクションが、削除したものではなく
/// 後から作ったものを指していた。
pub const STABLE_IDS_SCHEMA_VERSION: u32 = 8;

/// 版 8 の SQL の後の処理: スマートコレクションの条件（`smart_rule_json`）が参照する ID のうち、残っている
/// 行の最大より大きいもの（版 7 までに削除したもの）があれば、その値まで連番（`sqlite_sequence`）を上げる
/// （次に作るものがその ID にならないように）。条件の JSON は、読めない（アプリより新しい版などの）ものも
/// 含めて、キー `keyword_id`・`collection_id`・`folder_id` の整数をすべて拾う。
fn migrate_id_sequences(tx: &Transaction<'_>) -> Result<()> {
    let jsons: Vec<String> = {
        let mut stmt =
            tx.prepare("SELECT smart_rule_json FROM collection WHERE smart_rule_json IS NOT NULL")?;
        stmt.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut max_refs = IdRefs::default();
    for json in jsons {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
            max_refs.collect(&value, 0);
        }
    }
    for (table, max) in [
        ("folder", max_refs.folder),
        ("keyword", max_refs.keyword),
        ("collection", max_refs.collection),
    ] {
        if max > 0 {
            tx.execute(
                "UPDATE sqlite_sequence SET seq = max(seq, ?2) WHERE name = ?1",
                rusqlite::params![table, max],
            )?;
        }
    }
    Ok(())
}

/// スマートコレクションの条件の JSON が参照する ID の最大（[`migrate_id_sequences`]）。
#[derive(Debug, Default, PartialEq, Eq)]
struct IdRefs {
    folder: i64,
    keyword: i64,
    collection: i64,
}

impl IdRefs {
    /// JSON の木をたどって、キー `folder_id`・`keyword_id`・`collection_id` の整数の最大を覚える
    /// （深さは条件の入れ子の上限より十分大きい 64 段まで）。
    fn collect(&mut self, value: &serde_json::Value, depth: usize) {
        if depth > 64 {
            return;
        }
        match value {
            serde_json::Value::Object(map) => {
                for (key, v) in map {
                    let slot = match key.as_str() {
                        "folder_id" => Some(&mut self.folder),
                        "keyword_id" => Some(&mut self.keyword),
                        "collection_id" => Some(&mut self.collection),
                        _ => None,
                    };
                    if let (Some(slot), Some(id)) = (slot, v.as_i64()) {
                        *slot = (*slot).max(id);
                    }
                    self.collect(v, depth + 1);
                }
            }
            serde_json::Value::Array(items) => {
                for v in items {
                    self.collect(v, depth + 1);
                }
            }
            _ => {}
        }
    }
}

/// 階層キーワードの書き出しの印（`keyword.include_on_export`）と、コレクションの名前の比較キー
/// （`collection.name_key`・索引 `collection_unique_name`）、公開ビュー `photo_collections_v1` を足した
/// カタログのスキーマの版（LIB-09〜11）。
pub const COLLECTIONS_SCHEMA_VERSION: u32 = 7;

/// 版 7 の SQL の後の処理: 既存のコレクションの `name_key` を求め（NFKC ＋ 小文字化。SQL では求められない）、
/// 同じ親の下で同じ名前のものがあれば後のもの（id の大きいもの）の名前に「 (2)」などを足してから、重複を
/// 禁止する索引を作る（版 6 までは名前の重複を確かめていなかった。アプリは版 6 までコレクションを作らないが、
/// 外部のツールで作った行があっても移行を失敗させないため）。
fn migrate_collection_names(tx: &Transaction<'_>) -> Result<()> {
    use std::collections::HashSet;

    let rows: Vec<(i64, Option<i64>, String)> = {
        let mut stmt = tx.prepare("SELECT id, parent_id, name FROM collection ORDER BY id")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut taken: HashSet<(i64, String)> = HashSet::new();
    let mut update = tx.prepare("UPDATE collection SET name = ?2, name_key = ?3 WHERE id = ?1")?;
    for (id, parent, name) in rows {
        let parent = parent.unwrap_or(0);
        let base = if name.trim().is_empty() {
            "コレクション".to_owned()
        } else {
            name.trim().to_owned()
        };
        let mut candidate = base.clone();
        let mut n = 2u32;
        while !taken.insert((parent, crate::text::search_key(&candidate))) {
            candidate = format!("{base} ({n})");
            n += 1;
        }
        let key = crate::text::search_key(&candidate);
        update.execute(rusqlite::params![id, candidate, key])?;
    }
    drop(update);
    tx.execute_batch(
        "CREATE UNIQUE INDEX collection_unique_name ON collection(ifnull(parent_id, 0), name_key);",
    )?;
    Ok(())
}

/// 説明のメタデータ（`asset_iptc`。LIB-15）を足したカタログのスキーマの版。
///
/// 既存の asset には行を作らない（値がないのと同じ）。テキスト検索の索引（`asset_text`）は、値がない間は
/// 変わらないので作り直さない。
pub const IPTC_SCHEMA_VERSION: u32 = 6;

/// 公開ビュー（[`PUBLIC_VIEWS`]）を足したカタログのスキーマの版。
pub const PUBLIC_VIEWS_SCHEMA_VERSION: u32 = 5;

/// 外部から SQL で読むための公開ビュー（ORG-07。docs/catalog_schema.md の「公開ビュー」）の名前。
///
/// 名前の末尾の版（`_v1`）の約束: 列の名前・意味・値の形を変えず、列を減らさない（変えるときは `_v2` を
/// 足す）。列を足すことはある。
pub const PUBLIC_VIEWS: &[&str] = &[
    "photos_v1",
    "photo_keywords_v1",
    "photo_collections_v1",
    "photo_stacks_v1",
    "photo_files_v1",
];

/// `tz_source = 'utc_only'`（UTC の印 `Z` だけの撮影日時）を足したカタログのスキーマの版。
///
/// これより前の版から移行したカタログでは、移行で `utc_only` にした asset の表示のオフセット
/// （`tz_assumed`）が `+00:00` のまま（カタログはアプリの既定のタイムゾーンを知らないため）。
/// そのため移行は、`utc_only` にした asset があれば、推定し直しが済んでいない印
/// （[`crate::STATE_CAPTURE_TIME_RERESOLVE`]）を同じトランザクションで付ける。カタログを開いた側
/// （genzo-api）は、印があれば既定のタイムゾーンで推定し直し（genzo-model の
/// `CaptureTime::with_default_offset`）、終えたら印を外す（途中でアプリが終わっても、次に開いたときに
/// 推定し直す）。
pub const UTC_ONLY_SCHEMA_VERSION: u32 = 4;

/// サムネイル DB のマイグレーションの列。
pub(crate) const THUMBS_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "サムネイル（L0）とプレビュー（L1）の索引（04 の 4 章・4.1 節）",
        sql: include_str!("sql/thumbs_0001_initial.sql"),
        post: None,
    },
    Migration {
        version: 2,
        description: "キャッシュの持ち主（カタログの世代）と、作り直し待ちの印",
        sql: include_str!("sql/thumbs_0002_owner_and_pending.sql"),
        post: None,
    },
];

/// このアプリのカタログのスキーマの版。
pub const CATALOG_SCHEMA_VERSION: u32 = 14;

/// このアプリのサムネイル DB のスキーマの版。
pub const THUMBS_SCHEMA_VERSION: u32 = 2;

/// DB の種類ごとのスキーマの定義。
#[derive(Clone, Copy)]
pub(crate) struct SchemaSpec<'a> {
    pub application_id: i32,
    pub migrations: &'a [Migration],
    /// 既存の DB を移行する前にバックアップを作るか（サムネイル DB は作り直せるので作らない）。
    pub backup_before_migration: bool,
}

impl SchemaSpec<'_> {
    /// 最新の版。
    pub fn target_version(&self) -> u32 {
        self.migrations.last().map_or(0, |m| m.version)
    }
}

pub(crate) const CATALOG_SPEC: SchemaSpec<'static> = SchemaSpec {
    application_id: CATALOG_APPLICATION_ID,
    migrations: CATALOG_MIGRATIONS,
    backup_before_migration: true,
};

pub(crate) const THUMBS_SPEC: SchemaSpec<'static> = SchemaSpec {
    application_id: THUMBS_APPLICATION_ID,
    migrations: THUMBS_MIGRATIONS,
    backup_before_migration: false,
};

/// 同期の強さ（3.4 節）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Durability {
    /// `synchronous = FULL`（macOS では `fullfsync` も）。カタログ用（DATA-03b）。
    Full,
    /// `synchronous = NORMAL`。作り直せるサムネイル DB 用。
    Normal,
}

/// 接続の設定（3.4 節）。
///
/// WAL、`synchronous`、macOS の `fullfsync`、`foreign_keys = ON`、`busy_timeout`。
/// メモリ上の DB（テスト用）は WAL にできないため、ジャーナルの設定を飛ばす。
pub(crate) fn configure_connection(
    conn: &Connection,
    durability: Durability,
    in_memory: bool,
) -> Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    // SQLite の既定では外部キーの制約が無効なため、すべての接続で有効にする（3.4 節）。
    conn.pragma_update(None, "foreign_keys", true)?;
    if !in_memory {
        let mode: String =
            conn.pragma_update_and_check(None, "journal_mode", "wal", |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(CatalogError::InvalidInput(format!(
                "WAL モードにできません（journal_mode = {mode}）。ネットワーク上のドライブなどでは使えません"
            )));
        }
    }
    let synchronous = match durability {
        Durability::Full => "FULL",
        Durability::Normal => "NORMAL",
    };
    conn.pragma_update(None, "synchronous", synchronous)?;
    // macOS の通常の fsync は、ストレージ装置のキャッシュまで書き込まない場合があるため（3.4 節）。
    #[cfg(target_os = "macos")]
    if durability == Durability::Full {
        conn.pragma_update(None, "fullfsync", true)?;
        conn.pragma_update(None, "checkpoint_fullfsync", true)?;
    }
    conn.pragma_update(None, "cache_size", -CACHE_SIZE_KIB)?;
    conn.set_prepared_statement_cache_capacity(64);
    Ok(())
}

/// 書き込み用の接続のトランザクションを `BEGIN IMMEDIATE` で始めるようにする。
///
/// 既定の `BEGIN DEFERRED` では、読み取りから始めたトランザクションが途中で書き込みに移るとき、
/// その間に別の接続がコミットしていると、`busy_timeout` で待たずに直ちに `SQLITE_BUSY`
/// （`SQLITE_BUSY_SNAPSHOT`）で失敗する（SQLite の説明書「Write-Ahead Logging」の
/// 「Sometimes Queries Return SQLITE_BUSY In WAL Mode」）。登録・削除・履歴の移動などは
/// 読んでから書くので、書き込みのロックを最初に取り、競合したら `busy_timeout` まで待つ。
/// 書き込みは DB 書き込みスレッドに集める設計（04 の 1.3 節）だが、CLI とアプリが同じカタログを
/// 同時に開いた場合などにも、途中で失敗せずに待つようにするため。
pub(crate) fn use_immediate_transactions(conn: &mut Connection) {
    conn.set_transaction_behavior(rusqlite::TransactionBehavior::Immediate);
}

/// 読み取り専用の接続の設定（WAL・同期の設定は書き込み側の接続が行う）。
pub(crate) fn configure_read_only_connection(conn: &Connection) -> Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "cache_size", -CACHE_SIZE_KIB)?;
    conn.set_prepared_statement_cache_capacity(64);
    Ok(())
}

/// `PRAGMA user_version` を読む。
pub(crate) fn user_version(conn: &Connection) -> Result<u32> {
    let v: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    u32::try_from(v).map_err(|_| CatalogError::Corrupt(format!("user_version が不正です: {v}")))
}

/// DB の素性を確かめる（GenzoParis の DB か、版が新しすぎないか）。書き込みはしない。
///
/// 戻り値は現在の版（新しい空の DB なら 0）。
pub(crate) fn check_identity(conn: &Connection, spec: &SchemaSpec<'_>, path: &Path) -> Result<u32> {
    let not_ours = || CatalogError::NotOurDatabase(path.to_path_buf());
    let found = match user_version(conn) {
        Ok(v) => v,
        Err(CatalogError::Sqlite(rusqlite::Error::SqliteFailure(e, _)))
            if e.code == rusqlite::ErrorCode::NotADatabase =>
        {
            return Err(not_ours());
        }
        Err(e) => return Err(e),
    };
    let app_id: i32 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let object_count: i64 =
        conn.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))?;
    if found == 0 {
        // 版 0 でテーブルがあるのは、GenzoParis 以外の DB。
        if object_count > 0 {
            return Err(not_ours());
        }
    } else if app_id != spec.application_id {
        return Err(not_ours());
    }
    let target = spec.target_version();
    if found > target {
        return Err(CatalogError::FutureSchema {
            found,
            supported: target,
        });
    }
    Ok(found)
}

/// マイグレーションの結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MigrationOutcome {
    /// 適用前の版（新しい DB なら 0）。
    pub from: u32,
    /// 適用後の版。
    pub to: u32,
    /// 移行前に作ったバックアップ。
    pub backup: Option<PathBuf>,
}

/// 最新の版までマイグレーションを適用する。
///
/// `db_path` と `backup_dir` は、既存の DB を移行する前のバックアップに使う
/// （`backup_dir` が `None` なら DB と同じフォルダ）。メモリ上の DB（`db_path = None`）は
/// バックアップを作らない。
pub(crate) fn migrate(
    conn: &mut Connection,
    spec: &SchemaSpec<'_>,
    db_path: Option<&Path>,
    backup_dir: Option<&Path>,
) -> Result<MigrationOutcome> {
    validate_migrations(spec.migrations)?;
    let from = user_version(conn)?;
    let to = spec.target_version();
    if from > to {
        return Err(CatalogError::FutureSchema {
            found: from,
            supported: to,
        });
    }
    if from == to {
        return Ok(MigrationOutcome {
            from,
            to,
            backup: None,
        });
    }

    // DATA-08: 既存の DB を移行する前に、自動でバックアップする。
    let backup = match (from > 0 && spec.backup_before_migration, db_path) {
        (true, Some(path)) => {
            let dir = backup_dir
                .map(Path::to_path_buf)
                .or_else(|| path.parent().map(Path::to_path_buf))
                .unwrap_or_default();
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "catalog".to_owned());
            std::fs::create_dir_all(&dir).map_err(|e| CatalogError::io(&dir, e))?;
            let dest = dir.join(format!(
                "{stem}.v{from}-before-migration-{}.db",
                timestamp_for_filename(chrono::Utc::now())
            ));
            vacuum_into_file(conn, &dest)?;
            Some(dest)
        }
        _ => None,
    };

    conn.pragma_update(None, "foreign_keys", false)?;
    let result = apply_migrations(conn, spec, from);
    // 失敗した場合も外部キーの確認を元に戻す。
    let restore = conn.pragma_update(None, "foreign_keys", true);
    result?;
    restore?;
    Ok(MigrationOutcome { from, to, backup })
}

/// 版 `from` より後のマイグレーションを、1 版ずつ 1 つのトランザクションで適用する。
///
/// ビューの扱い（版 5 の公開ビュー。ORG-07）: 各版の SQL を実行する前に、すべてのビューを外し（定義を
/// 覚えておく）、SQL と Rust の処理の後に、まだないものを同じ定義で作り直す。そのうえで、すべてのビューが
/// 使える（`SELECT * FROM <ビュー> LIMIT 0` を準備できる）ことを確かめ、使えなければ移行を失敗にして
/// 戻す。
///
/// - 表の作り直し（「12 の手順」）・列の追加では、ビューのことを考えなくてよい。
/// - ビューの定義を変えるマイグレーションは、`DROP VIEW IF EXISTS <名前>` のあとで新しい定義を
///   `CREATE VIEW` する（実行の前に外してあるので、`IF EXISTS` が要る）。作ったビューは作り直さない。
/// - ビューが参照する列を消す・名前を変えるマイグレーションは、そのビューを新しい定義で作り直さないと、
///   作り直しか確認で失敗する（公開ビューの列は変えない約束。[`PUBLIC_VIEWS`]）。
/// - ビューを廃止する仕組みはない（公開ビューは消さない約束）。
fn apply_migrations(conn: &mut Connection, spec: &SchemaSpec<'_>, from: u32) -> Result<()> {
    for m in spec.migrations.iter().filter(|m| m.version > from) {
        let tx = conn.transaction()?;
        let views = detach_views(&tx)?;
        tx.execute_batch(m.sql)?;
        if let Some(post) = m.post {
            post(&tx)?;
        }
        reattach_views(&tx, &views, m.version)?;
        check_views(&tx, m.version)?;
        let violations: i64 =
            tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        if violations > 0 {
            return Err(CatalogError::Corrupt(format!(
                "版 {} へのマイグレーションの後に外部キーの違反が {violations} 件あります",
                m.version
            )));
        }
        tx.pragma_update(None, "user_version", m.version)?;
        tx.pragma_update(None, "application_id", spec.application_id)?;
        tx.commit()?;
    }
    Ok(())
}

/// スキーマの 1 つのオブジェクト（`sqlite_schema` の行。表・索引・ビュー・トリガー）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaObject {
    /// 種類（`table`・`index`・`view`・`trigger`）。
    pub kind: String,
    /// 名前。
    pub name: String,
    /// 対象の表（索引・トリガーでは対象の表、表・ビューでは自分の名前）。
    pub table: String,
    /// 作った SQL（`CREATE ...`）。
    pub sql: String,
}

/// 表・ビューの列（`PRAGMA table_info` の行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    /// 列の名前。
    pub name: String,
    /// 宣言した型（`INTEGER`・`TEXT` など。ビューの列では空のことがある）。
    pub decl_type: String,
    /// `NOT NULL` か。
    pub not_null: bool,
    /// 既定値（SQL の式。なければ `None`）。
    pub default_value: Option<String>,
    /// 主キーの何番目の列か（1 から。主キーでなければ 0）。
    pub primary_key: u32,
}

/// 表・ビューと、その列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    /// 名前。
    pub name: String,
    /// 種類（`table`・`virtual`（FTS5 などの仮想表）・`view`）。
    pub kind: String,
    /// 列（定義の順）。
    pub columns: Vec<ColumnInfo>,
}

/// DB の構造の一覧（[`describe_schema`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaDescription {
    /// スキーマの版（`PRAGMA user_version`）。
    pub schema_version: u32,
    /// `PRAGMA application_id`（カタログは [`CATALOG_APPLICATION_ID`]）。
    pub application_id: i32,
    /// 表・ビュー（名前の順。SQLite の内部の表と、仮想表の中身を持つ表（シャドウテーブル）は除く）。
    pub tables: Vec<TableInfo>,
    /// 表・索引・ビュー・トリガー（種類・名前の順。SQLite が自動で作る索引と内部の表は除く）。
    pub objects: Vec<SchemaObject>,
}

impl SchemaDescription {
    /// 名前で表・ビューを探す。
    pub fn table(&self, name: &str) -> Option<&TableInfo> {
        self.tables.iter().find(|t| t.name == name)
    }
}

/// DB の構造を調べる（`sqlite_schema`・`PRAGMA table_list`・`PRAGMA table_info`。読むだけ）。
///
/// 除くもの: 名前が `sqlite_` で始まる SQLite の内部の表・自動の索引（`sqlite_sequence`・
/// `sqlite_autoindex_*` など）と、仮想表（FTS5）の中身を持つ表（`PRAGMA table_list` の `shadow`。
/// `asset_fts_data` など）。
pub(crate) fn describe_schema(conn: &Connection) -> Result<SchemaDescription> {
    let schema_version = user_version(conn)?;
    let application_id: i32 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let listed: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT name, type FROM pragma_table_list WHERE schema = 'main'
             AND type IN ('table', 'virtual', 'view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
             ORDER BY name",
        )?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut tables = Vec::with_capacity(listed.len());
    for (name, kind) in listed {
        let mut stmt = conn.prepare(
            "SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info(?1) ORDER BY cid",
        )?;
        let columns = stmt
            .query_map([&name], |row| {
                Ok(ColumnInfo {
                    name: row.get(0)?,
                    decl_type: row.get(1)?,
                    not_null: row.get::<_, i64>(2)? != 0,
                    default_value: row.get(3)?,
                    primary_key: u32::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        tables.push(TableInfo {
            name,
            kind,
            columns,
        });
    }
    let shadow: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT name FROM pragma_table_list WHERE schema = 'main' AND type = 'shadow'",
        )?;
        stmt.query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    let objects = {
        let mut stmt = conn.prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema
             WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
             ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'view' THEN 1 WHEN 'index' THEN 2 ELSE 3 END, name",
        )?;
        stmt.query_map([], |row| {
            Ok(SchemaObject {
                kind: row.get(0)?,
                name: row.get(1)?,
                table: row.get(2)?,
                sql: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|o| !shadow.contains(&o.name))
        .collect()
    };
    Ok(SchemaDescription {
        schema_version,
        application_id,
        tables,
        objects,
    })
}

/// SQL の識別子を二重引用符で囲む（中の `"` は二重にする）。
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// すべてのビューを外し、作った順の（名前, 定義）を返す（[`apply_migrations`]）。
fn detach_views(tx: &Transaction<'_>) -> Result<Vec<(String, String)>> {
    let views: Vec<(String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT name, sql FROM sqlite_schema WHERE type = 'view' AND sql IS NOT NULL ORDER BY rowid",
        )?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    // 後から作ったビュー（先に作ったビューを参照しうる）から外す。
    for (name, _) in views.iter().rev() {
        tx.execute_batch(&format!("DROP VIEW {}", quote_ident(name)))?;
    }
    Ok(views)
}

/// [`detach_views`] で外したビューのうち、マイグレーションが作り直さなかったものを同じ定義で作る。
fn reattach_views(tx: &Transaction<'_>, views: &[(String, String)], version: u32) -> Result<()> {
    for (name, sql) in views {
        let exists: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE name = ?1)",
            [name],
            |row| row.get(0),
        )?;
        if exists {
            continue;
        }
        tx.execute_batch(sql).map_err(|e| {
            CatalogError::Corrupt(format!(
                "版 {version} へのマイグレーションの後に、ビュー {name} を元の定義で作り直せません: {e}\
                 （ビューが参照する列を変えるマイグレーションは、ビューを新しい定義で作り直してください）"
            ))
        })?;
    }
    Ok(())
}

/// すべてのビューが使えることを確かめる（[`apply_migrations`]）。
fn check_views(tx: &Transaction<'_>, version: u32) -> Result<()> {
    let names: Vec<String> = {
        let mut stmt = tx.prepare("SELECT name FROM sqlite_schema WHERE type = 'view'")?;
        stmt.query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for name in names {
        tx.prepare(&format!("SELECT * FROM {} LIMIT 0", quote_ident(&name)))
            .map_err(|e| {
                CatalogError::Corrupt(format!(
                    "版 {version} へのマイグレーションの後に、ビュー {name} を使えません: {e}"
                ))
            })?;
    }
    Ok(())
}

/// マイグレーションの版が 1 から 1 ずつ増えているかを確かめる（定義の誤りを防ぐ）。
pub(crate) fn validate_migrations(migrations: &[Migration]) -> Result<()> {
    for (i, m) in migrations.iter().enumerate() {
        let expected = u32::try_from(i + 1).unwrap_or(u32::MAX);
        if m.version != expected {
            return Err(CatalogError::InvalidInput(format!(
                "マイグレーションの版が連番ではありません（{i} 番目が版 {}）",
                m.version
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_lists_are_consecutive_and_match_constants() {
        validate_migrations(CATALOG_MIGRATIONS).unwrap();
        validate_migrations(THUMBS_MIGRATIONS).unwrap();
        assert_eq!(CATALOG_SPEC.target_version(), CATALOG_SCHEMA_VERSION);
        assert_eq!(THUMBS_SPEC.target_version(), THUMBS_SCHEMA_VERSION);
        let bad = [Migration {
            version: 2,
            description: "",
            sql: "",
            post: None,
        }];
        assert!(validate_migrations(&bad).is_err());
    }

    #[test]
    fn id_refs_collect_the_largest_ids_in_nested_rules() {
        let json = serde_json::json!({
            "version": 99,
            "match": "all",
            "rules": [
                {"type": "keyword", "op": "has", "keyword_id": 3},
                {"type": "group", "rules": [
                    {"type": "keyword", "op": "not_has", "keyword_id": 12},
                    {"type": "collection", "op": "in", "collection_id": 7},
                    {"type": "folder", "op": "in", "folder_id": 5, "include_subfolders": true},
                    {"type": "future", "collection_id": "x"}
                ]}
            ]
        });
        let mut refs = IdRefs::default();
        refs.collect(&json, 0);
        assert_eq!(
            refs,
            IdRefs {
                folder: 5,
                keyword: 12,
                collection: 7
            }
        );
    }

    #[test]
    fn application_ids_are_ascii_tags() {
        assert_eq!(&CATALOG_APPLICATION_ID.to_be_bytes(), b"GZCT");
        assert_eq!(&THUMBS_APPLICATION_ID.to_be_bytes(), b"GZTH");
    }
}
