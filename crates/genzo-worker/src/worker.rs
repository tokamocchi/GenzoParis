//! ワーカープロセスの処理（バイナリ `genzo-worker` の本体。docs/04_architecture.md の 1.2 節）。
//!
//! 標準入力から [`Request`] を 1 行ずつ読み、処理して [`Response`] を標準出力に 1 行で書く
//! （[`serve`]）。標準入力が閉じられるか、[`Job::Shutdown`] を受け取ったら終了する。
//!
//! # 守ること
//!
//! - **カタログに触れない**（DB に書き込むのは本体だけ。1.2 節）。結果は応答と共有メモリで返す。
//! - **元ファイルは読み取り専用で開く**（DATA-01）。RAW は LibRaw（`fopen(.., "rb")`）、それ以外は
//!   `genzo_media::open_read_only`。共有メモリのファイルは本体が作ったものを開くだけで、作らない。
//! - **GPU を使わない**（R-10。GPU への投入は本体の GPU スレッドだけ）。
//! - 埋め込みの ICC プロファイル（信頼できない入力）の解析と lcms2 の変換も、ワーカーで行う（SEC-05）。
//! - 標準出力には応答だけを書く（ログは標準エラー出力）。
//!
//! # 形式の判定（[`detect_photo_format`]）
//!
//! 先頭のバイト列で JPEG・PNG を判定する。TIFF の印（`II*\0` / `MM\0*`）は、多くの RAW（ARW・DNG・
//! NEF など）も同じなので、拡張子が `.tif` / `.tiff` のときだけ TIFF、それ以外は RAW とする。
//! どれでもなければ RAW として LibRaw に渡す（CR3・RAF なども LibRaw が判定する）。
//!
//! # 向き（Orientation）の扱い
//!
//! - [`Job::DecodeRaw`]・[`Job::DecodeImage`]: 画素に **反映しない**（ファイルに記録されたまま）。
//!   向きは撮影情報（`metadata.orientation`）で返す。RAW の CFA と同じく、向きはパイプラインの
//!   ジオメトリのステージ（2.1 節のステージ 9。2.7 節の座標系 S → G）で反映する。RAW と RAW 以外で
//!   座標系をそろえ、切り抜きなどの座標が同じ規則で扱えるようにするため。
//! - [`Job::Thumbnail`]・[`Job::VideoThumbnail`]: B5 のキャッシュなので **反映する**（04 の 4 章
//!   「ICC プロファイルと向きを反映してから B5 に変換して保存」）。RAW の埋め込み JPEG には、
//!   埋め込み JPEG の Exif ではなく RAW の向きを使う。
//!
//! # 共有メモリが足りない場合
//!
//! [`WorkerErrorKind::ShmTooSmall`] で必要な大きさを返し、作った結果（展開した RAW など）を 1 件だけ
//! 覚えておく。本体が大きな共有メモリで同じ依頼を送り直したら、覚えた結果を使う（展開をやり直さない）。
//! 別の依頼が来たら捨てる。

use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use genzo_color::{IccProfile, IccTransform, IccVersion, RenderingIntent, StandardProfile};
use genzo_media::{
    AdobeRgbBasis, CacheJpeg, CacheSpec, DecodedImage, DynRgbImage, FfmpegTools, ImageFileFormat,
    MediaError, RgbImage, RgbImage8, Sample, SourceProfile, SrgbAssumption,
    cache_jpeg_from_decoded, cache_jpeg_from_encoded, render_cache_jpeg,
};
use genzo_model::{Orientation, PhotoMetadata};
use genzo_raw::{CfaPattern, RawError, RawImage, ThumbnailFormat};

use crate::limits;
use crate::protocol::{
    GarbageMode, Job, JobOutput, LineRead, LinearImageInfo, MAX_LINE_BYTES, MemoryLimitStatus,
    OversizeMode, PROTOCOL_VERSION, PhotoFormat, PhotoProbe, PongInfo, ProfileSummary,
    RawFrameInfo, Request, Response, ShmRef, ThumbnailInfo, ThumbnailSource, VideoFrameInfo,
    WorkerError, WorkerErrorKind, read_line_limited,
};
use crate::shm::{PayloadKind, SHM_MAGIC, ShmHeader, ShmWriter};
use crate::validate::{MAX_THUMBNAIL_EDGE, sanitize_metadata};

/// ワーカーの起動の設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerOptions {
    /// テスト用の口（`Test*` のジョブ）を有効にするか。
    pub test_hooks: bool,
    /// メモリの上限の状態（[`Job::Ping`] の応答で本体に伝える）。
    pub memory_limit: MemoryLimitStatus,
}

impl Default for WorkerOptions {
    fn default() -> Self {
        Self {
            test_hooks: false,
            memory_limit: MemoryLimitStatus::NotRequested,
        }
    }
}

/// バイナリ `genzo-worker` の入口（[`run_worker`] を、プログラム名を除いた引数で呼ぶ）。
///
/// 引数: `--memory-limit-bytes <N>`（メモリの上限。[`crate::limits`]）。環境変数
/// `GENZO_WORKER_TEST_HOOKS=1` でテスト用の口を有効にする。
pub fn main_entry() -> ExitCode {
    run_worker(Vec::new(), std::env::args_os().skip(1).collect())
}

/// この実行ファイルをワーカーとして起動し直すときに、先頭に付ける引数（[`run_worker`] で設定する）。
static RELAUNCH_PREFIX: std::sync::OnceLock<Vec<OsString>> = std::sync::OnceLock::new();

/// ワーカーの本体を実行する（バイナリ `genzo-worker` の `main` と同じ処理）。
///
/// 本体の実行ファイル自身を隠しサブコマンド付きで起動する構成（`genzo __worker ...`。ワーカーの
/// 実行ファイルを別に配布しないため）では、本体の `main` が隠しサブコマンドを見つけたら、この関数を
/// 呼んで終了コードをそのまま返す。
///
/// - `relaunch_prefix`: この実行ファイルをワーカーとして起動し直すときに先頭に付ける引数
///   （`genzo-worker` では空、`genzo __worker` の構成では `["__worker"]`）。テスト用の子プロセス
///   （[`TEST_HOLD_WHILE_ARG`]）を起動するときに使う。
/// - `args`: ワーカーへの引数（プログラム名と隠しサブコマンドを除いたもの）。`--memory-limit-bytes <N>`
///   など（[`main_entry`]）。
pub fn run_worker(relaunch_prefix: Vec<OsString>, args: Vec<OsString>) -> ExitCode {
    // 1 つのプロセスでワーカーを実行するのは 1 回だけなので、最初の値を使う。
    let _ = RELAUNCH_PREFIX.set(relaunch_prefix);
    let mut memory_limit = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--memory-limit-bytes") => {
                let Some(n) = it.next().and_then(|v| v.to_str()?.parse::<u64>().ok()) else {
                    eprintln!("genzo-worker: --memory-limit-bytes には整数（バイト）が必要です");
                    return ExitCode::from(2);
                };
                memory_limit = Some(n);
            }
            Some("--version") => {
                println!("genzo-worker {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            // テスト用の子プロセス（テスト用の口が有効なときだけ）。
            Some(TEST_HOLD_WHILE_ARG) if test_hooks_enabled() => {
                let Some(path) = it.next() else {
                    eprintln!("genzo-worker: {TEST_HOLD_WHILE_ARG} にはパスが必要です");
                    return ExitCode::from(2);
                };
                hold_while_exists(Path::new(path));
                return ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("genzo-worker: 知らない引数です: {}", arg.to_string_lossy());
                return ExitCode::from(2);
            }
        }
    }
    let memory_limit = match memory_limit {
        Some(bytes) => limits::apply_memory_limit(bytes),
        None => MemoryLimitStatus::NotRequested,
    };
    let options = WorkerOptions {
        test_hooks: test_hooks_enabled(),
        memory_limit,
    };
    // 標準入力は別のスレッドで読み、閉じたら（本体が終了した・異常終了した）処理中の依頼があっても
    // すぐに終わる（孤立したワーカーがデコードを続け、CPU と共有メモリのファイルを使い続けないように）。
    let stdin = match watch_stdin() {
        Ok(r) => io::BufReader::new(r),
        Err(e) => {
            eprintln!("genzo-worker: 標準入力を読むスレッドを作れません: {e}");
            return ExitCode::from(3);
        }
    };
    let stdout = io::stdout().lock();
    match serve(stdin, stdout, &options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("genzo-worker: {e}");
            ExitCode::from(3)
        }
    }
}

