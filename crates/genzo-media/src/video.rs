//! 動画のメタデータとサムネイル（VID-02・VID-03、PoC-7）。FFmpeg / ffprobe を子プロセスとして
//! 実行する（03 の技術選定、04 の 1.2 節。動画のデコーダーの異常で本体が落ちないように）。
//!
//! - メタデータ: `ffprobe -print_format json -show_format -show_streams` の出力から
//!   [`VideoMetadata`]（長さ・fps・コーデック・ビット深度・伝達関数・原色・寸法・作成日時）を作る
//!   （[`parse_ffprobe_json`]）。寸法は回転（Display Matrix の side data、古い版では `rotate` タグ）を
//!   反映した表示上の寸法。
//! - サムネイル: ffmpeg で代表フレーム（長さの [`THUMBNAIL_POSITION_RATIO`] の位置。
//!   [`SHORT_VIDEO_MAX_S`] 秒以下の短い動画と長さが分からない動画は先頭）を 1 枚、PNG で標準出力に
//!   出させ、B5 のキャッシュの JPEG にする（[`crate::cache`]）。
//!
//! ## 色の扱い（IQ-09）
//!
//! YUV → RGB の変換は ffmpeg（swscale）が行う。変換の行列は、動画に記録された行列
//! （`color_space`）があればそれを使い、記録がなければ **BT.709** を使う（ffmpeg の既定は BT.601
//! のため明示する。HD 以上の動画は BT.709 が普通）。得られた RGB は **sRGB とみなす**（BT.709 と
//! sRGB は原色と白色点が同じ。伝達関数の違い（BT.709 の OETF と sRGB）は無視する）。
//! HDR（PQ・HLG）のトーンマッピング、BT.2020 の色域の変換、Log 収録（D-Log など。メタデータでは
//! 見分けられないことが多い）の LUT の適用は **later**（VID-06・CLR-03）。現状はそのまま sRGB と
//! みなして表示するので、色が正しくない。該当しそうな動画は [`VideoColorHandling`] で知らせる。
//!
//! ## 安全性
//!
//! - 実行ファイルは環境変数 `GENZO_FFPROBE` / `GENZO_FFMPEG`（なければ PATH の `ffprobe` /
//!   `ffmpeg`）。シェルは使わない。
//! - 入力は `file:` を付けて渡し、`-protocol_whitelist file` でファイル以外のプロトコル（ネットワーク
//!   など）を開かせない。ファイル名が `-` で始まっても、オプションとして解釈されない。
//! - タイムアウトを超えたら強制終了する。出力の大きさにも上限を設ける（[`crate::process`]）。
//! - ffmpeg・ffprobe の出力が壊れていてもパニックしない（解釈できない項目は `None`）。

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::Duration;

use genzo_model::{Orientation, VideoMetadata};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cache::{CacheJpeg, CacheSpec, render_cache_jpeg};
use crate::decode::{ImageFileFormat, decode_image_bytes};
use crate::error::{MediaError, Result};
use crate::process::run_with_timeout;

/// ffprobe の実行ファイルを指定する環境変数。
pub const ENV_FFPROBE: &str = "GENZO_FFPROBE";
/// ffmpeg の実行ファイルを指定する環境変数。
pub const ENV_FFMPEG: &str = "GENZO_FFMPEG";

/// ffprobe のタイムアウトの既定値（仮置き）。
///
/// PERF-11（1 本あたり 2 秒以内）より十分長く、ネットワークドライブや起動直後の HDD でも
/// 誤って打ち切らない長さ。PoC-7 の計測で見直す。
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// ffmpeg（代表フレームの取り出し）のタイムアウトの既定値（仮置き）。
///
/// 4K・10bit の HEVC をソフトウェアでデコードする場合も見込んだ長さ。PoC-7 の計測で見直す。
pub const DEFAULT_FRAME_TIMEOUT: Duration = Duration::from_secs(30);

/// ffprobe の出力（JSON）の大きさの上限（仮置き）。チャプターやタグが多い動画でも十分な大きさ。
pub const MAX_PROBE_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// 代表フレームの PNG の大きさの上限（仮置き）。8K（7680×4320）の無圧縮の RGB（約 100 MB）が
/// 収まる大きさ。
pub const MAX_FRAME_OUTPUT_BYTES: usize = 256 * 1024 * 1024;

/// 代表フレームの位置（長さに対する割合。仮置き）。
///
/// 先頭は黒い画面や手ぶれのことが多いため、少し後ろのフレームを使う（VID-02 の「代表フレーム」）。
pub const THUMBNAIL_POSITION_RATIO: f64 = 0.1;

/// これ以下の長さ（秒）の動画は先頭のフレームを使う（仮置き）。
pub const SHORT_VIDEO_MAX_S: f64 = 2.0;

