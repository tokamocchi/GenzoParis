//! 写真・動画のメタデータ（LIB-14、PoC-7）。
//!
//! ワーカーがファイルから読み取り、本体がカタログ（`asset`・`video_meta` テーブル）に
//! 保存する。読み取れなかった項目は `None` にする。

use serde::{Deserialize, Serialize};

/// GPS の座標（WGS 84、10 進数の度）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GpsCoord {
    /// 緯度（-90〜90。北が正）。
    pub lat: f64,
    /// 経度（-180〜180。東が正）。
    pub lon: f64,
}

impl GpsCoord {
    /// 範囲内の有限の値なら座標を作る。
    pub fn new(lat: f64, lon: f64) -> Option<Self> {
        let c = Self { lat, lon };
        c.is_valid().then_some(c)
    }

    /// 範囲内の有限の値か。
    pub fn is_valid(&self) -> bool {
        self.lat.is_finite()
            && self.lon.is_finite()
            && (-90.0..=90.0).contains(&self.lat)
            && (-180.0..=180.0).contains(&self.lon)
    }
}

/// EXIF の向き（Orientation タグ。1〜8）。
///
/// JSON・DB では EXIF の値（整数）で表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
#[repr(u8)]
pub enum Orientation {
    /// 1: そのまま。
    #[default]
    Normal = 1,
    /// 2: 左右反転。
    FlipHorizontal = 2,
    /// 3: 180 度回転。
    Rotate180 = 3,
    /// 4: 上下反転。
    FlipVertical = 4,
    /// 5: 左上と右下を結ぶ対角線で反転（転置）。
    Transpose = 5,
    /// 6: 時計回りに 90 度回転して表示する。
    Rotate90Cw = 6,
    /// 7: 右上と左下を結ぶ対角線で反転。
    Transverse = 7,
    /// 8: 反時計回りに 90 度回転して表示する（時計回りに 270 度）。
    Rotate270Cw = 8,
}

impl Orientation {
    /// EXIF の値から作る。1〜8 以外は `None`。
    pub const fn from_exif(value: u16) -> Option<Self> {
        Some(match value {
            1 => Self::Normal,
            2 => Self::FlipHorizontal,
            3 => Self::Rotate180,
            4 => Self::FlipVertical,
            5 => Self::Transpose,
            6 => Self::Rotate90Cw,
            7 => Self::Transverse,
            8 => Self::Rotate270Cw,
            _ => return None,
        })
    }

    /// EXIF の値（1〜8）を返す。
    pub const fn to_exif(self) -> u8 {
        self as u8
    }

    /// 表示するときに幅と高さが入れ替わるか（90 度・270 度の回転を含むか）。
    pub const fn swaps_dimensions(self) -> bool {
        matches!(
            self,
            Self::Transpose | Self::Rotate90Cw | Self::Transverse | Self::Rotate270Cw
        )
    }
}

impl TryFrom<u8> for Orientation {
    type Error = crate::ParseEnumError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::from_exif(u16::from(value)).ok_or_else(|| crate::ParseEnumError {
            type_name: "Orientation",
            value: value.to_string(),
        })
    }
}

impl From<Orientation> for u8 {
    fn from(o: Orientation) -> Self {
        o.to_exif()
    }
}

/// ファイルに記録された撮影日時（解析する前の文字列）。
///
/// [`CaptureTime::from_capture_info`](crate::CaptureTime::from_capture_info) で UTC を求める。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureInfo {
    /// 元の日時の文字列（EXIF の DateTimeOriginal（小数秒を付けたものを含む）、ISO 8601 など）。
    pub datetime: Option<String>,
    /// 元のオフセットの文字列（EXIF の OffsetTimeOriginal など）。
    pub offset: Option<String>,
}

/// 写真のメタデータ（`asset` テーブルの撮影情報に対応する）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PhotoMetadata {
    /// カメラのメーカー（例: `"SONY"`）。
    pub make: Option<String>,
    /// カメラの機種（例: `"ILCE-7M4"`）。
    pub model: Option<String>,
    /// レンズ名。
    pub lens: Option<String>,
    /// ISO 感度。
    pub iso: Option<u32>,
    /// 絞り値（F 値）。
    pub aperture: Option<f32>,
    /// シャッター速度（秒）。
    pub shutter_s: Option<f32>,
    /// 焦点距離（mm）。
    pub focal_mm: Option<f32>,
    /// 画像の幅（画素。向きを適用する前）。
    pub width: Option<u32>,
    /// 画像の高さ（画素。向きを適用する前）。
    pub height: Option<u32>,
    /// 向き。
    pub orientation: Orientation,
    /// GPS の座標。
    pub gps: Option<GpsCoord>,
    /// 撮影日時（元の文字列とオフセット）。
    pub capture: CaptureInfo,
}

