//! genzo-api の結合テストの共通の道具。
//!
//! - ワーカーは実際のプロセスを使う。本体の実行ファイル自身を隠しサブコマンドで起動する構成
//!   （`genzo-api-worker __worker`。[`WorkerLaunch::SelfSubcommand`]）で起動する。
//! - テストの結果は応答の内容で決め、時間では決めない（待つときは十分に長い上限だけを置く）。
//! - ffmpeg・LibRaw・GPU がない環境では、それを使う部分を飛ばす（失敗にしない）。CI のランナー
//!   （環境変数 `CI`）では GPU を使わない（genzo-gpu のテストと同じ方針。実機と違う実装の差で失敗しうるため）。
//! - 元ファイルのハッシュを操作の前後で比べる（DATA-01）。

#![allow(dead_code)]

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{FixedOffset, NaiveDate};
use genzo_api::{
    Core, CoreConfig, CoreHooks, GpuMode, ImportReport, JobInfo, JobResult, JobState, SearchFilter,
    SearchSort, TrashBackend, VariantSummary, WorkerLaunch,
};
use genzo_media::{ExifData, FfmpegTools, RgbImage8};
use genzo_model::{GpsCoord, OutputColorSpace};
use genzo_worker::JobTimeouts;
use parking_lot::Mutex;

/// ワーカーのジョブのタイムアウト（テスト用。負荷の高い CI でも誤って打ち切らない長さ）。
pub const TEST_JOB_TIMEOUT: Duration = Duration::from_secs(120);

/// ジョブ・描画を待つ時間の上限。
pub const WAIT_LIMIT: Duration = Duration::from_secs(300);

/// 既定のタイムゾーン（日本時間）。
pub const JST_MINUTES: i32 = 9 * 60;

/// CI のランナーか（環境変数 `CI` が空でなく、`0`・`false` でない）。
pub fn on_ci() -> bool {
    std::env::var("CI").is_ok_and(|v| {
        let v = v.trim().to_ascii_lowercase();
        !v.is_empty() && v != "0" && v != "false"
    })
}

/// テストで使う GPU の設定（CI では使わない）。
pub fn test_gpu_mode() -> GpuMode {
    if on_ci() { GpuMode::Off } else { GpuMode::Auto }
}

/// ゴミ箱の偽物（OS のゴミ箱を汚さないよう、テスト用のフォルダへ移す）。
pub struct FakeTrash {
    pub dir: PathBuf,
    pub moved: Mutex<Vec<PathBuf>>,
    /// この名前のファイルは移せない（ゴミ箱のないドライブの代わり）。
    pub fail_names: Mutex<HashSet<String>>,
}

impl FakeTrash {
    pub fn new(dir: PathBuf) -> Arc<Self> {
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(Self {
            dir,
            moved: Mutex::new(Vec::new()),
            fail_names: Mutex::new(HashSet::new()),
        })
    }
}

impl TrashBackend for FakeTrash {
    fn move_to_trash(&self, path: &Path) -> Result<(), String> {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if self.fail_names.lock().contains(&name) {
            return Err("ゴミ箱が使えません（テスト）".to_owned());
        }
        let n = self.moved.lock().len();
        std::fs::rename(path, self.dir.join(format!("{n}-{name}"))).map_err(|e| e.to_string())?;
        self.moved.lock().push(path.to_path_buf());
        Ok(())
    }
}

/// テストの環境（一時フォルダ・写真のフォルダ・ゴミ箱の偽物）。
pub struct Env {
    pub dir: tempfile::TempDir,
    pub photos: PathBuf,
    pub trash: Arc<FakeTrash>,
}