/// 標準入力を読むスレッドから受け取るデータ（[`watch_stdin`]）。
struct StdinChannel {
    rx: crossbeam_channel::Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    pos: usize,
}

impl io::Read for StdinChannel {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.chunk.len() {
            match self.rx.recv() {
                Ok(chunk) => {
                    self.chunk = chunk;
                    self.pos = 0;
                }
                // 読むスレッドが終わった（入力の終わり）。
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.chunk.len() - self.pos);
        buf[..n].copy_from_slice(&self.chunk[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// 標準入力を別のスレッドで読む。入力が終わったら（本体が標準入力を閉じた・本体が終了した）、処理中の
/// 依頼があってもプロセスをすぐに終える（本体は [`Job::Shutdown`] の応答を受け取ってから閉じるので、
/// 正常な終了の手順は変わらない）。
fn watch_stdin() -> io::Result<StdinChannel> {
    use std::io::Read;
    let (tx, rx) = crossbeam_channel::unbounded::<Vec<u8>>();
    std::thread::Builder::new()
        .name("genzo-worker-stdin".to_owned())
        .spawn(move || {
            let mut stdin = io::stdin().lock();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => std::process::exit(0),
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            return;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => std::process::exit(0),
                }
            }
        })?;
    Ok(StdinChannel {
        rx,
        chunk: Vec::new(),
        pos: 0,
    })
}

/// 環境変数でテスト用の口が有効にされているか。
fn test_hooks_enabled() -> bool {
    std::env::var_os(crate::ENV_TEST_HOOKS).is_some_and(|v| v == "1")
}

/// 依頼を読んで処理し、応答を書く。入力が終わるか [`Job::Shutdown`] で `Ok(())` を返す。
///
/// 解釈できない依頼には [`WorkerErrorKind::InvalidRequest`] を返して続ける。長すぎる行
/// （[`MAX_LINE_BYTES`] 超）は、エラーの応答を書いてから `Err` を返す（行の続きを読み飛ばせないため）。
pub fn serve<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    options: &WorkerOptions,
) -> io::Result<()> {
    let mut worker = Worker::new(options);
    loop {
        let line = match read_line_limited(&mut input, MAX_LINE_BYTES)? {
            LineRead::Eof => return Ok(()),
            LineRead::TooLong => {
                write_response(
                    &mut output,
                    &Response {
                        id: 0,
                        result: Err(WorkerError::new(
                            WorkerErrorKind::InvalidRequest,
                            "依頼の行が長すぎる",
                        )),
                    },
                )?;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "依頼の行が長すぎる",
                ));
            }
            LineRead::Line(line) => line,
        };
        let request = match serde_json::from_slice::<Request>(&line) {
            Ok(r) => r,
            Err(e) => {
                let id = request_id(&line).unwrap_or(0);
                write_response(
                    &mut output,
                    &Response {
                        id,
                        result: Err(WorkerError::new(
                            WorkerErrorKind::InvalidRequest,
                            format!("依頼を解釈できない: {e}"),
                        )),
                    },
                )?;
                continue;
            }
        };
        let stop = matches!(request.job, Job::Shutdown);
        // None は、テスト用の口が出力を書き終えたことを表す。
        if let Some(result) = worker.handle(request.id, request.job, &mut output)? {
            write_response(
                &mut output,
                &Response {
                    id: request.id,
                    result,
                },
            )?;
        }
        if stop {
            return Ok(());
        }
    }
}

/// 解釈できない依頼から、番号だけを取り出す。
fn request_id(line: &[u8]) -> Option<u64> {
    serde_json::from_slice::<serde_json::Value>(line)
        .ok()?
        .get("id")?
        .as_u64()
}

fn write_response<W: Write>(out: &mut W, response: &Response) -> io::Result<()> {
    let mut line = match serde_json::to_vec(response) {
        Ok(v) => v,
        Err(e) => serde_json::to_vec(&Response {
            id: response.id,
            result: Err(WorkerError::new(
                WorkerErrorKind::Internal,
                format!("応答を JSON にできない: {e}"),
            )),
        })
        .map_err(io::Error::other)?,
    };
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}

/// 共有メモリが足りなかったときに覚えておく結果。
enum Pending {
    Raw {
        path: PathBuf,
        image: RawImage,
        cam_xyz_source: genzo_raw::CamXyzSource,
    },
    Image {
        path: PathBuf,
        decoded: DecodedImage,
    },
    Jpeg {
        key: ThumbKey,
        jpeg: Vec<u8>,
        info: ThumbnailInfo,
    },
}

/// サムネイルの依頼の同一性。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ThumbKey {
    video: bool,
    path: PathBuf,
    max_edge: u32,
    quality: u8,
}

struct Worker<'a> {
    options: &'a WorkerOptions,
    pending: Option<Pending>,
    ffmpeg: FfmpegTools,
    /// 直近に使った変換（元のプロファイルのバイト列とビット数で区別する）。同じプロファイルの
    /// 画像（sRGB の JPEG など）が続くときに、表を作り直さないため。
    linearizer: Option<(Vec<u8>, u8, Linearizer)>,
}

type JobResult = Result<JobOutput, WorkerError>;

impl<'a> Worker<'a> {
    fn new(options: &'a WorkerOptions) -> Self {
        Self {
            options,
            pending: None,
            ffmpeg: FfmpegTools::from_env(),
            linearizer: None,
        }
    }

    /// `source` と `bits` の変換（直近と同じなら使い回す）。
    fn linearizer(&mut self, source: &IccProfile, bits: u8) -> Result<&Linearizer, WorkerError> {
        let reusable = self
            .linearizer
            .as_ref()
            .is_some_and(|(icc, b, _)| *b == bits && icc.as_slice() == source.as_bytes());
        if !reusable {
            let lin = Linearizer::new(source, bits)?;
            self.linearizer = Some((source.as_bytes().to_vec(), bits, lin));
        }
        Ok(&self.linearizer.as_ref().expect("直前で設定した").2)
    }