/// fps として受け付ける上限（これを超える値は記録の誤りとみなす。仮置き）。
const MAX_SANE_FPS: f64 = 10_000.0;

/// 長さ（秒）として受け付ける上限（約 1 年。これを超える値は記録の誤りとみなす）。
const MAX_SANE_DURATION_S: f64 = 365.0 * 24.0 * 3600.0;

/// ffprobe の結果。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VideoProbe {
    /// カタログに保存するメタデータ（寸法は回転を反映した表示上の寸法）。
    pub metadata: VideoMetadata,
    /// 使う映像のストリームの番号（ffmpeg の `-map 0:N`）。
    pub stream_index: u32,
    /// 回転（ffprobe の値を 0・90・180・270 に丸めたもの。FFmpeg の規約で反時計回り）。
    pub rotation_deg: u32,
    /// ストリームに記録された寸法（回転を反映する前）。
    pub coded_size: Option<(u32, u32)>,
    /// 画素の形式（例: `"yuv420p10le"`）。
    pub pix_fmt: Option<String>,
    /// YUV → RGB の行列（ffprobe の color_space。例: `"bt709"`）。
    pub color_space: Option<String>,
    /// 値の範囲（`"tv"`（制限）・`"pc"`（全域））。
    pub color_range: Option<String>,
}

/// 動画の色をどう扱ったか（IQ-09）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoColorHandling {
    /// BT.709（または記録なし）として、sRGB とみなした。
    Bt709,
    /// HDR（PQ・HLG）だが、トーンマッピングせずに sRGB とみなした（later）。
    HdrNotToneMapped,
    /// BT.709 以外の原色（BT.2020、SD の BT.601 など）だが、原色を変換せずに sRGB とみなした
    /// （later）。
    PrimariesNotConverted,
}

impl VideoColorHandling {
    /// メタデータから判定する。
    pub fn from_metadata(m: &VideoMetadata) -> Self {
        let transfer = m.color_transfer.as_deref().unwrap_or_default();
        let primaries = m.color_primaries.as_deref().unwrap_or_default();
        if matches!(transfer, "smpte2084" | "arib-std-b67") {
            Self::HdrNotToneMapped
        } else if !primaries.is_empty() && primaries != "bt709" {
            // bt470bg・smpte170m（SD）は BT.709 と原色が近いが、厳密には違う。
            Self::PrimariesNotConverted
        } else {
            Self::Bt709
        }
    }
}

/// 動画のサムネイル。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoThumbnail {
    /// B5 のキャッシュの JPEG。
    pub jpeg: CacheJpeg,
    /// 取り出したフレームの位置（ミリ秒）。
    pub position_ms: u64,
    /// 色の扱い。
    pub color: VideoColorHandling,
}

/// "unknown" などの値を `None` にした文字列。
fn meaningful(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?.trim();
    match s {
        "" | "unknown" | "unspecified" | "reserved" | "N/A" => None,
        _ => Some(s.chars().take(256).collect()),
    }
}