impl Env {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        std::fs::create_dir_all(&photos).unwrap();
        let trash = FakeTrash::new(dir.path().join("fake-trash"));
        Self { dir, photos, trash }
    }

    pub fn catalog_path(&self) -> PathBuf {
        self.dir.path().join("catalog").join("catalog.db")
    }

    /// テスト用の設定。
    pub fn config(&self) -> CoreConfig {
        let mut c = CoreConfig::new(self.catalog_path(), self.dir.path().join("data"));
        c.worker = WorkerLaunch::SelfSubcommand {
            executable: Some(PathBuf::from(env!("CARGO_BIN_EXE_genzo-api-worker"))),
            subcommand: genzo_worker::WORKER_SUBCOMMAND.to_owned(),
        };
        c.default_utc_offset_minutes = Some(JST_MINUTES);
        c.gpu = test_gpu_mode();
        c.render_previews_after_import = false;
        c.batch_workers = Some(1);
        c.worker_test_hooks = true;
        c.worker_shm_dir = Some(self.dir.path().join("shm"));
        std::fs::create_dir_all(self.dir.path().join("shm")).unwrap();
        c.worker_timeouts = Some(JobTimeouts::uniform(TEST_JOB_TIMEOUT));
        c.worker_startup_timeout = Some(TEST_JOB_TIMEOUT);
        c
    }

    pub fn open(&self) -> Core {
        self.open_with(|_| {})
    }

    pub fn open_with(&self, f: impl FnOnce(&mut CoreConfig)) -> Core {
        let mut c = self.config();
        f(&mut c);
        Core::open_with(
            c,
            CoreHooks {
                trash: self.trash.clone(),
            },
        )
        .unwrap()
    }
}

/// ジョブが成功するまで待って、結果を返す。
pub fn wait_ok(core: &Core, job: u64) -> JobResult {
    let info = core.wait_job(job, Some(WAIT_LIMIT)).unwrap();
    assert_eq!(info.state, JobState::Succeeded, "{info:?}");
    info.result.expect("成功したジョブには結果がある")
}

/// ジョブが終わるまで待つ。
pub fn wait_done(core: &Core, job: u64) -> JobInfo {
    let info = core.wait_job(job, Some(WAIT_LIMIT)).unwrap();
    assert!(info.state.is_finished(), "{info:?}");
    info
}

/// 取り込んで、結果を返す。
pub fn import(core: &Core, path: &Path) -> ImportReport {
    let job = core.import_folder(path, true).unwrap();
    match wait_ok(core, job) {
        JobResult::Import(r) => r,
        other => panic!("{other:?}"),
    }
}

/// 既定の条件・並び（撮影日時順）で検索して、すべての詳細を返す。
pub fn all_items(core: &Core) -> Vec<VariantSummary> {
    let r = core
        .search(&SearchFilter::default(), SearchSort::default())
        .unwrap();
    items_of(core, r.generation, r.count)
}

/// 世代 `generation` の結果をすべて返す。
pub fn items_of(core: &Core, generation: u64, count: u64) -> Vec<VariantSummary> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < count {
        let page = core.range(generation, start, 100).unwrap();
        start += 100;
        out.extend(page.items);
    }
    out
}

/// 名前のファイルの variant（マスター）。
pub fn by_name<'a>(items: &'a [VariantSummary], name: &str) -> &'a VariantSummary {
    items
        .iter()
        .find(|s| s.file_name == name && s.is_master)
        .unwrap_or_else(|| panic!("{name} がない: {:?}", names(items)))
}

pub fn names(items: &[VariantSummary]) -> Vec<String> {
    items.iter().map(|s| s.file_name.clone()).collect()
}

/// フォルダの中のすべてのファイルのハッシュ（DATA-01 の確認用）。
pub fn hashes(dir: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let t = e.file_type().unwrap();
            if t.is_dir() {
                stack.push(e.path());
            } else if t.is_file() {
                out.insert(e.path(), genzo_catalog::full_hash(&e.path()).unwrap());
            }
        }
    }
    out
}

/// 撮影日時と GPS の Exif。
pub fn exif(local: Option<&str>, offset_minutes: Option<i32>, gps: Option<GpsCoord>) -> ExifData {
    ExifData {
        make: Some("GenzoTest".to_owned()),
        model: Some("Synthetic".to_owned()),
        datetime_original: local
            .map(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()),
        offset_time_original: offset_minutes.and_then(|m| FixedOffset::east_opt(m * 60)),
        gps,
        ..Default::default()
    }
}

/// 合成の画素（image crate で作るグラデーション）。
pub fn gradient(w: u32, h: u32, seed: u8) -> image::RgbImage {
    image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([
            (x * 255 / w.max(1)) as u8,
            (y * 255 / h.max(1)) as u8,
            seed.wrapping_mul(37).wrapping_add(40),
        ])
    })
}