    /// ジョブを処理する。`Ok(None)` は、テスト用の口が出力を書き終えた（応答を書かない）。
    fn handle<W: Write>(
        &mut self,
        id: u64,
        job: Job,
        out: &mut W,
    ) -> io::Result<Option<JobResult>> {
        // 覚えた結果は、同じ依頼の送り直しにだけ使う。
        let pending = self.pending.take();
        let result = match job {
            Job::Ping => Ok(JobOutput::Pong(self.pong())),
            Job::Shutdown => Ok(JobOutput::ShuttingDown),
            Job::ProbePhoto { path } => probe_photo(&path),
            Job::ProbeVideo { path } => self.probe_video(&path),
            Job::DecodeRaw { path, shm } => self.decode_raw(path, &shm, pending),
            Job::DecodeImage { path, shm } => self.decode_image(path, &shm, pending),
            Job::Thumbnail {
                path,
                max_edge,
                quality,
                shm,
            } => self.thumbnail(false, path, max_edge, quality, &shm, pending),
            Job::VideoThumbnail {
                path,
                max_edge,
                quality,
                shm,
            } => self.thumbnail(true, path, max_edge, quality, &shm, pending),
            Job::TestCrash { .. }
            | Job::TestHang { .. }
            | Job::TestGarbage { .. }
            | Job::TestOversize { .. }
            | Job::TestAllocate { .. } => return self.test_hook(id, job, out),
        };
        Ok(Some(result))
    }

    fn pong(&self) -> PongInfo {
        PongInfo {
            protocol_version: PROTOCOL_VERSION,
            worker_version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: std::process::id(),
            raw_decoder: genzo_raw::decoder_id(),
            test_hooks: self.options.test_hooks,
            memory_limit: self.options.memory_limit.clone(),
        }
    }

    fn probe_video(&self, path: &Path) -> JobResult {
        check_input(path)?;
        let probe = self.ffmpeg.probe(path).map_err(media_err)?;
        Ok(JobOutput::Video(probe))
    }

    fn decode_raw(&mut self, path: PathBuf, shm: &ShmRef, pending: Option<Pending>) -> JobResult {
        let (image, cam_xyz_source) = match pending {
            Some(Pending::Raw {
                path: p,
                image,
                cam_xyz_source,
            }) if p == path => (image, cam_xyz_source),
            _ => {
                check_input(&path)?;
                let format = detect_photo_format(&path)?;
                if format != PhotoFormat::Raw {
                    return Err(WorkerError::new(
                        WorkerErrorKind::Unsupported,
                        format!("RAW ではない（{format:?}）"),
                    ));
                }
                // 行列の出どころ（render_deps の記録用）も返すため、詳細付きで展開する。
                let decoded = genzo_raw::decode_file_with_details(&path).map_err(raw_err)?;
                let source = decoded.details.cam_xyz_source;
                (decoded.into_verified_image().map_err(raw_err)?, source)
            }
        };
        let data_len = image.data.len() as u64 * 2;
        if let Err(e) = ShmWriter::check_capacity(shm, data_len) {
            self.pending = Some(Pending::Raw {
                path,
                image,
                cam_xyz_source,
            });
            return Err(e);
        }
        let mut writer = ShmWriter::open(shm)?;
        let dst = writer
            .data_mut(data_len as usize)
            .ok_or_else(|| internal("共有メモリにデータが収まらない"))?;
        for (d, v) in dst.chunks_exact_mut(2).zip(&image.data) {
            d.copy_from_slice(&v.to_le_bytes());
        }
        writer.finish(
            ShmHeader {
                kind: PayloadKind::CfaU16,
                width: image.width,
                height: image.height,
                data_len,
                checksum: None,
            },
            shm.checksum,
        )?;
        let mut metadata = image.metadata;
        sanitize_metadata(&mut metadata);
        Ok(JobOutput::Raw(RawFrameInfo {
            width: image.width,
            height: image.height,
            cfa: image.cfa,
            black_level: image.black_level,
            white_level: image.white_level,
            as_shot_wb: image.as_shot_wb,
            cam_xyz: image.cam_xyz,
            metadata,
            decoder_id: genzo_raw::decoder_id(),
            cam_xyz_source: Some(cam_xyz_source),
        }))
    }

    fn decode_image(&mut self, path: PathBuf, shm: &ShmRef, pending: Option<Pending>) -> JobResult {
        let decoded = match pending {
            Some(Pending::Image { path: p, decoded }) if p == path => decoded,
            _ => {
                check_input(&path)?;
                let format = detect_photo_format(&path)?;
                if format == PhotoFormat::Raw {
                    return Err(WorkerError::new(
                        WorkerErrorKind::Unsupported,
                        "RAW は DecodeRaw で展開する",
                    ));
                }
                genzo_media::decode_image_file(&path).map_err(media_err)?
            }
        };
        let (width, height) = decoded.pixels.dimensions();
        let data_len = PayloadKind::RgbF32
            .data_len_for(width, height)
            .ok_or_else(|| internal("寸法が大きすぎる"))?;
        if let Err(e) = ShmWriter::check_capacity(shm, data_len) {
            self.pending = Some(Pending::Image { path, decoded });
            return Err(e);
        }
        let source = decoded.profile.to_icc().map_err(media_err)?;
        let linearizer = self.linearizer(&source, decoded.pixels.bits_per_channel())?;
        let mut writer = ShmWriter::open(shm)?;
        let dst = writer
            .data_mut(data_len as usize)
            .ok_or_else(|| internal("共有メモリにデータが収まらない"))?;
        match &decoded.pixels {
            DynRgbImage::Rgb8(img) => linearizer.write_rows(img, dst),
            DynRgbImage::Rgb16(img) => linearizer.write_rows(img, dst),
        }
        writer.finish(
            ShmHeader {
                kind: PayloadKind::RgbF32,
                width,
                height,
                data_len,
                checksum: None,
            },
            shm.checksum,
        )?;
        let mut metadata = decoded.metadata;
        sanitize_metadata(&mut metadata);
        Ok(JobOutput::Image(LinearImageInfo {
            format: photo_format_of(decoded.format),
            width,
            height,
            source_bits: decoded.pixels.bits_per_channel(),
            metadata,
            profile: profile_summary(&decoded.profile),
            alpha_dropped: decoded.alpha_dropped,
        }))
    }

    fn thumbnail(
        &mut self,
        video: bool,
        path: PathBuf,
        max_edge: u32,
        quality: Option<u8>,
        shm: &ShmRef,
        pending: Option<Pending>,
    ) -> JobResult {
        if !(1..=MAX_THUMBNAIL_EDGE).contains(&max_edge) {
            return Err(WorkerError::new(
                WorkerErrorKind::InvalidRequest,
                format!("長辺は 1〜{MAX_THUMBNAIL_EDGE}（{max_edge}）"),
            ));
        }
        let spec = CacheSpec {
            long_edge: max_edge,
            quality: quality.unwrap_or_else(|| default_quality_for(max_edge)),
        };
        spec.validate().map_err(media_err)?;
        let key = ThumbKey {
            video,
            path,
            max_edge,
            quality: spec.quality,
        };
        let (jpeg, info) = match pending {
            Some(Pending::Jpeg { key: k, jpeg, info }) if k == key => (jpeg, info),
            _ => {
                check_input(&key.path)?;
                if video {
                    let t = self.ffmpeg.thumbnail(&key.path, spec).map_err(media_err)?;
                    let info = thumbnail_info(
                        &t.jpeg,
                        ThumbnailSource::VideoFrame,
                        Some(VideoFrameInfo {
                            position_ms: t.position_ms,
                            color: t.color,
                        }),
                    );
                    (t.jpeg.bytes, info)
                } else {
                    let (cache, source) = photo_thumbnail(&key.path, spec)?;
                    let info = thumbnail_info(&cache, source, None);
                    (cache.bytes, info)
                }
            }
        };
        let data_len = jpeg.len() as u64;
        if let Err(e) = ShmWriter::check_capacity(shm, data_len) {
            self.pending = Some(Pending::Jpeg { key, jpeg, info });
            return Err(e);
        }
        let mut writer = ShmWriter::open(shm)?;
        writer
            .data_mut(jpeg.len())
            .ok_or_else(|| internal("共有メモリにデータが収まらない"))?
            .copy_from_slice(&jpeg);
        writer.finish(
            ShmHeader {
                kind: PayloadKind::Jpeg,
                width: info.width,
                height: info.height,
                data_len,
                checksum: None,
            },
            shm.checksum,
        )?;
        Ok(JobOutput::Thumbnail(info))
    }

