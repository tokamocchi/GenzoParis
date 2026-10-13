//! 動画の再生用のファイル（プロキシ。VID-04、04 の 4.2 節）: WebView（OS の動画の機能）で再生できない
//! 動画（H.265・10bit・MPEG-4 Part 2 など）を、利用者が入れた ffmpeg（決定事項ログ No.62・67。同梱しない）で、
//! 再生しやすい形に変換する。
//!
//! - 形式: **H.264（8bit・4:2:0。`yuv420p`）＋ AAC（2ch）の MP4**（`+faststart`。先頭に索引を置き、読み込みの
//!   途中から再生・シークできる）。長辺は [`PROXY_LONG_EDGE`] 以下（拡大しない。寸法は偶数）。画素は正方形にし、
//!   表示上の縦横比（画素の縦横比（SAR）を反映したもの）は変えない（[`proxy_video_filter`]）。
//!   1 秒ごとにキーフレームを置く（シークを速くする）。字幕・データのストリーム・メタデータ・チャプターは
//!   入れない。音声は最初の 1 本だけ（なければ映像だけ）。
//! - エンコーダー: 利用者の ffmpeg にあるものから [`ProxyEncoder::PREFERENCE`] の順に選ぶ
//!   （[`pick_proxy_encoder`]。libx264 → macOS の VideoToolbox → Windows の Media Foundation → OpenH264）。
//!   どれもなければ作れない（[`MediaError::Unsupported`]）。
//! - 色: 行列・原色・伝達関数は変換せず、入力の解釈（[`crate::video`] のサムネイルと同じ。記録がなければ SD は
//!   BT.601、それ以外は BT.709）を出力に記録する（WebView が同じ行列で戻すため）。値の範囲は、**制限の範囲（`tv`）に
//!   揃える**（全域（フルレンジ。`yuvj420p`・`color_range=pc`）の入力は、縮小のフィルターで値を制限の範囲へ変換する。
//!   制限の範囲の入力は変えない。全域の H.264 の扱いは WebView・OS のデコーダーで揃っていないため。検証 R1）。
//!   10bit は 8bit に丸める。HDR（PQ・HLG）のトーンマッピング・BT.2020 の色域の変換・Log の LUT は later
//!   （VID-06・CLR-03。サムネイルと同じく、色が正しくない）。
//! - 進捗: `-progress pipe:1` の `out_time_us` などを読み、変換した位置（秒）を知らせる（[`parse_progress_line`]）。
//! - 安全: 入力は `file:` を付けて `-protocol_whitelist file` で開く（[`crate::video`] と同じ）。出力は
//!   呼び出し側が用意した一時ファイルに `-n`（上書きしない）で書く。出力が [`PROXY_IDLE_TIMEOUT`] の間
//!   途絶えたら止まったとみなして強制終了し、取り消しでも強制終了する（[`crate::process::run_streaming`]）。
//!   変換の後に、ffprobe で出力を確かめる（[`verify_proxy`]）。

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{MediaError, Result};
use crate::process::{run_streaming, run_with_timeout};
use crate::video::{FfmpegTools, VideoProbe, file_url, untagged_is_sd};

/// 再生用のファイルの長辺の上限（画素。**仮置き**: 1280。ルーペの画面で見るのに足り、ソフトウェアの変換でも
/// 実時間に近い速さで作れる大きさ）。
pub const PROXY_LONG_EDGE: u32 = 1280;

/// libx264 の品質（CRF。**仮置き**: 23 は libx264 の既定）。
pub const PROXY_X264_CRF: u32 = 23;

/// libx264 以外のエンコーダーの映像のビットレート（kbps。**仮置き**: 4000。長辺 1280 の H.264 で、
/// 動きの多い場面でも崩れにくい値として置いた）。
pub const PROXY_VIDEO_KBPS: u32 = 4000;

/// 音声のビットレート（kbps。**仮置き**: 128。AAC の 2ch）。
pub const PROXY_AUDIO_KBPS: u32 = 128;