/// 数値または数値の文字列。
fn number(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

/// `"30000/1001"` のような有理数。分母が 0 なら `None`。
fn parse_rate(v: Option<&Value>) -> Option<f64> {
    let s = v?.as_str()?;
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    let n: f64 = n.trim().parse().ok()?;
    let d: f64 = d.trim().parse().ok()?;
    if d == 0.0 {
        return None;
    }
    let r = n / d;
    (r.is_finite() && r > 0.0 && r <= MAX_SANE_FPS).then_some(r)
}

fn positive_u32(v: Option<&Value>) -> Option<u32> {
    let n = number(v)?;
    (n >= 1.0 && n <= f64::from(u32::MAX) && n.fract() == 0.0).then_some(n as u32)
}

/// 画素の形式の名前からビット深度を推定する（例: `yuv420p` → 8、`yuv420p10le` → 10、`p010le` → 10）。
pub fn bit_depth_from_pix_fmt(pix_fmt: &str) -> Option<u32> {
    let s = pix_fmt.trim().to_ascii_lowercase();
    let s = s
        .strip_suffix("le")
        .or_else(|| s.strip_suffix("be"))
        .unwrap_or(&s);
    let known = match s {
        "nv12" | "nv21" | "nv16" | "nv24" | "nv42" | "yuyv422" | "uyvy422" | "yvyu422"
        | "rgb24" | "bgr24" | "rgba" | "bgra" | "argb" | "abgr" | "rgb0" | "bgr0" | "0rgb"
        | "0bgr" | "gray" | "pal8" => Some(8),
        "p010" | "p210" | "p410" | "y210" | "x2rgb10" | "x2bgr10" | "v210" | "xv30" => Some(10),
        "p012" | "p212" | "p412" | "y212" | "xv36" => Some(12),
        "p016" | "p216" | "p416" | "rgb48" | "bgr48" | "rgba64" | "bgra64" => Some(16),
        _ => None,
    };
    if known.is_some() {
        return known;
    }
    let digits_after = |prefix_end: usize| -> Option<u32> {
        let rest = &s[prefix_end..];
        (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
            .then(|| rest.parse().ok())
            .flatten()
            .filter(|&d: &u32| (1..=32).contains(&d))
    };
    if let Some(rest) = s.strip_prefix("gray") {
        return digits_after(s.len() - rest.len());
    }
    if let Some(i) = s.rfind('p') {
        if i + 1 == s.len() {
            // 末尾が p（yuv420p・yuvj420p・gbrp など）は 8bit。
            return s
                .starts_with("yuv")
                .then_some(8)
                .or(s.starts_with("gbr").then_some(8));
        }
        return digits_after(i + 1);
    }
    None
}

/// 大文字・小文字を区別せずにタグを引く。
fn tag<'a>(obj: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    obj?.get("tags")?
        .as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
}

/// 回転（度）を 0・90・180・270 にする。
fn normalize_rotation(deg: f64) -> u32 {
    let quarter = (deg / 90.0).round() as i64;
    (quarter.rem_euclid(4) * 90) as u32
}

fn stream_rotation(stream: &Value) -> u32 {
    let from_side_data = stream
        .get("side_data_list")
        .and_then(Value::as_array)
        .and_then(|list| list.iter().find_map(|sd| number(sd.get("rotation"))));
    let from_tag = number(tag(Some(stream), "rotate"));
    from_side_data
        .or(from_tag)
        .map(normalize_rotation)
        .unwrap_or(0)
}

/// ffprobe の JSON（`-print_format json -show_format -show_streams`）を解釈する。
///
/// 映像のストリーム（カバー画像（attached_pic）を除く最初のもの）がなければエラー。
pub fn parse_ffprobe_json(json: &[u8]) -> Result<VideoProbe> {
    let root: Value = serde_json::from_slice(json).map_err(|e| MediaError::VideoProbe {
        reason: format!("JSON として読めない: {e}"),
    })?;
    let streams = root
        .get("streams")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let format = root.get("format");
    let (position, stream) = streams
        .iter()
        .enumerate()
        .find(|(_, s)| {
            s.get("codec_type").and_then(Value::as_str) == Some("video")
                && s.get("disposition")
                    .and_then(|d| d.get("attached_pic"))
                    .and_then(Value::as_i64)
                    != Some(1)
        })
        .ok_or_else(|| MediaError::VideoProbe {
            reason: "映像のストリームがない".to_owned(),
        })?;
    let stream_index = stream
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|i| u32::try_from(i).ok())
        .unwrap_or(position as u32);

    let duration_s = number(format.and_then(|f| f.get("duration")))
        .or_else(|| number(stream.get("duration")))
        .filter(|d| *d >= 0.0 && *d <= MAX_SANE_DURATION_S);
    let fps =
        parse_rate(stream.get("avg_frame_rate")).or_else(|| parse_rate(stream.get("r_frame_rate")));
    let pix_fmt = meaningful(stream.get("pix_fmt"));
    let bit_depth = positive_u32(stream.get("bits_per_raw_sample"))
        .filter(|d| (1..=32).contains(d))
        .or_else(|| pix_fmt.as_deref().and_then(bit_depth_from_pix_fmt));
    let rotation_deg = stream_rotation(stream);
    let coded = positive_u32(stream.get("width")).zip(positive_u32(stream.get("height")));
    let (width, height) = match coded {
        Some((w, h)) if rotation_deg % 180 == 90 => (Some(h), Some(w)),
        Some((w, h)) => (Some(w), Some(h)),
        None => (None, None),
    };
    // 作成日時: Apple の端末が書く現地時刻付きの値 → コンテナの creation_time（UTC）→ ストリームの値。
    let creation_time = meaningful(tag(format, "com.apple.quicktime.creationdate"))
        .or_else(|| meaningful(tag(format, "creation_time")))
        .or_else(|| meaningful(tag(Some(stream), "creation_time")));

    Ok(VideoProbe {
        metadata: VideoMetadata {
            duration_s,
            fps,
            codec: meaningful(stream.get("codec_name")),
            bit_depth,
            color_transfer: meaningful(stream.get("color_transfer")),
            color_primaries: meaningful(stream.get("color_primaries")),
            width,
            height,
            creation_time,
        },
        stream_index,
        rotation_deg,
        coded_size: coded,
        pix_fmt,
        color_space: meaningful(stream.get("color_space")),
        color_range: meaningful(stream.get("color_range")),
    })
}

/// 代表フレームの位置（秒）。
pub fn thumbnail_position_s(duration_s: Option<f64>) -> f64 {
    match duration_s {
        Some(d) if d.is_finite() && d > SHORT_VIDEO_MAX_S => d * THUMBNAIL_POSITION_RATIO,
        _ => 0.0,
    }
}