    fn test_hook<W: Write>(
        &mut self,
        id: u64,
        job: Job,
        out: &mut W,
    ) -> io::Result<Option<JobResult>> {
        if !self.options.test_hooks {
            return Ok(Some(Err(WorkerError::new(
                WorkerErrorKind::InvalidRequest,
                format!(
                    "テスト用の口は無効です（環境変数 {}=1 で有効になる）",
                    crate::ENV_TEST_HOOKS
                ),
            ))));
        }
        match job {
            Job::TestCrash { shm, leave_child } => {
                if let Some(shm) = shm {
                    if leave_child {
                        leave_child_holding_pipes(&shm.path);
                    }
                    scribble(&shm);
                }
                eprintln!("genzo-worker: テスト用の口で異常終了します");
                std::process::abort();
            }
            Job::TestHang { shm } => {
                if let Some(shm) = shm {
                    scribble(&shm);
                }
                loop {
                    std::thread::sleep(Duration::from_secs(3600));
                }
            }
            Job::TestGarbage { mode } => {
                match mode {
                    GarbageMode::NotJson => out.write_all(b"this is not json\n")?,
                    GarbageMode::WrongId => {
                        let r = Response {
                            id: id.wrapping_add(1000),
                            result: Ok(JobOutput::Pong(self.pong())),
                        };
                        write_response(out, &r)?;
                        return Ok(None);
                    }
                    GarbageMode::TooLong => {
                        let mut line = vec![b'x'; MAX_LINE_BYTES + 16];
                        line.push(b'\n');
                        out.write_all(&line)?;
                    }
                }
                out.flush()?;
                Ok(None)
            }
            Job::TestOversize { mode, shm } => Ok(Some(oversize(mode, &shm))),
            Job::TestAllocate { bytes } => Ok(Some(allocate(bytes))),
            _ => unreachable!("test_hook にはテスト用のジョブだけを渡す"),
        }
    }
}

/// 写真のメタデータを読む。
fn probe_photo(path: &Path) -> JobResult {
    check_input(path)?;
    let format = detect_photo_format(path)?;
    let (mut metadata, profile) = if format == PhotoFormat::Raw {
        (genzo_raw::read_metadata(path).map_err(raw_err)?, None)
    } else {
        let info = genzo_media::probe_image_file(path).map_err(media_err)?;
        (info.metadata, Some(profile_summary(&info.profile)))
    };
    sanitize_metadata(&mut metadata);
    Ok(JobOutput::Photo(PhotoProbe {
        format,
        metadata,
        profile,
    }))
}

/// 写真（RAW の埋め込みサムネイル、または RAW 以外の画像そのもの）から B5 の JPEG を作る。
fn photo_thumbnail(
    path: &Path,
    spec: CacheSpec,
) -> Result<(CacheJpeg, ThumbnailSource), WorkerError> {
    let format = detect_photo_format(path)?;
    if format != PhotoFormat::Raw {
        let decoded = genzo_media::decode_image_file(path).map_err(media_err)?;
        let cache = cache_jpeg_from_decoded(&decoded, None, spec).map_err(media_err)?;
        return Ok((cache, ThumbnailSource::Image));
    }
    let thumb = genzo_raw::extract_thumbnail(path).map_err(raw_err)?;
    thumb.validate().map_err(raw_err)?;
    // 埋め込みの画像には向きが記録されていないことが多いので、RAW の向きを使う（4 章）。
    let orientation = genzo_raw::read_metadata(path)
        .map(|m| m.orientation)
        .unwrap_or(Orientation::Normal);
    match thumb.format {
        ThumbnailFormat::Jpeg => {
            let cache =
                cache_jpeg_from_encoded(&thumb.data, Some(orientation), spec).map_err(media_err)?;
            Ok((cache, ThumbnailSource::EmbeddedJpeg))
        }
        ThumbnailFormat::Rgb8 | ThumbnailFormat::Gray8 => {
            let data = if thumb.format == ThumbnailFormat::Gray8 {
                thumb.data.iter().flat_map(|&v| [v, v, v]).collect()
            } else {
                thumb.data
            };
            let img = RgbImage8::from_raw(thumb.width, thumb.height, data).map_err(media_err)?;
            // 埋め込みのビットマップは色の情報を持たないので sRGB とみなす。
            let srgb = IccProfile::standard(StandardProfile::Srgb)
                .map_err(|e| internal(format!("sRGB のプロファイルを作れない: {e}")))?;
            let cache = render_cache_jpeg(&DynRgbImage::Rgb8(img), &srgb, orientation, spec)
                .map_err(media_err)?;
            Ok((cache, ThumbnailSource::EmbeddedBitmap))
        }
    }
}

fn thumbnail_info(
    cache: &CacheJpeg,
    source: ThumbnailSource,
    video: Option<VideoFrameInfo>,
) -> ThumbnailInfo {
    ThumbnailInfo {
        width: cache.width,
        height: cache.height,
        byte_len: cache.bytes.len() as u64,
        source,
        video,
    }
}

/// サムネイルの長辺に応じた JPEG の品質の既定値。
///
/// 長辺が L0（320px）以下なら L0 の品質、それより大きければ L1 の品質
/// （`genzo_media::CacheSpec` の L0 / L1。04 の 4 章の表。品質の値そのものは genzo-media の仮置き）。
pub fn default_quality_for(max_edge: u32) -> u8 {
    if max_edge <= CacheSpec::L0_THUMBNAIL.long_edge {
        CacheSpec::L0_THUMBNAIL.quality
    } else {
        CacheSpec::L1_PREVIEW.quality
    }
}

/// ファイルの先頭のバイト列と拡張子から、写真の形式を判定する（モジュールの説明）。
///
/// ファイルは読み取り専用で開く（DATA-01）。
pub fn detect_photo_format(path: &Path) -> Result<PhotoFormat, WorkerError> {
    let mut file = genzo_media::open_read_only(path).map_err(media_err)?;
    let mut head = [0u8; 16];
    let mut n = 0;
    while n < head.len() {
        match file.read(&mut head[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(io_err(path, &e)),
        }
    }
    Ok(classify_photo_format(&head[..n], path))
}

/// 先頭のバイト列と拡張子から写真の形式を決める（[`detect_photo_format`] の判定の部分）。
pub fn classify_photo_format(head: &[u8], path: &Path) -> PhotoFormat {
    match ImageFileFormat::detect(head) {
        Some(ImageFileFormat::Jpeg) => PhotoFormat::Jpeg,
        Some(ImageFileFormat::Png) => PhotoFormat::Png,
        Some(ImageFileFormat::Tiff) => {
            let is_tiff_ext = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("tif") || e.eq_ignore_ascii_case("tiff"));
            if is_tiff_ext {
                PhotoFormat::Tiff
            } else {
                PhotoFormat::Raw
            }
        }
        None => PhotoFormat::Raw,
    }
}

