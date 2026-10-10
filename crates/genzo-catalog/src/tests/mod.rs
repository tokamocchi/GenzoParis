//! カタログの結合テスト（内部の接続にも触れるため、crate の中に置く）。

mod backup;
mod develop;
mod dummy;
mod file_ops;
mod keywords;
mod register;
mod schema;
mod search;
mod text_search;
mod thumbs;

use chrono::FixedOffset;
use genzo_model::{CaptureTime, FolderId, PhotoMetadata, VideoMetadata, VolumeId};
use tempfile::TempDir;

use crate::{Catalog, FileFacts, RegisterFile, RegisterOutcome};

/// 番号から決まる 16 進数 64 文字（クイックハッシュの代わり）。
pub(crate) fn hex(n: u64) -> String {
    blake3::hash(&n.to_le_bytes()).to_hex().to_string()
}

/// 番号から決まるファイルの情報。
pub(crate) fn facts(n: u64) -> FileFacts {
    FileFacts {
        size: 1_000 + n,
        mtime_ns: 1_700_000_000_000_000_000 + n as i64,
        quick_hash: hex(n),
    }
}

/// 日本時間の撮影日時（EXIF の形式 `"2024:05:01 12:00:00"`）。`None` なら撮影日時なし。
pub(crate) fn capture(exif: Option<&str>) -> CaptureTime {
    let jst = FixedOffset::east_opt(9 * 3600).unwrap();
    match exif {
        Some(s) => CaptureTime::resolve(Some(s), Some("+09:00"), jst).unwrap(),
        None => CaptureTime::unknown(),
    }
}

/// 写真のメタデータ。
pub(crate) fn photo_meta(model: &str, lens: &str) -> PhotoMetadata {
    PhotoMetadata {
        make: Some("SONY".to_owned()),
        model: Some(model.to_owned()),
        lens: Some(lens.to_owned()),
        iso: Some(100),
        aperture: Some(2.8),
        shutter_s: Some(0.004),
        focal_mm: Some(35.0),
        width: Some(7008),
        height: Some(4672),
        ..Default::default()
    }
}

/// 写真の登録の要求。
pub(crate) fn photo_req(folder: FolderId, name: &str, n: u64, when: Option<&str>) -> RegisterFile {
    RegisterFile::photo(
        folder,
        name,
        facts(n),
        photo_meta("ILCE-7M4", "FE 24-70mm F2.8 GM II"),
        capture(when),
    )
}

/// 動画の登録の要求。
pub(crate) fn video_req(folder: FolderId, name: &str, n: u64, when: Option<&str>) -> RegisterFile {
    RegisterFile::video(
        folder,
        name,
        facts(n),
        VideoMetadata {
            duration_s: Some(12.5),
            fps: Some(59.94),
            codec: Some("hevc".to_owned()),
            bit_depth: Some(10),
            color_transfer: Some("arib-std-b67".to_owned()),
            color_primaries: Some("bt2020".to_owned()),
            width: Some(3840),
            height: Some(2160),
            creation_time: when.map(str::to_owned),
        },
        capture(when),
    )
}

/// 一時フォルダのカタログと、ボリューム・フォルダ。
pub(crate) struct Fixture {
    pub dir: TempDir,
    pub cat: Catalog,
    pub volume: VolumeId,
    pub folder: FolderId,
}

impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut cat = Catalog::open(dir.path().join("catalog.db")).unwrap();
        let volume = cat
            .ensure_volume("vol-1", Some("内蔵"), Some("/mnt/photos"))
            .unwrap();
        let folder = cat.ensure_folder(volume, "2024/京都").unwrap();
        Self {
            dir,
            cat,
            volume,
            folder,
        }
    }

    pub fn catalog_path(&self) -> std::path::PathBuf {
        self.dir.path().join("catalog.db")
    }

    /// 写真を 1 枚登録する。
    pub fn photo(&mut self, name: &str, n: u64, when: Option<&str>) -> RegisterOutcome {
        self.cat
            .register_file(&photo_req(self.folder, name, n, when))
            .unwrap()
    }

    /// テーブルの行数。
    pub fn rows(&self, table: &str) -> i64 {
        rows(&self.cat, table)
    }
}

/// テーブルの行数。
pub(crate) fn rows(cat: &Catalog, table: &str) -> i64 {
    cat.conn
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn handles_can_move_to_the_db_thread_and_errors_are_thread_safe() {
    // DB 書き込みスレッド・読み取りプールへ渡せること（04 の 1.3 節）。
    fn assert_send<T: Send>() {}
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send::<Catalog>();
    assert_send::<crate::ThumbStore>();
    assert_send::<crate::PreviewCache>();
    assert_send_sync::<crate::CatalogError>();
}