/// ffmpeg の scale フィルターの `in_color_matrix` に渡す値。記録があれば `auto`（記録どおり）、
/// なければ `bt709`。
fn input_color_matrix(probe: &VideoProbe) -> &'static str {
    if probe.color_space.is_some() {
        "auto"
    } else {
        "bt709"
    }
}

/// `file:` を付けた入力の指定。
fn file_url(path: &Path) -> OsString {
    let mut s = OsString::from("file:");
    s.push(path.as_os_str());
    s
}

/// FFmpeg / ffprobe の実行ファイルとタイムアウト。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegTools {
    /// ffprobe の実行ファイル。
    pub ffprobe: OsString,
    /// ffmpeg の実行ファイル。
    pub ffmpeg: OsString,
    /// ffprobe のタイムアウト。
    pub probe_timeout: Duration,
    /// ffmpeg（代表フレームの取り出し）のタイムアウト。
    pub frame_timeout: Duration,
}

impl FfmpegTools {
    /// 環境変数 `GENZO_FFPROBE` / `GENZO_FFMPEG`（空でなければ）、なければ PATH の `ffprobe` /
    /// `ffmpeg` を使う。
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var_os(k))
    }

    /// 環境変数の代わりに `lookup` で実行ファイルを決める（テスト用）。
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> Self {
        let pick = |key: &str, default: &str| {
            lookup(key)
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| OsString::from(default))
        };
        Self {
            ffprobe: pick(ENV_FFPROBE, "ffprobe"),
            ffmpeg: pick(ENV_FFMPEG, "ffmpeg"),
            probe_timeout: DEFAULT_PROBE_TIMEOUT,
            frame_timeout: DEFAULT_FRAME_TIMEOUT,
        }
    }

    /// ffprobe と ffmpeg が実行できるか（`-version` が成功するか）。
    pub fn is_available(&self) -> bool {
        [&self.ffprobe, &self.ffmpeg].iter().all(|p| {
            run_with_timeout(p, &["-version"], Duration::from_secs(10), 1024 * 1024)
                .is_ok_and(|o| o.status.success())
        })
    }

    fn check_status(program: &OsStr, out: &crate::process::ProcessOutput) -> Result<()> {
        if out.status.success() {
            Ok(())
        } else {
            Err(MediaError::ToolFailed {
                program: program.to_string_lossy().into_owned(),
                status: out.status.to_string(),
                stderr: out.stderr_tail.trim().to_owned(),
            })
        }
    }

    /// ffprobe で動画の情報を読む（VID-03）。
    pub fn probe(&self, path: &Path) -> Result<VideoProbe> {
        let args: Vec<OsString> = [
            "-hide_banner",
            "-v",
            "error",
            "-protocol_whitelist",
            "file",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-i",
        ]
        .iter()
        .map(OsString::from)
        .chain([file_url(path)])
        .collect();
        let out = run_with_timeout(
            &self.ffprobe,
            &args,
            self.probe_timeout,
            MAX_PROBE_OUTPUT_BYTES,
        )?;
        Self::check_status(&self.ffprobe, &out)?;
        parse_ffprobe_json(&out.stdout)
    }

    /// ffmpeg で `position_s` 秒のフレームを 1 枚、PNG（8bit の RGB）で取り出す。
    ///
    /// 回転は ffmpeg が反映する（既定の autorotate）。フレームがなければ空のバイト列を返す。
    pub fn extract_frame_png(
        &self,
        path: &Path,
        probe: &VideoProbe,
        position_s: f64,
    ) -> Result<Vec<u8>> {
        let mut args: Vec<OsString> = [
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-protocol_whitelist",
            "file",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        if position_s > 0.0 && position_s.is_finite() {
            args.push("-ss".into());
            args.push(format!("{position_s:.3}").into());
        }
        args.push("-i".into());
        args.push(file_url(path));
        for a in [
            "-map".to_owned(),
            format!("0:{}", probe.stream_index),
            "-frames:v".to_owned(),
            "1".to_owned(),
            "-an".to_owned(),
            "-sn".to_owned(),
            "-dn".to_owned(),
            "-vf".to_owned(),
            format!(
                "scale=in_color_matrix={},format=rgb24",
                input_color_matrix(probe)
            ),
            "-c:v".to_owned(),
            "png".to_owned(),
            // 標準出力に流すだけなので圧縮しない（速さを優先）。
            "-compression_level".to_owned(),
            "0".to_owned(),
            "-f".to_owned(),
            "image2pipe".to_owned(),
            "pipe:1".to_owned(),
        ] {
            args.push(a.into());
        }
        let out = run_with_timeout(
            &self.ffmpeg,
            &args,
            self.frame_timeout,
            MAX_FRAME_OUTPUT_BYTES,
        )?;
        Self::check_status(&self.ffmpeg, &out)?;
        Ok(out.stdout)
    }

    /// 動画のサムネイル（B5 のキャッシュの JPEG）を作る（VID-02）。
    ///
    /// 代表フレームが取り出せなければ（長さの記録が実際より長いなど）、先頭のフレームで試し直す。
    pub fn thumbnail(&self, path: &Path, spec: CacheSpec) -> Result<VideoThumbnail> {
        spec.validate()?;
        let probe = self.probe(path)?;
        self.thumbnail_with_probe(path, &probe, spec)
    }

    /// [`thumbnail`](Self::thumbnail) の、ffprobe の結果を渡す版。
    pub fn thumbnail_with_probe(
        &self,
        path: &Path,
        probe: &VideoProbe,
        spec: CacheSpec,
    ) -> Result<VideoThumbnail> {
        spec.validate()?;
        let mut position = thumbnail_position_s(probe.metadata.duration_s);
        let mut png = self.extract_frame_png(path, probe, position)?;
        if png.is_empty() && position > 0.0 {
            position = 0.0;
            png = self.extract_frame_png(path, probe, position)?;
        }
        if png.is_empty() {
            return Err(MediaError::decode("動画からフレームを取り出せない"));
        }
        if ImageFileFormat::detect(&png) != Some(ImageFileFormat::Png) {
            return Err(MediaError::decode("ffmpeg の出力が PNG ではない"));
        }
        let decoded = decode_image_bytes(&png)?;
        // ffmpeg が PNG に書いた色の情報は使わず、sRGB とみなす（モジュールの説明を参照）。
        let srgb = crate::profiles::standard(
            genzo_color::StandardProfile::Srgb,
            genzo_color::IccVersion::V4_3,
        )?;
        let jpeg = render_cache_jpeg(&decoded.pixels, &srgb, Orientation::Normal, spec)?;
        Ok(VideoThumbnail {
            jpeg,
            position_ms: (position * 1000.0).round() as u64,
            color: VideoColorHandling::from_metadata(&probe.metadata),
        })
    }
}