fn photo_format_of(f: ImageFileFormat) -> PhotoFormat {
    match f {
        ImageFileFormat::Jpeg => PhotoFormat::Jpeg,
        ImageFileFormat::Png => PhotoFormat::Png,
        ImageFileFormat::Tiff => PhotoFormat::Tiff,
    }
}

/// ICC プロファイルを使わなかった理由の文字列。
fn icc_unused_reason(reason: &SrgbAssumption) -> String {
    match reason {
        SrgbAssumption::NoProfile => "ICC プロファイルが埋め込まれていない".to_owned(),
        SrgbAssumption::Invalid(e) => format!("ICC プロファイルを読めない: {e}"),
        SrgbAssumption::NotRgb => "RGB のプロファイルではない".to_owned(),
        SrgbAssumption::Unsupported(e) => format!("変換に使えないプロファイル: {e}"),
    }
}

fn profile_summary(p: &SourceProfile) -> ProfileSummary {
    match p {
        SourceProfile::Embedded(icc) => ProfileSummary {
            embedded: true,
            description: icc.description().map(str::to_owned),
            assumed_srgb_reason: None,
            assumed_adobe_rgb_reason: None,
        },
        SourceProfile::AssumedSrgb(reason) => ProfileSummary {
            embedded: false,
            description: None,
            assumed_srgb_reason: Some(icc_unused_reason(reason)),
            assumed_adobe_rgb_reason: None,
        },
        SourceProfile::AssumedAdobeRgb { basis, icc } => ProfileSummary {
            embedded: false,
            description: None,
            assumed_srgb_reason: None,
            assumed_adobe_rgb_reason: Some(format!(
                "{}（{}）",
                match basis {
                    AdobeRgbBasis::DcfOptionR03 => {
                        "Exif が DCF のオプション色空間（ColorSpace=Uncalibrated、InteropIndex=R03）を示す"
                    }
                    AdobeRgbBasis::ColorSpace2 =>
                        "Exif の ColorSpace が 2（非標準。Adobe RGB を示す）",
                },
                icc_unused_reason(icc)
            )),
        },
    }
}

/// 入力のファイルがあり、ディレクトリでないことを確かめる。
fn check_input(path: &Path) -> Result<(), WorkerError> {
    let meta = fs::metadata(path).map_err(|e| io_err(path, &e))?;
    if meta.is_dir() {
        return Err(WorkerError::new(
            WorkerErrorKind::InvalidRequest,
            format!("ディレクトリです: {}", path.display()),
        ));
    }
    Ok(())
}

fn internal(msg: impl Into<String>) -> WorkerError {
    WorkerError::new(WorkerErrorKind::Internal, msg)
}

fn io_err(path: &Path, e: &io::Error) -> WorkerError {
    let kind = if e.kind() == io::ErrorKind::NotFound {
        WorkerErrorKind::NotFound
    } else {
        WorkerErrorKind::Io
    };
    WorkerError::new(kind, format!("{}: {e}", path.display()))
}

/// RAW のエラーの種類。
fn raw_err(e: RawError) -> WorkerError {
    let kind = match &e {
        RawError::Unsupported(_) => WorkerErrorKind::Unsupported,
        RawError::Io(io) if io.kind() == io::ErrorKind::NotFound => WorkerErrorKind::NotFound,
        RawError::Io(_) => WorkerErrorKind::Io,
        RawError::TooManyPixels { .. } => WorkerErrorKind::TooLarge,
        _ => WorkerErrorKind::Decode,
    };
    WorkerError::new(kind, e.to_string())
}

/// genzo-media のエラーの種類。
fn media_err(e: MediaError) -> WorkerError {
    let kind = match &e {
        MediaError::Io { source, .. } if source.kind() == io::ErrorKind::NotFound => {
            WorkerErrorKind::NotFound
        }
        MediaError::Io { .. } => WorkerErrorKind::Io,
        MediaError::Decode { .. }
        | MediaError::InvalidImage { .. }
        | MediaError::VideoProbe { .. }
        | MediaError::Color(_) => WorkerErrorKind::Decode,
        MediaError::Unsupported { .. } => WorkerErrorKind::Unsupported,
        MediaError::TooLarge { .. } => WorkerErrorKind::TooLarge,
        MediaError::ToolSpawn { .. }
        | MediaError::ToolTimeout { .. }
        | MediaError::ToolOutputTooLarge { .. }
        | MediaError::ToolFailed { .. } => WorkerErrorKind::Tool,
        MediaError::InvalidArgument { .. } | MediaError::InvalidExportSettings(_) => {
            WorkerErrorKind::InvalidRequest
        }
        _ => WorkerErrorKind::Internal,
    };
    WorkerError::new(kind, e.to_string())
}

/// 符号化された RGB → リニア BT.2020（B2）の変換（04 の 2.6 節「RAW 以外の入力」）。
///
/// 行列とトーンカーブのプロファイル（sRGB・Display P3・Adobe RGB など、ほとんどの写真）では、変換は
/// 「チャンネルごとのトーンカーブ → 行列」なので
/// `f(r, g, b) = f(r, 0, 0) + f(0, g, 0) + f(0, 0, b) − 2·f(0, 0, 0)` が成り立つ。値の段階ごとの表を
/// lcms2 で作り、表を引いて足すだけにする（lcms2 の浮動小数点の変換は画素ごとにトーンカーブの
/// べき乗を計算するため遅い）。表で計算できるかは、格子の色で lcms2 の結果と比べて確かめ、
/// 合わなければ（LUT のプロファイルなど）画素ごとに lcms2 で変換する。
///
/// genzo-media の `cache` モジュールの内部の処理と同じ考え方（そちらは公開されていないため、
/// ここに持つ。上流への要望）。
enum Linearizer {
    Separable {
        tables: [Vec<[f32; 3]>; 3],
        black2: [f32; 3],
    },
    PerPixel {
        transform: IccTransform,
        max: f32,
    },
}

/// 表の和と lcms2 の結果の差の許容値（リニアの値。1 が白）。
///
/// lcms2 の浮動小数点の丸め（1e-6 程度）より十分大きく、8bit の 1 段階（暗部で約 3e-4）より小さい値
/// （genzo-media の `cache` と同じ値）。
const SEPARABLE_TOLERANCE: f32 = 1e-4;