impl PhotoMetadata {
    /// カタログの `asset.camera` に入れる表示名（メーカーと機種）。
    ///
    /// 機種名がメーカー名で始まる場合は重ねない（例: `"Canon"` ＋ `"Canon EOS R5"`）。
    pub fn camera_name(&self) -> Option<String> {
        let make = self
            .make
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let model = self
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match (make, model) {
            (Some(make), Some(model)) => {
                if model.to_lowercase().starts_with(&make.to_lowercase()) {
                    Some(model.to_owned())
                } else {
                    Some(format!("{make} {model}"))
                }
            }
            (Some(one), None) | (None, Some(one)) => Some(one.to_owned()),
            (None, None) => None,
        }
    }

    /// 向きを適用した表示上の寸法（幅, 高さ）。
    pub fn display_size(&self) -> Option<(u32, u32)> {
        let (w, h) = (self.width?, self.height?);
        Some(if self.orientation.swaps_dimensions() {
            (h, w)
        } else {
            (w, h)
        })
    }
}

/// 動画のメタデータ（`video_meta` テーブル。ffprobe で取得する。PoC-7）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoMetadata {
    /// 長さ（秒）。
    pub duration_s: Option<f64>,
    /// フレームレート（fps）。
    pub fps: Option<f64>,
    /// コーデック（ffprobe の codec_name。例: `"hevc"`）。
    pub codec: Option<String>,
    /// ビット深度。
    pub bit_depth: Option<u32>,
    /// 伝達関数（ffprobe の color_transfer。例: `"bt709"`、`"arib-std-b67"`）。
    pub color_transfer: Option<String>,
    /// 原色（ffprobe の color_primaries。例: `"bt709"`、`"bt2020"`）。
    pub color_primaries: Option<String>,
    /// 幅（画素）。
    pub width: Option<u32>,
    /// 高さ（画素）。
    pub height: Option<u32>,
    /// 作成日時（ffprobe の creation_time などの元の文字列）。
    pub creation_time: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gps_coord_validation() {
        assert!(GpsCoord::new(35.68, 139.76).is_some());
        assert!(GpsCoord::new(-90.0, 180.0).is_some());
        assert!(GpsCoord::new(90.1, 0.0).is_none());
        assert!(GpsCoord::new(0.0, -180.5).is_none());
        assert!(GpsCoord::new(f64::NAN, 0.0).is_none());
    }

    #[test]
    fn orientation_round_trip() {
        for v in 1..=8u16 {
            let o = Orientation::from_exif(v).unwrap();
            assert_eq!(u16::from(o.to_exif()), v);
            let json = serde_json::to_string(&o).unwrap();
            assert_eq!(json, v.to_string());
            assert_eq!(serde_json::from_str::<Orientation>(&json).unwrap(), o);
        }
        assert_eq!(Orientation::from_exif(0), None);
        assert_eq!(Orientation::from_exif(9), None);
        assert!(serde_json::from_str::<Orientation>("9").is_err());
        assert!(Orientation::Rotate90Cw.swaps_dimensions());
        assert!(!Orientation::Rotate180.swaps_dimensions());
    }

    #[test]
    fn camera_name_and_display_size() {
        let mut m = PhotoMetadata {
            make: Some("SONY".to_owned()),
            model: Some("ILCE-7M4".to_owned()),
            width: Some(7008),
            height: Some(4672),
            orientation: Orientation::Rotate90Cw,
            ..Default::default()
        };
        assert_eq!(m.camera_name().as_deref(), Some("SONY ILCE-7M4"));
        assert_eq!(m.display_size(), Some((4672, 7008)));
        m.make = Some("Canon".to_owned());
        m.model = Some("Canon EOS R5".to_owned());
        assert_eq!(m.camera_name().as_deref(), Some("Canon EOS R5"));
        m.make = None;
        assert_eq!(m.camera_name().as_deref(), Some("Canon EOS R5"));
        m.model = Some("  ".to_owned());
        assert_eq!(m.camera_name(), None);
    }

    #[test]
    fn metadata_json_defaults() {
        let m: PhotoMetadata = serde_json::from_str(r#"{"iso": 100}"#).unwrap();
        assert_eq!(m.iso, Some(100));
        assert_eq!(m.orientation, Orientation::Normal);
        let v: VideoMetadata =
            serde_json::from_str(r#"{"codec": "hevc", "bit_depth": 10}"#).unwrap();
        assert_eq!(v.codec.as_deref(), Some("hevc"));
        assert_eq!(v.bit_depth, Some(10));
        assert_eq!(v.duration_s, None);
    }
}