/// 変換の進捗（ffmpeg の `-progress` の出力）がこの時間途絶えたら、止まったとみなして打ち切る
/// （**仮置き**: 120 秒。ffmpeg は既定で 0.5 秒ごとに書く。4K・10bit の HEVC の最初のフレームの準備や、
/// `+faststart` の書き直し（出力の大きさに比例）でも誤って打ち切らない長さ）。
pub const PROXY_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// 再生用のファイルの形式の版（変換の設定を変えたら上げる。キャッシュキーの材料）。
///
/// 2: 値の範囲を制限の範囲に揃え、画素の縦横比を反映した（検証 R1。版 1 のものは作り直す）。
pub const PROXY_FORMAT_VERSION: u32 = 2;

/// ffmpeg のエンコーダーの一覧（`-encoders`）を読む時間の上限。
const ENCODERS_TIMEOUT: Duration = Duration::from_secs(20);

/// ffmpeg のエンコーダーの一覧の大きさの上限（バイト）。
const MAX_ENCODERS_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

/// 出力の長さとして受け付ける、元の動画の長さに対する割合の下限（**仮置き**: 0.9。これより短ければ
/// 途中で終わったとみなす。音声と映像の長さの違い・最後のフレームの丸めの分は、さらに 0.5 秒を許す）。
const MIN_DURATION_RATIO: f64 = 0.9;

/// 再生用のファイルの H.264 のエンコーダー。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyEncoder {
    /// libx264（GPL。多くの ffmpeg の配布物に入っている）。
    Libx264,
    /// macOS の VideoToolbox（`h264_videotoolbox`）。
    VideoToolbox,
    /// Windows の Media Foundation（`h264_mf`）。
    MediaFoundation,
    /// Cisco の OpenH264（`libopenh264`）。
    OpenH264,
}

impl ProxyEncoder {
    /// 選ぶ順（[`pick_proxy_encoder`]）。品質が安定していて、どの OS でも同じ結果になる libx264 を先にする。
    pub const PREFERENCE: [Self; 4] = [
        Self::Libx264,
        Self::VideoToolbox,
        Self::MediaFoundation,
        Self::OpenH264,
    ];

    /// ffmpeg のエンコーダーの名前。
    pub const fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::Libx264 => "libx264",
            Self::VideoToolbox => "h264_videotoolbox",
            Self::MediaFoundation => "h264_mf",
            Self::OpenH264 => "libopenh264",
        }
    }

    /// 映像のエンコードの引数（`-c:v` から）。
    fn codec_args(self) -> Vec<String> {
        let mut a: Vec<String> = vec!["-c:v".into(), self.ffmpeg_name().into()];
        let bitrate = || vec!["-b:v".to_owned(), format!("{PROXY_VIDEO_KBPS}k")];
        match self {
            Self::Libx264 => {
                for s in ["-preset", "veryfast", "-crf"] {
                    a.push(s.into());
                }
                a.push(PROXY_X264_CRF.to_string());
                a.extend(["-profile:v".into(), "high".into()]);
            }
            Self::VideoToolbox => {
                a.extend(bitrate());
                // ハードウェアのエンコーダーが使えないとき（仮想マシンなど）は、OS のソフトウェアの実装を使う。
                a.extend(["-allow_sw".into(), "1".into()]);
                a.extend(["-profile:v".into(), "high".into()]);
            }
            Self::MediaFoundation | Self::OpenH264 => a.extend(bitrate()),
        }
        a.extend(["-pix_fmt".into(), "yuv420p".into()]);
        a
    }
}

/// `ffmpeg -encoders` の出力から、エンコーダーの名前を読む（`------` の行より後の、` V....D libx264 説明` の形の
/// 行の 2 番目の語。映像（`V`）・音声（`A`）・字幕（`S`）のすべて）。
pub fn parse_encoder_list(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_list = false;
    for line in text.lines() {
        let t = line.trim();
        if !in_list {
            in_list = t.starts_with("------");
            continue;
        }
        let mut words = t.split_whitespace();
        let (Some(flags), Some(name)) = (words.next(), words.next()) else {
            continue;
        };
        let kind_ok = flags.len() == 6 && matches!(flags.as_bytes()[0], b'V' | b'A' | b'S');
        if kind_ok && name.bytes().all(|b| b.is_ascii_graphic()) {
            out.push(name.to_owned());
        }
    }
    out
}