impl Linearizer {
    fn new(source: &IccProfile, bits: u8) -> Result<Self, WorkerError> {
        let color = |e: genzo_color::ColorError| {
            WorkerError::new(WorkerErrorKind::Decode, format!("色の変換を作れない: {e}"))
        };
        let lin =
            IccProfile::standard_with_version(StandardProfile::LinearBt2020, IccVersion::V4_3)
                .map_err(color)?;
        let transform = IccTransform::new(source, &lin, RenderingIntent::RelativeColorimetric)
            .map_err(color)?;
        let max: u32 = if bits == 8 { 255 } else { 65535 };
        let maxf = max as f32;
        let mut tables: [Vec<[f32; 3]>; 3] = Default::default();
        for (c, table) in tables.iter_mut().enumerate() {
            let input: Vec<[f32; 3]> = (0..=max)
                .map(|v| {
                    let mut px = [0.0; 3];
                    px[c] = v as f32 / maxf;
                    px
                })
                .collect();
            let mut out = vec![[0.0; 3]; input.len()];
            transform.transform(&input, &mut out).map_err(color)?;
            *table = out;
        }
        let black = tables[0][0];
        let sep = Self::Separable {
            tables,
            black2: black.map(|v| 2.0 * v),
        };
        let grid = [0.0f32, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0].map(|v| (v * maxf).round() as u32);
        let mut probe = Vec::with_capacity(grid.len().pow(3));
        for &r in &grid {
            for &g in &grid {
                for &b in &grid {
                    probe.push([r, g, b]);
                }
            }
        }
        let normalized: Vec<[f32; 3]> = probe.iter().map(|p| p.map(|v| v as f32 / maxf)).collect();
        let mut expected = vec![[0.0; 3]; normalized.len()];
        transform
            .transform(&normalized, &mut expected)
            .map_err(color)?;
        let separable = probe.iter().zip(&expected).all(|(p, e)| {
            sep.lookup(*p)
                .iter()
                .zip(e)
                .all(|(a, b)| (a - b).abs() <= SEPARABLE_TOLERANCE)
        });
        Ok(if separable {
            sep
        } else {
            Self::PerPixel {
                transform,
                max: maxf,
            }
        })
    }

    fn lookup(&self, px: [u32; 3]) -> [f32; 3] {
        match self {
            Self::Separable { tables, black2 } => {
                let r = tables[0][px[0] as usize];
                let g = tables[1][px[1] as usize];
                let b = tables[2][px[2] as usize];
                [
                    r[0] + g[0] + b[0] - black2[0],
                    r[1] + g[1] + b[1] - black2[1],
                    r[2] + g[2] + b[2] - black2[2],
                ]
            }
            Self::PerPixel { .. } => unreachable!("lookup は Separable のときだけ使う"),
        }
    }

    /// 画像の各行をリニア BT.2020 にして、`dst` に f32（リトルエンディアン）の R, G, B で書く。
    fn write_rows<T: Sample>(&self, img: &RgbImage<T>, dst: &mut [u8]) {
        let width = img.width() as usize;
        let mut row_px = vec![[0.0f32; 3]; width];
        for (src, out) in img.rows().zip(dst.chunks_exact_mut(width * 12)) {
            match self {
                Self::Separable { .. } => {
                    for (o, p) in row_px.iter_mut().zip(src.chunks_exact(3)) {
                        *o = self.lookup([p[0].to_u32(), p[1].to_u32(), p[2].to_u32()]);
                    }
                }
                Self::PerPixel { transform, max } => {
                    for (o, p) in row_px.iter_mut().zip(src.chunks_exact(3)) {
                        *o = [
                            p[0].to_u32() as f32 / max,
                            p[1].to_u32() as f32 / max,
                            p[2].to_u32() as f32 / max,
                        ];
                    }
                    transform.transform_in_place(&mut row_px);
                }
            }
            for (d, px) in out.chunks_exact_mut(12).zip(&row_px) {
                d[0..4].copy_from_slice(&px[0].to_le_bytes());
                d[4..8].copy_from_slice(&px[1].to_le_bytes());
                d[8..12].copy_from_slice(&px[2].to_le_bytes());
            }
        }
    }

    #[cfg(test)]
    fn is_separable(&self) -> bool {
        matches!(self, Self::Separable { .. })
    }
}

/// テスト用の子プロセスの引数（[`Job::TestCrash`] の `leave_child`）。続けてファイルのパスを渡す。
pub const TEST_HOLD_WHILE_ARG: &str = "--test-hold-while";

/// テスト用の子プロセスが待つ時間の上限（ファイルが消えなくても、この時間で終わる）。結合テストの
/// タイムアウトより長くする（出力が閉じるのを待つ実装では、テストがタイムアウトになるように）。
pub const TEST_CHILD_HOLD_MAX: Duration = Duration::from_secs(120);

/// テスト用の子プロセスがファイルを確かめる間隔。
const TEST_CHILD_POLL: Duration = Duration::from_millis(20);

/// テスト用: 標準出力・標準エラー出力を引き継いだ子プロセス（このワーカーの実行ファイルを
/// [`run_worker`] の `relaunch_prefix` と [`TEST_HOLD_WHILE_ARG`] 付きで起動したもの）を残す。子プロセスは待たない（ワーカーはこの後
/// 異常終了する）。
fn leave_child_holding_pipes(hold_while: &Path) {
    let prefix = RELAUNCH_PREFIX.get().cloned().unwrap_or_default();
    let spawned = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .args(prefix)
            .arg(TEST_HOLD_WHILE_ARG)
            .arg(hold_while)
            .stdin(std::process::Stdio::null())
            .spawn()
    });
    match spawned {
        // 子プロセスは、ワーカーが終わった後に OS（init など）が回収する。
        Ok(child) => drop(child),
        Err(e) => eprintln!("genzo-worker: テスト用の子プロセスを作れない: {e}"),
    }
}

/// テスト用の子プロセスの本体: `path` のファイルがある間（最長 [`TEST_CHILD_HOLD_MAX`]）待つ。
fn hold_while_exists(path: &Path) {
    let start = std::time::Instant::now();
    while path.exists() && start.elapsed() < TEST_CHILD_HOLD_MAX {
        std::thread::sleep(TEST_CHILD_POLL);
    }
}

/// テスト用: 共有メモリのデータの領域の先頭に、少しだけ書く（ヘッダは書かない）。
fn scribble(shm: &ShmRef) {
    if let Ok(mut w) = ShmWriter::open(shm) {
        let n = (w.capacity() - crate::shm::SHM_HEADER_LEN).min(64);
        if let Some(d) = w.data_mut(n) {
            d.fill(0xAB);
        }
    }
}

/// テスト用: 偽ったヘッダ・付随情報を書いて、RAW の展開の結果として返す。
fn oversize(mode: OversizeMode, shm: &ShmRef) -> JobResult {
    const W: u32 = 64;
    const H: u32 = 48;
    let data_len = u64::from(W * H) * 2;
    ShmWriter::check_capacity(shm, data_len)?;
    let mut writer = ShmWriter::open(shm)?;
    let dst = writer
        .data_mut(data_len as usize)
        .ok_or_else(|| internal("共有メモリにデータが収まらない"))?;
    for (i, d) in dst.chunks_exact_mut(2).enumerate() {
        d.copy_from_slice(&(600 + (i % 1000) as u16).to_le_bytes());
    }
    let mut header = ShmHeader {
        kind: PayloadKind::CfaU16,
        width: W,
        height: H,
        data_len,
        checksum: None,
    };
    let mut info = RawFrameInfo {
        width: W,
        height: H,
        cfa: CfaPattern::RGGB,
        black_level: [512.0; 4],
        white_level: 16383.0,
        as_shot_wb: [2.0, 1.0, 1.5, 1.0],
        cam_xyz: None,
        metadata: PhotoMetadata::default(),
        decoder_id: Some("test-oversize".to_owned()),
        cam_xyz_source: None,
    };
    match mode {
        OversizeMode::TooManyPixels => {
            header.width = 20_000;
            header.height = 10_001;
            info.width = 20_000;
            info.height = 10_001;
        }
        OversizeMode::LengthMismatch => header.data_len = data_len - 2,
        OversizeMode::BeyondCapacity => header.data_len = shm.capacity,
        OversizeMode::DimensionMismatch => {
            info.width = H;
            info.height = W;
        }
        OversizeMode::BadMagic | OversizeMode::BadChecksum => {}
        OversizeMode::InvalidLevels => info.white_level = 0.0,
    }
    match mode {
        OversizeMode::BadMagic => {
            let mut bytes = header.encode();
            bytes[..SHM_MAGIC.len()].copy_from_slice(b"BADMAGIC");
            writer.write_header_bytes(&bytes);
        }
        OversizeMode::BadChecksum => {
            // 正しいチェックサムとは必ず違う値にする。
            let correct = crate::shm::checksum(
                writer
                    .data_mut(data_len as usize)
                    .ok_or_else(|| internal("共有メモリにデータが収まらない"))?,
            );
            header.checksum = Some(correct ^ 1);
            writer.write_header_bytes(&header.encode());
        }
        // データの長さが共有メモリに収まらないので、チェックサムは求められない。
        OversizeMode::BeyondCapacity => writer.write_header_bytes(&header.encode()),
        _ => writer.finish(header, shm.checksum)?,
    }
    Ok(JobOutput::Raw(info))
}