/// 合成の JPEG を書く（Exif は任意）。
pub fn write_jpeg(path: &Path, (w, h): (u32, u32), seed: u8, exif: Option<&ExifData>) {
    let img = gradient(w, h, seed);
    let rgb = RgbImage8::from_raw(w, h, img.into_raw()).unwrap();
    let exif_bytes =
        exif.map(|e| genzo_media::exif_tiff_bytes(e, w, h, OutputColorSpace::Srgb).unwrap());
    let bytes = genzo_media::jpeg::encode_jpeg(&rgb, 92, None, exif_bytes.as_deref()).unwrap();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

/// 合成の PNG / TIFF を書く（image crate。撮影日時なし）。
pub fn write_plain_image(path: &Path, (w, h): (u32, u32), seed: u8) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    gradient(w, h, seed).save(path).unwrap();
}

/// ffmpeg が使えれば、短い動画を作る（`creation_time` は UTC の ISO 8601）。作れなければ `false`。
pub fn make_video(path: &Path, creation_time: &str) -> bool {
    let t = FfmpegTools::from_env();
    if !t.is_available() {
        eprintln!("ffmpeg / ffprobe がないため、動画の部分を飛ばす");
        return false;
    }
    let status = Command::new(&t.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=96x64:rate=10",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "mpeg4",
            "-metadata",
            &format!("creation_time={creation_time}"),
        ])
        .arg(path)
        .stdin(Stdio::null())
        .status();
    let ok = status.is_ok_and(|s| s.success()) && path.exists();
    if !ok {
        eprintln!("テスト用の動画を作れないため、動画の部分を飛ばす");
    }
    ok
}

/// LibRaw が有効なら、合成 DNG（埋め込みの JPEG のプレビュー付き）を書く。書かなければ `false`。
pub fn make_dng(path: &Path, datetime_exif: &str) -> bool {
    if !genzo_raw::LIBRAW_ENABLED {
        eprintln!("LibRaw が無効な build のため、DNG の部分を飛ばす");
        return false;
    }
    use genzo_testkit::bayer::{BayerParams, mosaic, working_to_camera};
    use genzo_testkit::image::FloatImage;
    const CAM_XYZ: [[f32; 3]; 3] = [
        [0.7424, -0.2329, -0.0466],
        [-0.4598, 1.2471, 0.2347],
        [-0.0715, 0.1505, 0.6066],
    ];
    let (w, h) = (96u32, 64u32);
    let scene = FloatImage::from_rgb_fn(w, h, |x, y| {
        let a = 0.05 + 0.6 * x as f32 / w as f32;
        let b = 0.05 + 0.4 * y as f32 / h as f32;
        [a, b, 0.2]
    })
    .unwrap();
    let camera = working_to_camera(&scene, &CAM_XYZ).unwrap();
    let mut params = BayerParams {
        cam_xyz: Some(CAM_XYZ),
        black_level: [512.0; 4],
        ..BayerParams::default()
    };
    params.metadata.capture.datetime = Some(datetime_exif.to_owned());
    params.metadata.capture.offset = Some("+09:00".to_owned());
    let raw = mosaic(&camera, &params).unwrap();
    let preview = gradient(w, h, 7);
    let jpeg = genzo_media::jpeg::encode_jpeg(
        &RgbImage8::from_raw(w, h, preview.into_raw()).unwrap(),
        90,
        None,
        None,
    )
    .unwrap();
    let options = genzo_raw::dng::DngOptions {
        preview: Some(genzo_raw::dng::DngPreview::Jpeg {
            width: w,
            height: h,
            data: jpeg,
        }),
        ..Default::default()
    };
    genzo_raw::dng::write_file(path, &raw, &options).unwrap();
    true
}

/// 条件が成り立つまで待つ（`WAIT_LIMIT` を過ぎたら失敗。時間は上限にだけ使い、結果は条件で決める）。
pub fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT_LIMIT;
    while !cond() {
        assert!(
            Instant::now() < deadline,
            "{what} が {WAIT_LIMIT:?} 以内に成り立たなかった"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 日付（テストの期間の指定用）。
pub fn utc(y: i32, m: u32, d: u32) -> chrono::DateTime<chrono::Utc> {
    NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
}

/// イベントを受け取り側から取り出す（溜まっている分だけ）。
pub fn drain(rx: &crossbeam_channel::Receiver<genzo_api::Event>) -> Vec<genzo_api::Event> {
    rx.try_iter().collect()
}
