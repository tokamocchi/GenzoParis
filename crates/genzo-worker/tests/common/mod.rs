//! 結合テストの共通の道具（実際に `genzo-worker` のバイナリを起動する）。

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use genzo_media::RgbImage8;
use genzo_media::jpeg::encode_jpeg;
use genzo_worker::{JobTimeouts, WorkerConfig};

/// テストでのジョブのタイムアウト（タイムアウトを確かめるテスト以外）。
///
/// テストの結果は応答の内容で決め、時間では決めない。既定値（テスト用の口は 10 秒など）のままだと、
/// 負荷の高い CI（デバッグ版、ウイルス対策ソフトの検査、macOS のクラッシュレポートの作成で
/// 異常終了したワーカーの終了が遅れる場合など）で、異常終了がタイムアウトとして報告されて
/// 失敗しうるため、十分に長くする。
pub const TEST_JOB_TIMEOUT: Duration = Duration::from_secs(60);

/// テスト用のワーカーの設定（テスト用の口とチェックサムを有効にし、共有メモリは `shm_root` に置く）。
pub fn config(shm_root: &Path) -> WorkerConfig {
    WorkerConfig {
        executable: Some(PathBuf::from(env!("CARGO_BIN_EXE_genzo-worker"))),
        timeouts: JobTimeouts::uniform(TEST_JOB_TIMEOUT),
        // CI の遅い環境でも握手が間に合う長さ。
        startup_timeout: Duration::from_secs(60),
        test_hooks: true,
        verify_checksum: true,
        shm_root: Some(shm_root.to_path_buf()),
        ..WorkerConfig::default()
    }
}

/// `shm_root` の下にある共有メモリのファイルの数（一時ディレクトリの中も数える）。
pub fn shm_files_under(shm_root: &Path) -> usize {
    let mut n = 0;
    for dir in std::fs::read_dir(shm_root).unwrap() {
        let dir = dir.unwrap();
        if dir.file_type().unwrap().is_dir() {
            // 持ち主のロックファイル（genzo_worker::shm::ARENA_OWNER_LOCK）は数えない。
            n += std::fs::read_dir(dir.path())
                .unwrap()
                .filter(|e| e.as_ref().unwrap().file_name() != genzo_worker::shm::ARENA_OWNER_LOCK)
                .count();
        }
    }
    n
}

/// 一様な色の JPEG を書く。`exif` は Exif の TIFF の構造（任意）。
pub fn write_jpeg(
    dir: &Path,
    name: &str,
    (w, h): (u32, u32),
    color: impl Fn(u32, u32) -> [u8; 3],
    exif: Option<&[u8]>,
) -> PathBuf {
    let img = RgbImage8::from_fn(w, h, color).unwrap();
    let bytes = encode_jpeg(&img, 100, None, exif).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// 向き（Orientation）だけを持つ Exif の TIFF の構造（ビッグエンディアン）。
pub fn exif_orientation(value: u16) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"MM\0*");
    v.extend_from_slice(&8u32.to_be_bytes());
    v.extend_from_slice(&1u16.to_be_bytes());
    // タグ 0x0112（Orientation）、型 3（SHORT）、個数 1、値（左詰め）。
    v.extend_from_slice(&0x0112u16.to_be_bytes());
    v.extend_from_slice(&3u16.to_be_bytes());
    v.extend_from_slice(&1u32.to_be_bytes());
    v.extend_from_slice(&value.to_be_bytes());
    v.extend_from_slice(&[0, 0]);
    // 次の IFD はない。
    v.extend_from_slice(&0u32.to_be_bytes());
    v
}

/// 条件が成り立つまで待つ（`limit` を過ぎたら失敗）。時間は上限にだけ使い、結果は条件で決める。
pub fn wait_until(limit: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !cond() {
        assert!(
            Instant::now() < deadline,
            "条件が {limit:?} 以内に成り立たなかった"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// ファイルの内容（DATA-01 の確認用）。
pub fn contents(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}