/// テスト用: メモリを確保して書き込む（上限があれば確保に失敗する）。
fn allocate(bytes: u64) -> JobResult {
    let n = usize::try_from(bytes)
        .map_err(|_| WorkerError::new(WorkerErrorKind::TooLarge, "確保する量が大きすぎる"))?;
    let mut v: Vec<u8> = Vec::new();
    v.try_reserve_exact(n).map_err(|e| {
        WorkerError::new(
            WorkerErrorKind::TooLarge,
            format!("{bytes} バイトを確保できない: {e}"),
        )
    })?;
    v.resize(n, 1);
    // 書き込んだことを最適化で消されないように、値を使う。
    let touched = v.iter().step_by(4096).map(|&b| u64::from(b)).sum::<u64>();
    std::hint::black_box(touched);
    Ok(JobOutput::Allocated { bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shm::{SHM_HEADER_LEN, ShmArena};
    use genzo_media::jpeg::encode_jpeg;

    fn run(options: &WorkerOptions, requests: &[Request]) -> Vec<Response> {
        let mut input = Vec::new();
        for r in requests {
            input.extend(serde_json::to_vec(r).unwrap());
            input.push(b'\n');
        }
        let mut output = Vec::new();
        serve(io::Cursor::new(input), &mut output, options).unwrap();
        output
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).unwrap())
            .collect()
    }

    /// Adobe RGB とみなした場合の要約（指摘 F28）。sRGB とみなした理由は入れない。
    #[test]
    fn profile_summary_reports_assumed_adobe_rgb() {
        let s = profile_summary(&SourceProfile::AssumedAdobeRgb {
            basis: AdobeRgbBasis::DcfOptionR03,
            icc: SrgbAssumption::NoProfile,
        });
        assert!(!s.embedded);
        assert_eq!(s.assumed_srgb_reason, None);
        let r = s.assumed_adobe_rgb_reason.unwrap();
        assert!(r.contains("R03") && r.contains("ICC"), "{r}");
        let s = profile_summary(&SourceProfile::AssumedSrgb(SrgbAssumption::NoProfile));
        assert!(s.assumed_srgb_reason.is_some());
        assert_eq!(s.assumed_adobe_rgb_reason, None);
        // 新しい項目がない古い形の JSON も読める（serde の既定値）。
        let old: ProfileSummary = serde_json::from_str(
            r#"{"embedded":false,"description":null,"assumed_srgb_reason":"x"}"#,
        )
        .unwrap();
        assert_eq!(old.assumed_adobe_rgb_reason, None);
    }

    fn write_jpeg(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let img = RgbImage8::from_fn(w, h, |x, y| [(x * 7) as u8, (y * 5) as u8, 128]).unwrap();
        let bytes = encode_jpeg(&img, 95, None, None).unwrap();
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn ping_and_shutdown() {
        let options = WorkerOptions::default();
        let out = run(
            &options,
            &[
                Request {
                    id: 1,
                    job: Job::Ping,
                },
                Request {
                    id: 2,
                    job: Job::Shutdown,
                },
                // Shutdown の後の依頼は処理しない。
                Request {
                    id: 3,
                    job: Job::Ping,
                },
            ],
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, 1);
        match &out[0].result {
            Ok(JobOutput::Pong(p)) => {
                assert_eq!(p.protocol_version, PROTOCOL_VERSION);
                assert_eq!(p.pid, std::process::id());
                assert!(!p.test_hooks);
                assert_eq!(p.raw_decoder, genzo_raw::decoder_id());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(out[1].result, Ok(JobOutput::ShuttingDown));
    }

    #[test]
    fn invalid_requests_get_errors_and_the_worker_continues() {
        let input = b"{\"id\":5,\"job\":{\"type\":\"format_disk\"}}\nnot json\n{\"id\":6,\"job\":{\"type\":\"ping\"}}\n";
        let mut output = Vec::new();
        serve(
            io::Cursor::new(input.to_vec()),
            &mut output,
            &WorkerOptions::default(),
        )
        .unwrap();
        let out: Vec<Response> = output
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).unwrap())
            .collect();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].id, 5);
        assert_eq!(
            out[0].result.as_ref().unwrap_err().kind,
            WorkerErrorKind::InvalidRequest
        );
        assert_eq!(out[1].id, 0);
        assert!(out[1].result.is_err());
        assert!(matches!(out[2].result, Ok(JobOutput::Pong(_))));
    }

    #[test]
    fn too_long_requests_stop_the_worker() {
        let mut input = vec![b'x'; MAX_LINE_BYTES + 1];
        input.push(b'\n');
        let mut output = Vec::new();
        assert!(
            serve(
                io::Cursor::new(input),
                &mut output,
                &WorkerOptions::default()
            )
            .is_err()
        );
        let r: Response = serde_json::from_slice(output.trim_ascii_end()).unwrap();
        assert_eq!(r.result.unwrap_err().kind, WorkerErrorKind::InvalidRequest);
    }

    #[test]
    fn test_hooks_are_disabled_by_default() {
        for job in [
            Job::TestCrash {
                shm: None,
                leave_child: false,
            },
            Job::TestHang { shm: None },
            Job::TestGarbage {
                mode: GarbageMode::NotJson,
            },
            Job::TestAllocate { bytes: 1 },
        ] {
            let out = run(&WorkerOptions::default(), &[Request { id: 9, job }]);
            assert_eq!(
                out[0].result.as_ref().unwrap_err().kind,
                WorkerErrorKind::InvalidRequest
            );
        }
    }

    #[test]
    fn probe_photo_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let jpeg = write_jpeg(dir.path(), "a.jpg", 40, 30);
        let out = run(
            &WorkerOptions::default(),
            &[
                Request {
                    id: 1,
                    job: Job::ProbePhoto { path: jpeg.clone() },
                },
                Request {
                    id: 2,
                    job: Job::ProbePhoto {
                        path: dir.path().join("missing.jpg"),
                    },
                },
                Request {
                    id: 3,
                    job: Job::ProbePhoto {
                        path: dir.path().to_path_buf(),
                    },
                },
            ],
        );
        match &out[0].result {
            Ok(JobOutput::Photo(p)) => {
                assert_eq!(p.format, PhotoFormat::Jpeg);
                assert_eq!((p.metadata.width, p.metadata.height), (Some(40), Some(30)));
                let prof = p.profile.as_ref().unwrap();
                assert!(!prof.embedded);
                assert!(prof.assumed_srgb_reason.is_some());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            out[1].result.as_ref().unwrap_err().kind,
            WorkerErrorKind::NotFound
        );
        assert_eq!(
            out[2].result.as_ref().unwrap_err().kind,
            WorkerErrorKind::InvalidRequest
        );
    }

    #[test]
    fn shm_too_small_then_retry_uses_the_kept_result() {
        let dir = tempfile::tempdir().unwrap();
        let jpeg = write_jpeg(dir.path(), "a.jpg", 20, 10);
        let arena = ShmArena::new_in(dir.path()).unwrap();
        let small = arena.allocate(SHM_HEADER_LEN as u64 + 16).unwrap();
        let big = arena
            .allocate(SHM_HEADER_LEN as u64 + 20 * 10 * 12)
            .unwrap();
        let options = WorkerOptions::default();
        let mut worker = Worker::new(&options);
        let mut sink = Vec::new();
        let job = |shm: &crate::shm::ShmBuffer| Job::DecodeImage {
            path: jpeg.clone(),
            shm: shm.shm_ref(true),
        };
        let r = worker.handle(1, job(&small), &mut sink).unwrap().unwrap();
        assert_eq!(
            r.unwrap_err().kind,
            WorkerErrorKind::ShmTooSmall {
                required_bytes: SHM_HEADER_LEN as u64 + 2400
            }
        );
        assert!(matches!(worker.pending, Some(Pending::Image { .. })));
        // 元のファイルを消しても、覚えた結果で処理できる（展開をやり直していない）。
        fs::remove_file(&jpeg).unwrap();
        let r = worker.handle(2, job(&big), &mut sink).unwrap().unwrap();
        match r.unwrap() {
            JobOutput::Image(info) => assert_eq!((info.width, info.height), (20, 10)),
            other => panic!("{other:?}"),
        }
        assert!(worker.pending.is_none());
        let p = big.open_payload(PayloadKind::RgbF32, true).unwrap();
        assert_eq!(p.read_to_vec().unwrap().len(), 2400);
        // 別の依頼が来たら、覚えた結果は捨てる。
        let r = worker.handle(3, job(&small), &mut sink).unwrap().unwrap();
        assert_eq!(r.unwrap_err().kind, WorkerErrorKind::NotFound);
    }

    #[test]
    fn decode_image_writes_linear_bt2020() {
        let dir = tempfile::tempdir().unwrap();
        // 灰色（sRGB の 8bit で 188 ≒ リニア 0.5）の PNG 風に、JPEG（品質 100）で作る。
        let img = RgbImage8::from_fn(8, 4, |_, _| [188, 188, 188]).unwrap();
        let bytes = encode_jpeg(&img, 100, None, None).unwrap();
        let path = dir.path().join("gray.jpg");
        fs::write(&path, &bytes).unwrap();
        let arena = ShmArena::new_in(dir.path()).unwrap();
        let buf = arena.allocate(SHM_HEADER_LEN as u64 + 8 * 4 * 12).unwrap();
        let out = run(
            &WorkerOptions::default(),
            &[Request {
                id: 1,
                job: Job::DecodeImage {
                    path,
                    shm: buf.shm_ref(false),
                },
            }],
        );
        let Ok(JobOutput::Image(info)) = &out[0].result else {
            panic!("{out:?}")
        };
        assert_eq!(info.source_bits, 8);
        let p = buf.open_payload(PayloadKind::RgbF32, false).unwrap();
        let (px, replaced) = crate::validate::linear_pixels(&p, &mut info.clone()).unwrap();
        assert_eq!(replaced, 0);
        let expected = genzo_color::transfer::srgb_decode(188.0 / 255.0) as f32;
        for v in px.iter().flatten() {
            // 無彩色は BT.2020 でも同じ値。JPEG の丸めの分を見込む。
            assert!((v - expected).abs() < 0.01, "{v} vs {expected}");
        }
    }

    #[test]
    fn linearizer_matches_lcms2_per_pixel() {
        for kind in [
            StandardProfile::Srgb,
            StandardProfile::DisplayP3,
            StandardProfile::AdobeRgb1998,
        ] {
            let src = IccProfile::standard(kind).unwrap();
            for bits in [8u8, 16] {
                let lin = Linearizer::new(&src, bits).unwrap();
                assert!(lin.is_separable(), "{kind:?} {bits}");
                let max = if bits == 8 { 255u32 } else { 65535 };
                let t = IccTransform::new(
                    &src,
                    &IccProfile::standard(StandardProfile::LinearBt2020).unwrap(),
                    RenderingIntent::RelativeColorimetric,
                )
                .unwrap();
                for i in 0..200u32 {
                    let h = i.wrapping_mul(2_654_435_761);
                    let p = [h % (max + 1), (h >> 8) % (max + 1), (h >> 16) % (max + 1)];
                    let mut e = [[0.0f32; 3]];
                    t.transform(&[p.map(|v| v as f32 / max as f32)], &mut e)
                        .unwrap();
                    let got = lin.lookup(p);
                    for k in 0..3 {
                        assert!((got[k] - e[0][k]).abs() < 2e-5, "{kind:?} {p:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn photo_formats_are_classified() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0];
        let png = *b"\x89PNG\r\n\x1a\n";
        let tiff = *b"II*\0\x08\0\0\0";
        assert_eq!(
            classify_photo_format(&jpeg, Path::new("a.arw")),
            PhotoFormat::Jpeg
        );
        assert_eq!(
            classify_photo_format(&png, Path::new("a")),
            PhotoFormat::Png
        );
        assert_eq!(
            classify_photo_format(&tiff, Path::new("a.TIF")),
            PhotoFormat::Tiff
        );
        assert_eq!(
            classify_photo_format(&tiff, Path::new("a.tiff")),
            PhotoFormat::Tiff
        );
        assert_eq!(
            classify_photo_format(&tiff, Path::new("a.ARW")),
            PhotoFormat::Raw
        );
        assert_eq!(
            classify_photo_format(&tiff, Path::new("a.dng")),
            PhotoFormat::Raw
        );
        assert_eq!(
            classify_photo_format(b"ftypcrx", Path::new("a.cr3")),
            PhotoFormat::Raw
        );
        assert_eq!(
            classify_photo_format(&[], Path::new("a.jpg")),
            PhotoFormat::Raw
        );
    }

    #[test]
    fn quality_follows_cache_levels() {
        assert_eq!(default_quality_for(160), CacheSpec::L0_THUMBNAIL.quality);
        assert_eq!(default_quality_for(320), CacheSpec::L0_THUMBNAIL.quality);
        assert_eq!(default_quality_for(321), CacheSpec::L1_PREVIEW.quality);
    }

    #[test]
    fn metadata_is_sanitized() {
        let mut m = PhotoMetadata {
            aperture: Some(f32::NAN),
            shutter_s: Some(0.01),
            focal_mm: Some(f32::INFINITY),
            ..Default::default()
        };
        sanitize_metadata(&mut m);
        assert_eq!(m.aperture, None);
        assert_eq!(m.shutter_s, Some(0.01));
        assert_eq!(m.focal_mm, None);
        serde_json::to_string(&m).unwrap();
    }
}