impl Default for FfmpegTools {
    fn default() -> Self {
        Self::from_env()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "streams": [
            {
                "index": 0, "codec_name": "aac", "codec_type": "audio"
            },
            {
                "index": 1, "codec_name": "mjpeg", "codec_type": "video",
                "width": 160, "height": 120,
                "disposition": {"attached_pic": 1}
            },
            {
                "index": 2, "codec_name": "hevc", "codec_type": "video",
                "width": 3840, "height": 2160,
                "pix_fmt": "yuv420p10le", "color_range": "tv",
                "color_space": "bt2020nc", "color_transfer": "arib-std-b67",
                "color_primaries": "bt2020",
                "r_frame_rate": "60000/1001", "avg_frame_rate": "59940/1000",
                "duration": "12.5",
                "tags": {"creation_time": "2024-05-01T03:04:05.000000Z"},
                "side_data_list": [
                    {"side_data_type": "Display Matrix", "rotation": -90}
                ]
            }
        ],
        "format": {
            "duration": "12.512000",
            "tags": {"creation_time": "2024-05-01T03:04:05.000000Z",
                     "com.apple.quicktime.creationdate": "2024-05-01T12:04:05+0900"}
        }
    }"#;

    #[test]
    fn parses_metadata_with_rotation_and_cover_art() {
        let p = parse_ffprobe_json(SAMPLE.as_bytes()).unwrap();
        assert_eq!(p.stream_index, 2);
        assert_eq!(p.rotation_deg, 270);
        assert_eq!(p.coded_size, Some((3840, 2160)));
        let m = &p.metadata;
        assert_eq!((m.width, m.height), (Some(2160), Some(3840)));
        assert_eq!(m.codec.as_deref(), Some("hevc"));
        assert_eq!(m.bit_depth, Some(10));
        assert!((m.fps.unwrap() - 59.94).abs() < 1e-9);
        assert!((m.duration_s.unwrap() - 12.512).abs() < 1e-9);
        assert_eq!(m.color_transfer.as_deref(), Some("arib-std-b67"));
        assert_eq!(m.color_primaries.as_deref(), Some("bt2020"));
        assert_eq!(m.creation_time.as_deref(), Some("2024-05-01T12:04:05+0900"));
        assert_eq!(p.color_space.as_deref(), Some("bt2020nc"));
        assert_eq!(p.pix_fmt.as_deref(), Some("yuv420p10le"));
        assert_eq!(
            VideoColorHandling::from_metadata(m),
            VideoColorHandling::HdrNotToneMapped
        );
        assert_eq!(input_color_matrix(&p), "auto");
    }