/// 使えるエンコーダーの名前の一覧から、再生用のファイルのエンコーダーを選ぶ（[`ProxyEncoder::PREFERENCE`] の順）。
/// どれもなければ `None`。
pub fn pick_proxy_encoder(available: &[String]) -> Option<ProxyEncoder> {
    ProxyEncoder::PREFERENCE
        .into_iter()
        .find(|e| available.iter().any(|n| n == e.ffmpeg_name()))
}

/// 変換に使う ffmpeg のスレッドの数の既定値（CPU のコア数 − 1。少なくとも 1）。画面の操作の余力を残す
/// （SCL-07）。
pub fn default_proxy_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(1)
        .max(1)
}

/// ffmpeg の色の名前（`bt709`・`smpte170m`・`arib-std-b67` など）として渡してよい文字列か（英小文字・数字・
/// `-` だけ。ffprobe の値をそのまま渡すので、念のため形を確かめる）。
fn color_name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-'))
}

/// 出力に記録する色の解釈（`-colorspace`・`-color_primaries`・`-color_trc`・`-color_range` の値）。
///
/// 行列・原色・伝達関数は入力の記録をそのまま使う。行列の記録がなければ、サムネイルと同じく SD（幅 1280 未満かつ
/// 高さ 576 以下）は BT.601（`smpte170m`）、それ以外は BT.709 とみなす。原色・伝達関数の記録がなければ、行列と
/// 同じ規格の値にする。値の範囲は、入力によらず制限の範囲（`tv`。[`proxy_video_filter`] が揃える）。
pub fn proxy_color_tags(probe: &VideoProbe) -> [(&'static str, String); 4] {
    let assumed = if untagged_is_sd(probe) {
        "smpte170m"
    } else {
        "bt709"
    };
    let pick = |v: Option<&str>| -> String {
        v.filter(|s| color_name_ok(s))
            .map_or_else(|| assumed.to_owned(), str::to_owned)
    };
    [
        ("-colorspace", pick(probe.color_space.as_deref())),
        (
            "-color_primaries",
            pick(probe.metadata.color_primaries.as_deref()),
        ),
        ("-color_trc", pick(probe.metadata.color_transfer.as_deref())),
        ("-color_range", "tv".to_owned()),
    ]
}

/// 入力が全域（フルレンジ）か（ffprobe の `color_range` が `pc`、または画素の形式が `yuvj` の JPEG の系統）。
pub fn input_is_full_range(probe: &VideoProbe) -> bool {
    probe.color_range.as_deref() == Some("pc")
        || probe
            .pix_fmt
            .as_deref()
            .is_some_and(|f| f.starts_with("yuvj"))
}

/// 縮小のフィルター（長辺を [`PROXY_LONG_EDGE`] 以下に。拡大はしない。寸法は偶数（少なくとも 2）。8bit の 4:2:0・
/// 制限の範囲にする）。
///
/// - 寸法: 表示上の幅（保存上の幅 `iw` × 画素の縦横比 `sar`。ffmpeg は SAR が記録されていなければ 1 とする）と
///   高さ `ih` で長い方を決め、長い方を上限まで縮め、短い方を表示上の縦横比のとおりに決めてから、画素を正方形に
///   する（`setsar=1`）。短い方は偶数に丸める（正方形の画素の 1920×1080・3840×2160・1080×1920 は、これまでと
///   同じ 1280×720・1280×720・720×1280 になることを ffmpeg 6.1 で確かめた）。回転は ffmpeg が
///   先に反映する（既定の autorotate。SAR も入れ替わる）ので、`iw`・`ih` は表示上の向き。
/// - 色: 行列の変換はしない（[`proxy_color_tags`] で入力の解釈を記録する）。値の範囲は、全域の入力
///   （[`input_is_full_range`]）を制限の範囲へ変換し、それ以外はフレームの記録のまま（記録がなければ制限の範囲）
///   として、制限の範囲で出す。
pub fn proxy_video_filter(probe: &VideoProbe) -> String {
    let e = PROXY_LONG_EDGE;
    let in_range = if input_is_full_range(probe) {
        "pc"
    } else {
        "auto"
    };
    let landscape = "gte(iw*sar,ih)";
    let w = format!(
        "if({landscape},max(2,trunc(min({e},iw*sar)/2)*2),max(2,2*round(trunc(min({e},ih)/2)*iw*sar/ih)))"
    );
    let h = format!(
        "if({landscape},max(2,2*round(trunc(min({e},iw*sar)/2)*ih/(iw*sar))),max(2,trunc(min({e},ih)/2)*2))"
    );
    format!("scale=w='{w}':h='{h}':in_range={in_range}:out_range=tv,setsar=1,format=yuv420p")
}

/// 再生用のファイルを作る ffmpeg の引数。
pub fn proxy_args(
    input: &Path,
    probe: &VideoProbe,
    output: &Path,
    encoder: ProxyEncoder,
    threads: usize,
) -> Vec<OsString> {
    let threads = threads.max(1).to_string();
    let mut a: Vec<OsString> = Vec::new();
    let mut push = |s: &str| a.push(OsString::from(s));
    for s in [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-nostats",
        "-progress",
        "pipe:1",
        "-protocol_whitelist",
        "file",
        "-threads",
        &threads,
        "-i",
    ] {
        push(s);
    }
    a.push(file_url(input));
    let mut rest: Vec<String> = vec![
        "-map".into(),
        format!("0:{}", probe.stream_index),
        "-map".into(),
        "0:a:0?".into(),
        "-sn".into(),
        "-dn".into(),
        "-map_metadata".into(),
        "-1".into(),
        "-map_chapters".into(),
        "-1".into(),
        "-vf".into(),
        proxy_video_filter(probe),
        "-force_key_frames".into(),
        "expr:gte(t,n_forced*1)".into(),
    ];
    rest.extend(encoder.codec_args());
    for (flag, value) in proxy_color_tags(probe) {
        rest.push(flag.into());
        rest.push(value);
    }
    rest.extend([
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        format!("{PROXY_AUDIO_KBPS}k"),
        "-ac".into(),
        "2".into(),
        "-movflags".into(),
        "+faststart".into(),
        "-max_muxing_queue_size".into(),
        "1024".into(),
        "-threads".into(),
        threads.clone(),
        "-f".into(),
        "mp4".into(),
        // 一時ファイルは呼び出し側が新しい名前で用意する。万一あっても上書きしない。
        "-n".into(),
    ]);
    a.extend(rest.into_iter().map(OsString::from));
    a.push(file_url(output));
    a
}

/// `-progress` の 1 行（`out_time_us=1234567` など）から、変換した位置（秒）を読む。進捗の行でなければ
/// `None`（`N/A` も `None`）。
///
/// `out_time_us`（マイクロ秒）・`out_time_ms`（名前に反してマイクロ秒。ffmpeg の古くからの仕様）・
/// `out_time`（`HH:MM:SS.micro`）のどれでも読む。
pub fn parse_progress_line(line: &str) -> Option<f64> {
    let (key, value) = line.trim().split_once('=')?;
    let value = value.trim();
    match key.trim() {
        "out_time_us" | "out_time_ms" => {
            let us: i64 = value.parse().ok()?;
            (us >= 0).then(|| us as f64 / 1_000_000.0)
        }
        "out_time" => {
            let mut parts = value.split(':');
            let (h, m, s) = (parts.next()?, parts.next()?, parts.next()?);
            if parts.next().is_some() {
                return None;
            }
            let h: f64 = h.parse().ok()?;
            let m: f64 = m.parse().ok()?;
            let s: f64 = s.parse().ok()?;
            let t = h * 3600.0 + m * 60.0 + s;
            (t.is_finite() && t >= 0.0 && !value.starts_with('-')).then_some(t)
        }
        _ => None,
    }
}

/// 作った再生用のファイルを確かめる（ffprobe で読み、H.264 の映像があり、長辺が [`PROXY_LONG_EDGE`] 以下で、
/// 長さが元の動画の長さ（分かれば）の 0.9 倍（`MIN_DURATION_RATIO`）− 0.5 秒以上あるか）。
pub fn verify_proxy(probe: &VideoProbe, source_duration_s: Option<f64>) -> Result<()> {
    let m = &probe.metadata;
    if m.codec.as_deref() != Some("h264") {
        return Err(MediaError::decode(format!(
            "再生用のファイルの映像が H.264 ではない（{}）",
            m.codec.as_deref().unwrap_or("不明")
        )));
    }
    if let (Some(w), Some(h)) = (m.width, m.height)
        && w.max(h) > PROXY_LONG_EDGE
    {
        return Err(MediaError::decode(format!(
            "再生用のファイルの寸法が大きすぎる（{w}×{h}）"
        )));
    }
    if let Some(src) = source_duration_s.filter(|d| d.is_finite() && *d > 0.0) {
        let got = m.duration_s.unwrap_or(0.0);
        let min = src * MIN_DURATION_RATIO - 0.5;
        if got < min {
            return Err(MediaError::decode(format!(
                "再生用のファイルが短すぎる（{got:.2} 秒。元の動画は {src:.2} 秒）"
            )));
        }
    }
    Ok(())
}

impl FfmpegTools {
    /// ffmpeg のエンコーダーの名前の一覧（`ffmpeg -hide_banner -encoders`）。
    pub fn encoders(&self) -> Result<Vec<String>> {
        let out = run_with_timeout(
            &self.ffmpeg,
            &["-hide_banner", "-encoders"],
            ENCODERS_TIMEOUT,
            MAX_ENCODERS_OUTPUT_BYTES,
        )?;
        Self::check_status(&self.ffmpeg, &out)?;
        Ok(parse_encoder_list(&String::from_utf8_lossy(&out.stdout)))
    }

    /// 再生用のファイルを作る（`input` の動画を、ffprobe の結果 `probe` を使って `output` に書く）。
    ///
    /// `output` は呼び出し側が用意した、まだない一時ファイルの名前（拡張子は問わない。形式は `-f mp4` で
    /// 指定する）。失敗・取り消しでは、ffmpeg が書きかけた `output` を消す。変換した位置（秒）を `progress`
    /// で知らせる。書き終えた後の確かめ（[`verify_proxy`]）は呼び出し側が行う（ffprobe を使うため）。
    #[allow(clippy::too_many_arguments)]
    pub fn transcode_proxy(
        &self,
        input: &Path,
        probe: &VideoProbe,
        output: &Path,
        encoder: ProxyEncoder,
        threads: usize,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(f64),
    ) -> Result<()> {
        let args = proxy_args(input, probe, output, encoder, threads);
        let r = run_streaming(
            &self.ffmpeg,
            &args,
            PROXY_IDLE_TIMEOUT,
            cancelled,
            &mut |line| {
                if let Some(t) = parse_progress_line(line) {
                    progress(t);
                }
            },
        )
        .and_then(|out| {
            if out.status.success() {
                Ok(())
            } else {
                Err(MediaError::ToolFailed {
                    program: self.ffmpeg.to_string_lossy().into_owned(),
                    status: out.status.to_string(),
                    stderr: out.stderr_tail.trim().to_owned(),
                })
            }
        });
        if r.is_err() {
            // 書きかけの一時ファイル（ffmpeg が作ったもの）を消す。Windows で ffmpeg の終了の直後にまだ開かれて
            // いれば消せないが、呼び出し側の回収（古い一時ファイルの削除）で消える。
            let _ = std::fs::remove_file(output);
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use genzo_model::VideoMetadata;

    const ENCODERS: &str = "Encoders:
 V..... = Video
 A..... = Audio
 S..... = Subtitle
 .F.... = Frame-level multithreading
 ------
 V....D a64multi             Multicolor charset for Commodore 64 (codec a64_multi)
 V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 (codec h264)
 V....D h264_videotoolbox    VideoToolbox H.264 Encoder (codec h264)
 A....D aac                  AAC (Advanced Audio Coding)
 S..... ass                  ASS (Advanced SubStation Alpha) subtitle
";

    fn probe() -> VideoProbe {
        VideoProbe {
            metadata: VideoMetadata {
                duration_s: Some(10.0),
                codec: Some("hevc".into()),
                ..Default::default()
            },
            stream_index: 2,
            coded_size: Some((3840, 2160)),
            ..Default::default()
        }
    }

    #[test]
    fn encoder_list_and_preference() {
        let names = parse_encoder_list(ENCODERS);
        assert_eq!(
            names,
            ["a64multi", "libx264", "h264_videotoolbox", "aac", "ass"]
        );
        // 見出し（`------` の前）の V..... = Video は読まない。
        assert!(!names.iter().any(|n| n == "="));
        assert_eq!(pick_proxy_encoder(&names), Some(ProxyEncoder::Libx264));
        let mac = vec!["h264_videotoolbox".to_owned(), "aac".to_owned()];
        assert_eq!(pick_proxy_encoder(&mac), Some(ProxyEncoder::VideoToolbox));
        let win = vec!["h264_mf".to_owned(), "libopenh264".to_owned()];
        assert_eq!(
            pick_proxy_encoder(&win),
            Some(ProxyEncoder::MediaFoundation)
        );
        let none = vec!["mpeg4".to_owned(), "aac".to_owned()];
        assert_eq!(pick_proxy_encoder(&none), None);
        assert!(parse_encoder_list("").is_empty());
        assert!(parse_encoder_list("壊れた出力\n------\n???").is_empty());
    }

    #[test]
    fn progress_lines() {
        assert_eq!(parse_progress_line("out_time_us=1500000"), Some(1.5));
        assert_eq!(parse_progress_line("out_time_ms=2500000"), Some(2.5));
        assert_eq!(parse_progress_line("out_time=00:01:02.500000"), Some(62.5));
        assert_eq!(parse_progress_line("out_time_us=N/A"), None);
        assert_eq!(parse_progress_line("out_time=-577014:32:22.775808"), None);
        assert_eq!(
            parse_progress_line("out_time_us=-9223372036854775807"),
            None
        );
        assert_eq!(parse_progress_line("frame=12"), None);
        assert_eq!(parse_progress_line("progress=end"), None);
        assert_eq!(parse_progress_line("壊れた行"), None);
    }

    #[test]
    fn color_tags_follow_the_input_or_the_thumbnail_rule() {
        // 記録がなく HD 以上: BT.709。
        let tags = proxy_color_tags(&probe());
        assert_eq!(tags[0], ("-colorspace", "bt709".to_owned()));
        assert_eq!(tags[1], ("-color_primaries", "bt709".to_owned()));
        assert_eq!(tags[2], ("-color_trc", "bt709".to_owned()));
        assert_eq!(tags[3], ("-color_range", "tv".to_owned()));
        // 記録がなく SD: BT.601（smpte170m）。
        let mut sd = probe();
        sd.coded_size = Some((720, 480));
        assert!(
            proxy_color_tags(&sd)
                .iter()
                .take(3)
                .all(|(_, v)| v == "smpte170m")
        );
        // 記録があればそのまま（HLG の BT.2020）。形の違う値は使わない。値の範囲は、全域の入力でも制限の範囲
        // （フィルターが変換する。検証 R1）。
        let mut hdr = probe();
        hdr.color_space = Some("bt2020nc".into());
        hdr.metadata.color_primaries = Some("bt2020".into());
        hdr.metadata.color_transfer = Some("arib-std-b67".into());
        hdr.color_range = Some("pc".into());
        let tags = proxy_color_tags(&hdr);
        assert_eq!(tags[0].1, "bt2020nc");
        assert_eq!(tags[1].1, "bt2020");
        assert_eq!(tags[2].1, "arib-std-b67");
        assert_eq!(tags[3].1, "tv");
        let mut odd = probe();
        odd.color_space = Some("bt709 -y".into());
        assert_eq!(proxy_color_tags(&odd)[0].1, "bt709");
    }

    #[test]
    fn arguments_keep_files_as_file_urls_and_do_not_overwrite() {
        let args: Vec<String> = proxy_args(
            Path::new("-in.mov"),
            &probe(),
            Path::new("/cache/.k.tmp"),
            ProxyEncoder::Libx264,
            3,
        )
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
        let pos = |s: &str| args.iter().position(|a| a == s).unwrap();
        assert_eq!(args[pos("-i") + 1], "file:-in.mov");
        assert!(pos("-protocol_whitelist") < pos("-i"));
        assert_eq!(args.last().unwrap(), "file:/cache/.k.tmp");
        assert_eq!(args[args.len() - 2], "-n");
        assert_eq!(args[pos("-map") + 1], "0:2");
        assert!(args.contains(&"0:a:0?".to_owned()));
        assert_eq!(args[pos("-c:v") + 1], "libx264");
        assert_eq!(args[pos("-pix_fmt") + 1], "yuv420p");
        assert_eq!(args[pos("-f") + 1], "mp4");
        assert_eq!(args[pos("-progress") + 1], "pipe:1");
        assert!(args.iter().filter(|a| *a == "-threads").count() == 2);
        assert!(args[pos("-vf") + 1].contains("min(1280,iw*sar)"));
        assert_eq!(args[pos("-color_range") + 1], "tv");
        for e in ProxyEncoder::PREFERENCE {
            let a = e.codec_args();
            assert_eq!(a[1], e.ffmpeg_name());
            assert!(a.ends_with(&["-pix_fmt".to_owned(), "yuv420p".to_owned()]));
        }
        assert!(default_proxy_threads() >= 1);
    }

    /// 値の範囲: 全域の入力（`color_range=pc`・`yuvj` の画素の形式）は全域として読み、出力は制限の範囲（検証 R1）。
    /// 寸法は表示上の幅（`iw*sar`）で決め、画素を正方形にする。
    #[test]
    fn filter_reads_full_range_inputs_and_keeps_the_display_aspect() {
        let limited = probe();
        assert!(!input_is_full_range(&limited));
        let f = proxy_video_filter(&limited);
        assert!(f.contains(":in_range=auto:out_range=tv,"), "{f}");
        assert!(f.ends_with(",setsar=1,format=yuv420p"), "{f}");
        assert!(f.contains("gte(iw*sar,ih)") && !f.contains("-2"), "{f}");
        let mut tagged = probe();
        tagged.color_range = Some("pc".into());
        let mut jpeg = probe();
        jpeg.pix_fmt = Some("yuvj420p".into());
        for p in [tagged, jpeg] {
            assert!(input_is_full_range(&p), "{p:?}");
            assert!(
                proxy_video_filter(&p).contains(":in_range=pc:out_range=tv,"),
                "{p:?}"
            );
        }
        let mut tv = probe();
        tv.color_range = Some("tv".into());
        tv.pix_fmt = Some("yuv420p10le".into());
        assert!(!input_is_full_range(&tv));
    }

    #[test]
    fn verification_of_the_output() {
        let mut out = probe();
        out.metadata.codec = Some("h264".into());
        out.metadata.width = Some(1280);
        out.metadata.height = Some(720);
        out.metadata.duration_s = Some(9.9);
        assert!(verify_proxy(&out, Some(10.0)).is_ok());
        assert!(verify_proxy(&out, None).is_ok());
        let mut short = out.clone();
        short.metadata.duration_s = Some(5.0);
        assert!(verify_proxy(&short, Some(10.0)).is_err());
        // 長さの記録がない出力は、元の長さが分かれば短すぎるとみなす。
        let mut unknown = out.clone();
        unknown.metadata.duration_s = None;
        assert!(verify_proxy(&unknown, Some(10.0)).is_err());
        assert!(verify_proxy(&unknown, Some(0.3)).is_ok());
        let mut big = out.clone();
        big.metadata.width = Some(1920);
        assert!(verify_proxy(&big, Some(10.0)).is_err());
        let mut hevc = out;
        hevc.metadata.codec = Some("hevc".into());
        assert!(verify_proxy(&hevc, Some(10.0)).is_err());
    }
}
