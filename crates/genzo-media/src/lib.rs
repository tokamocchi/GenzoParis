//! 画像・動画の入出力。
//!
//! | モジュール | 内容 | 設計書 |
//! |---|---|---|
//! | [`buffer`] | 8bit / 16bit の RGB 画像（[`RgbImage8`]・[`RgbImage16`]・[`DynRgbImage`]） | 2.6 節 |
//! | [`decode`] | JPEG / PNG / TIFF の読み込み、埋め込みの ICC（なければ sRGB）と Exif | 2.6 節「RAW 以外の入力」、LIB-14 |
//! | [`orientation`] | Exif の向き（Orientation）を画素に反映する | PRV-01、4 章 |
//! | [`resize`] | 面積平均の縮小（リニアな値で平均する） | PRV-01、EXP-01 |
//! | [`cache`] | B5 のキャッシュ（Display P3 の JPEG、L0・L1）を作る | PRV-01、2.6 節の B5、4 章 |
//! | [`export`] | 書き出し（JPEG / TIFF 16bit / PNG 8・16bit、ICC と Exif の埋め込み、GPS の削除） | EXP-01・EXP-04、IQ-06、SEC-03、2.4 節 |
//! | [`jpeg`]・[`png_io`]・[`tiff_io`] | 各形式のエンコードとメタデータの埋め込み | EXP-01、IQ-06 |
//! | [`exif_write`]・[`exif_read`] | Exif の書き出し（最小限の TIFF の構造を自前で書く）と読み取り（kamadak-exif） | EXP-04、LIB-14 |
//! | [`video`] | ffprobe / ffmpeg（子プロセス）による動画のメタデータとサムネイル | VID-02・VID-03、IQ-09、PoC-7 |
//! | [`process`] | 子プロセスのタイムアウト付きの実行 | 1.2 節、6.3 節 |
//! | [`safe_write`] | 原本を上書きせず、不完全なファイルを残さない書き出し | 6.4 節（レビュー R-06） |
//!
//! - 元のファイルは読み取り専用で開く（DATA-01）。書き出しは一時ファイルを経由し、原本と同じ
//!   ファイルへの書き出しを拒否する（6.4 節）。
//! - 信頼できない入力のデコード（画像のデコード・ffmpeg の実行）はワーカープロセスで行う
//!   （04 の 1.1 節・1.2 節、SEC-05）。この crate の関数は、壊れた入力に対してパニックせず
//!   エラーを返すことと、画素数・確保量・出力の大きさ・時間に上限を設けることを担う。
//!
//! # 例
//!
//! ```
//! use genzo_media::{CacheSpec, DynRgbImage, ExifData, RgbImage16, encode_export, decode_image_bytes};
//! use genzo_model::{ExportFormat, ExportSettings, PhotoMetadata};
//!
//! // ステージ 17b の結果（sRGB で符号化済みの 16bit）を JPEG に書き出す。
//! let img = DynRgbImage::Rgb16(RgbImage16::from_fn(64, 32, |x, _| [x as u16 * 1000, 30000, 0])?);
//! let settings = ExportSettings {
//!     format: ExportFormat::Jpeg { quality: 90 },
//!     remove_gps: true,
//!     ..Default::default()
//! };
//! let exif = ExifData::from_photo_metadata(&PhotoMetadata::default());
//! let jpeg = encode_export(&img, &settings, Some(&exif))?;
//!
//! // 読み戻すと、sRGB の ICC プロファイルが埋め込まれている（IQ-06）。
//! let decoded = decode_image_bytes(&jpeg)?;
//! assert!(decoded.profile.is_embedded());
//!
//! // B5 のサムネイル（Display P3、長辺 320px）。
//! let thumb = genzo_media::cache_jpeg_from_encoded(&jpeg, None, CacheSpec::L0_THUMBNAIL)?;
//! assert_eq!((thumb.width, thumb.height), (64, 32));
//! # Ok::<(), genzo_media::MediaError>(())
//! ```

#![forbid(unsafe_code)]

pub mod buffer;
pub mod cache;
pub mod decode;
pub mod error;
pub mod exif_read;
pub mod exif_write;
pub mod export;
pub mod jpeg;
pub mod orientation;
pub mod png_io;
pub mod process;
mod profiles;
pub mod resize;
pub mod safe_write;
pub mod tiff_io;
pub mod video;

pub use buffer::{DynRgbImage, MAX_IMAGE_PIXELS, RgbImage, RgbImage8, RgbImage16, Sample};
pub use cache::{
    CACHE_ICC_VERSION, CacheJpeg, CacheSpec, cache_icc_profile, cache_jpeg_from_decoded,
    cache_jpeg_from_encoded, render_cache_jpeg, render_cache_rgb,
};
pub use decode::{
    DecodedImage, ImageFileFormat, ImageInfo, SourceProfile, SrgbAssumption, decode_image_bytes,
    decode_image_file, open_read_only, probe_image_bytes, probe_image_file,
};
pub use error::{MediaError, Result};
pub use exif_write::{EXIF_SOFTWARE, ExifData, exif_tiff_bytes};
pub use export::{
    EXPORT_ICC_VERSION, encode_export, export_icc_profile, export_image, standard_profile_for,
};
pub use orientation::{apply_orientation, apply_orientation_dyn};
pub use safe_write::{
    Destination, ProtectedFiles, WriteOutcome, resolve_destination, write_atomically,
};
pub use video::{FfmpegTools, VideoColorHandling, VideoProbe, VideoThumbnail, parse_ffprobe_json};