    #[test]
    fn fallbacks_and_unknown_values() {
        let json = r#"{
            "streams": [{
                "index": 0, "codec_type": "video", "codec_name": "h264",
                "width": 1920, "height": 1080, "pix_fmt": "yuvj420p",
                "r_frame_rate": "30/1", "avg_frame_rate": "0/0",
                "color_transfer": "unknown", "duration": "N/A",
                "bits_per_raw_sample": "N/A",
                "tags": {"rotate": "90", "CREATION_TIME": "2020-01-01T00:00:00Z"}
            }],
            "format": {"duration": "N/A"}
        }"#;
        let p = parse_ffprobe_json(json.as_bytes()).unwrap();
        let m = &p.metadata;
        assert_eq!(m.fps, Some(30.0));
        assert_eq!(m.duration_s, None);
        assert_eq!(m.bit_depth, Some(8));
        assert_eq!(m.color_transfer, None);
        assert_eq!((m.width, m.height), (Some(1080), Some(1920)));
        assert_eq!(m.creation_time.as_deref(), Some("2020-01-01T00:00:00Z"));
        assert_eq!(p.color_space, None);
        assert_eq!(input_color_matrix(&p), "bt709");
        assert_eq!(
            VideoColorHandling::from_metadata(m),
            VideoColorHandling::Bt709
        );
        assert_eq!(thumbnail_position_s(m.duration_s), 0.0);
    }

    #[test]
    fn broken_json_is_an_error_not_a_panic() {
        for bad in [
            &b""[..],
            b"{",
            b"{\n\n}",
            b"null",
            b"[]",
            br#"{"streams": "x"}"#,
            br#"{"streams": [{"codec_type": "audio"}]}"#,
            br#"{"streams": [{"codec_type": "video", "width": -5, "height": 1e400}]}"#,
            br#"{"streams": [{"codec_type": "video", "r_frame_rate": "1/0", "side_data_list": [{"rotation": "abc"}]}]}"#,
        ] {
            match parse_ffprobe_json(bad) {
                Ok(p) => {
                    assert_eq!(p.metadata.width, None);
                    assert_eq!(p.metadata.fps, None);
                }
                Err(e) => assert!(matches!(e, MediaError::VideoProbe { .. }), "{e:?}"),
            }
        }
    }

    #[test]
    fn pix_fmt_bit_depths() {
        for (f, d) in [
            ("yuv420p", Some(8)),
            ("yuvj422p", Some(8)),
            ("yuv420p10le", Some(10)),
            ("yuv422p10be", Some(10)),
            ("yuv444p12le", Some(12)),
            ("yuva444p16le", Some(16)),
            ("gbrp", Some(8)),
            ("gbrp10le", Some(10)),
            ("p010le", Some(10)),
            ("p016le", Some(16)),
            ("nv12", Some(8)),
            ("gray", Some(8)),
            ("gray10le", Some(10)),
            ("gray16be", Some(16)),
            ("rgb48le", Some(16)),
            ("rgb24", Some(8)),
            ("x2rgb10le", Some(10)),
            ("something", None),
            ("", None),
        ] {
            assert_eq!(bit_depth_from_pix_fmt(f), d, "{f}");
        }
    }

    #[test]
    fn rotation_normalization() {
        for (deg, n) in [
            (0.0, 0),
            (90.0, 90),
            (-90.0, 270),
            (180.0, 180),
            (-180.0, 180),
            (270.0, 270),
            (450.0, 90),
            (89.6, 90),
        ] {
            assert_eq!(normalize_rotation(deg), n, "{deg}");
        }
    }

    #[test]
    fn positions_and_color_handling() {
        assert_eq!(thumbnail_position_s(None), 0.0);
        assert_eq!(thumbnail_position_s(Some(1.0)), 0.0);
        assert_eq!(thumbnail_position_s(Some(SHORT_VIDEO_MAX_S)), 0.0);
        assert!((thumbnail_position_s(Some(30.0)) - 3.0).abs() < 1e-12);
        assert_eq!(thumbnail_position_s(Some(f64::NAN)), 0.0);
        let m = VideoMetadata {
            color_primaries: Some("bt2020".to_owned()),
            color_transfer: Some("bt709".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            VideoColorHandling::from_metadata(&m),
            VideoColorHandling::PrimariesNotConverted
        );
    }

    #[test]
    fn tool_lookup() {
        let t = FfmpegTools::from_lookup(|k| match k {
            ENV_FFPROBE => Some(OsString::from("/opt/ff/ffprobe")),
            ENV_FFMPEG => Some(OsString::new()),
            _ => None,
        });
        assert_eq!(t.ffprobe, OsString::from("/opt/ff/ffprobe"));
        // 空の値は使わない。
        assert_eq!(t.ffmpeg, OsString::from("ffmpeg"));
        assert_eq!(t.probe_timeout, DEFAULT_PROBE_TIMEOUT);
        let missing =
            FfmpegTools::from_lookup(|_| Some(OsString::from("/nonexistent/genzo-ffmpeg-missing")));
        assert!(!missing.is_available());
        assert!(matches!(
            missing.probe(Path::new("x.mp4")),
            Err(MediaError::ToolSpawn { .. })
        ));
        assert_eq!(file_url(Path::new("-v.mp4")), OsString::from("file:-v.mp4"));
    }

    /// 偽の ffprobe / ffmpeg（シェルスクリプト）で、壊れた出力・失敗・時間切れを確かめる
    /// （本物の ffmpeg がなくても動く。「ffmpeg の出力が壊れていてもパニックしない」の確認）。
    #[cfg(unix)]
    mod fake_tools {
        use std::fs;
        use std::path::PathBuf;
        use std::time::Duration;

        use super::*;
        use crate::buffer::RgbImage8;

        /// 正常な ffprobe の出力（10 秒・64×48 の H.264）。
        const PROBE_JSON: &str = r#"{"streams":[{"index":0,"codec_type":"video","codec_name":"h264","width":64,"height":48,"pix_fmt":"yuv420p"}],"format":{"duration":"10.0"}}"#;

        /// 実行できるスクリプトを作る。`-version` にはすぐ成功する。
        ///
        /// 書いた直後の実行は、並行するテストの fork と重なると「Text file busy」で失敗することが
        /// あるため、実行できるようになるまで試す。
        fn script(dir: &Path, name: &str, body: &str) -> OsString {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            fs::write(
                &path,
                format!("#!/bin/sh\nif [ \"$1\" = -version ]; then exit 0; fi\n{body}\n"),
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            for _ in 0..200 {
                match std::process::Command::new(&path).arg("-version").status() {
                    Ok(s) if s.success() => return path.into_os_string(),
                    _ => std::thread::sleep(Duration::from_millis(10)),
                }
            }
            panic!("偽のツール {name} を実行できない");
        }

        /// シェルの一重引用符で囲む。
        fn quote(p: &Path) -> String {
            format!("'{}'", p.to_str().unwrap().replace('\'', r"'\''"))
        }

        fn tools(dir: &Path, probe_body: &str, ffmpeg_body: &str) -> FfmpegTools {
            FfmpegTools {
                ffprobe: script(dir, "ffprobe", probe_body),
                ffmpeg: script(dir, "ffmpeg", ffmpeg_body),
                probe_timeout: Duration::from_secs(10),
                frame_timeout: Duration::from_secs(10),
            }
        }

        /// ffprobe の正常な出力を返すスクリプトの本体。
        fn probe_ok(dir: &Path) -> String {
            let json = dir.join("probe.json");
            fs::write(&json, PROBE_JSON).unwrap();
            format!("cat {}", quote(&json))
        }

        /// 64×48 の PNG を書いたファイル。
        fn frame_png(dir: &Path) -> PathBuf {
            let img = RgbImage8::from_fn(64, 48, |x, _| [x as u8 * 4, 128, 64]).unwrap();
            let png = crate::png_io::encode_png(&img, None, None).unwrap();
            let path = dir.join("frame.png");
            fs::write(&path, png).unwrap();
            path
        }

        #[test]
        fn broken_ffprobe_outputs_are_errors() {
            let dir = tempfile::tempdir().unwrap();
            let video = dir.path().join("v.mp4");
            for (body, want_probe_error) in [
                ("printf 'not json at all'", true),
                ("printf '{\"streams\": ['", true),
                ("printf '{\"streams\": []}'", true),
                // 0 以外の終了コードは ToolFailed（標準エラー出力の末尾を含む）。
                ("printf 'moov atom not found' >&2; exit 1", false),
            ] {
                let t = tools(dir.path(), body, "exit 1");
                match t.probe(&video) {
                    Err(MediaError::VideoProbe { .. }) if want_probe_error => {}
                    Err(MediaError::ToolFailed { stderr, .. }) if !want_probe_error => {
                        assert!(stderr.contains("moov atom not found"), "{stderr}");
                    }
                    other => panic!("{body}: {other:?}"),
                }
                assert!(t.thumbnail(&video, CacheSpec::L0_THUMBNAIL).is_err());
            }
        }

        #[test]
        fn broken_ffmpeg_outputs_are_errors_not_panics() {
            let dir = tempfile::tempdir().unwrap();
            let video = dir.path().join("v.mp4");
            let probe = probe_ok(dir.path());
            // 巨大な寸法の IHDR だけを持つ PNG（デコードの前に寸法で拒否される）。
            let mut huge = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
            huge.extend_from_slice(&60_000u32.to_be_bytes());
            huge.extend_from_slice(&60_000u32.to_be_bytes());
            huge.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
            let huge_path = dir.path().join("huge.png");
            fs::write(&huge_path, &huge).unwrap();
            // 途中で切れた PNG。
            let good = fs::read(frame_png(dir.path())).unwrap();
            let cut_path = dir.path().join("cut.png");
            fs::write(&cut_path, &good[..good.len() / 2]).unwrap();
            for body in [
                // PNG ではない出力。
                "printf 'garbage garbage garbage'".to_owned(),
                format!("cat {}", quote(&huge_path)),
                format!("cat {}", quote(&cut_path)),
                // フレームを出さずに成功する（先頭でも出ない）。
                "exit 0".to_owned(),
                // 失敗する。
                "printf 'decoder error' >&2; exit 69".to_owned(),
            ] {
                let t = tools(dir.path(), &probe, &body);
                let r = t.thumbnail(&video, CacheSpec::L0_THUMBNAIL);
                assert!(
                    matches!(
                        r,
                        Err(MediaError::Decode { .. }
                            | MediaError::TooLarge { .. }
                            | MediaError::ToolFailed { .. })
                    ),
                    "{body}: {r:?}"
                );
            }
        }

        #[test]
        fn empty_output_at_the_position_retries_from_the_start() {
            // 長さの記録より実際が短い動画: 代表フレームの位置（-ss 付き）では何も出ず、先頭では
            // フレームが出る。
            let dir = tempfile::tempdir().unwrap();
            let video = dir.path().join("v.mp4");
            let frame = frame_png(dir.path());
            let body = format!(
                "for a in \"$@\"; do if [ \"$a\" = -ss ]; then exit 0; fi; done; cat {}",
                quote(&frame)
            );
            let t = tools(dir.path(), &probe_ok(dir.path()), &body);
            let thumb = t.thumbnail(&video, CacheSpec::L0_THUMBNAIL).unwrap();
            assert_eq!(thumb.position_ms, 0);
            assert_eq!((thumb.jpeg.width, thumb.jpeg.height), (64, 48));
            assert_eq!(thumb.color, VideoColorHandling::Bt709);

            // -ss の位置でもフレームが出るなら、長さの 10%（1 秒）の位置を使う。
            let t = tools(
                dir.path(),
                &probe_ok(dir.path()),
                &format!("cat {}", quote(&frame)),
            );
            let thumb = t.thumbnail(&video, CacheSpec::L0_THUMBNAIL).unwrap();
            assert_eq!(thumb.position_ms, 1000);
        }

        #[test]
        fn arguments_keep_the_input_as_a_file_url() {
            // 引数を 1 行ずつ書き出して、入力の指定と色の行列・出力の形式を確かめる。
            let dir = tempfile::tempdir().unwrap();
            let args_path = dir.path().join("args.txt");
            let frame = frame_png(dir.path());
            let body = format!(
                "for a in \"$@\"; do printf '%s\\n' \"$a\" >> {}; done; cat {}",
                quote(&args_path),
                quote(&frame)
            );
            let t = tools(dir.path(), &probe_ok(dir.path()), &body);
            let video = dir.path().join("-name with space.mp4");
            let probe = parse_ffprobe_json(PROBE_JSON.as_bytes()).unwrap();
            let png = t.extract_frame_png(&video, &probe, 1.0).unwrap();
            assert!(png.starts_with(b"\x89PNG"));
            let args = fs::read_to_string(&args_path).unwrap();
            let args: Vec<&str> = args.lines().collect();
            let input = format!("file:{}", video.display());
            let i = args.iter().position(|a| *a == "-i").unwrap();
            assert_eq!(args[i + 1], input);
            // -ss は入力の前（入力のシーク）。
            let ss = args.iter().position(|a| *a == "-ss").unwrap();
            assert!(ss < i);
            assert_eq!(args[ss + 1], "1.000");
            assert!(args.contains(&"scale=in_color_matrix=bt709,format=rgb24"));
            assert!(
                args.windows(2)
                    .any(|w| w == ["-protocol_whitelist", "file"])
            );
            assert!(args.windows(2).any(|w| w == ["-map", "0:0"]));
            assert_eq!(args.last(), Some(&"pipe:1"));
        }

        #[test]
        fn slow_ffmpeg_is_killed() {
            let dir = tempfile::tempdir().unwrap();
            let video = dir.path().join("v.mp4");
            let mut t = tools(dir.path(), &probe_ok(dir.path()), "exec sleep 30");
            t.frame_timeout = Duration::from_millis(300);
            let start = std::time::Instant::now();
            let r = t.thumbnail(&video, CacheSpec::L0_THUMBNAIL);
            assert!(matches!(r, Err(MediaError::ToolTimeout { .. })), "{r:?}");
            assert!(start.elapsed() < Duration::from_secs(10));
        }
    }
}
