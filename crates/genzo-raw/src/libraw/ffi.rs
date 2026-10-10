//! シム（`src/shim/genzo_libraw_shim.h`）の C の API の宣言。
//!
//! 構造体はヘッダの定義と同じ並び・同じ型で `#[repr(C)]` にする。大きさは
//! [`genzo_lr_abi_check`] で実行時に照合する（[`super::Processor::new`]）。

use std::ffi::{c_char, c_void};

/// ヘッダの `GENZO_LR_CBLACK_SIZE`（LibRaw の `LIBRAW_CBLACK_SIZE`）。
pub(crate) const CBLACK_SIZE: usize = 4104;
/// ヘッダの `GENZO_LR_CFA_ROWS`。
pub(crate) const CFA_ROWS: usize = 8;
/// ヘッダの `GENZO_LR_CFA_COLS`。
pub(crate) const CFA_COLS: usize = 2;

/// シム独自のエラーコード（ヘッダの `GENZO_LR_E_*`）。
pub(crate) mod code {
    pub(crate) const NULL_ARG: i32 = -200_001;
    pub(crate) const EXCEPTION: i32 = -200_002;
    pub(crate) const BAD_ALLOC: i32 = -200_003;
    pub(crate) const NO_BAYER_DATA: i32 = -200_004;
    pub(crate) const BAD_GEOMETRY: i32 = -200_005;
    pub(crate) const BUFFER_SIZE: i32 = -200_006;
    pub(crate) const NO_THUMB_DATA: i32 = -200_007;
    pub(crate) const ABI_MISMATCH: i32 = -200_008;
}

/// LibRaw のエラーコード（`libraw_const.h` の `LibRaw_errors`）。
pub(crate) mod libraw_code {
    pub(crate) const SUCCESS: i32 = 0;
    pub(crate) const UNSPECIFIED_ERROR: i32 = -1;
    pub(crate) const FILE_UNSUPPORTED: i32 = -2;
    pub(crate) const REQUEST_FOR_NONEXISTENT_IMAGE: i32 = -3;
    pub(crate) const OUT_OF_ORDER_CALL: i32 = -4;
    pub(crate) const NO_THUMBNAIL: i32 = -5;
    pub(crate) const UNSUPPORTED_THUMBNAIL: i32 = -6;
    pub(crate) const INPUT_CLOSED: i32 = -7;
    pub(crate) const NOT_IMPLEMENTED: i32 = -8;
    pub(crate) const REQUEST_FOR_NONEXISTENT_THUMBNAIL: i32 = -9;
    pub(crate) const UNSUFFICIENT_MEMORY: i32 = -100_007;
    pub(crate) const DATA_ERROR: i32 = -100_008;
    pub(crate) const IO_ERROR: i32 = -100_009;
    pub(crate) const CANCELLED_BY_CALLBACK: i32 = -100_010;
    pub(crate) const BAD_CROP: i32 = -100_011;
    pub(crate) const TOO_BIG: i32 = -100_012;
    pub(crate) const MEMPOOL_OVERFLOW: i32 = -100_013;
}

/// シムのインスタンス（不透明な型）。
#[repr(C)]
pub(crate) struct GenzoLr {
    _private: [u8; 0],
}

/// `genzo_lr_sizes`。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Sizes {
    pub raw_width: u32,
    pub raw_height: u32,
    pub width: u32,
    pub height: u32,
    pub top_margin: u32,
    pub left_margin: u32,
    pub raw_pitch: u32,
    pub flip: i32,
    pub filters: u32,
    pub colors: i32,
    pub dng_version: u32,
    pub is_foveon: u32,
    pub fuji_rotated: u32,
    pub raw_count: u32,
    pub cfa: [[i32; CFA_COLS]; CFA_ROWS],
    pub cdesc: [c_char; 8],
}

/// `genzo_lr_dng_color`。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct DngColor {
    pub illuminant: u32,
    pub colormatrix: [[f32; 3]; 4],
    pub calibration: [[f32; 4]; 4],
}

/// `genzo_lr_color`。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Color {
    pub black: u32,
    pub maximum: u32,
    pub cblack: [u32; CBLACK_SIZE],
    pub linear_max: [i64; 4],
    pub cam_mul: [f32; 4],
    pub pre_mul: [f32; 4],
    pub cam_xyz: [[f32; 3]; 4],
    pub rgb_cam: [[f32; 4]; 3],
    pub cmatrix: [[f32; 4]; 3],
    pub dng_color: [DngColor; 2],
    pub dng_analogbalance: [f32; 4],
    pub raw_bps: u32,
    pub reserved: u32,
}

/// `genzo_lr_meta`。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Meta {
    pub make: [c_char; 64],
    pub model: [c_char; 64],
    pub normalized_make: [c_char; 64],
    pub normalized_model: [c_char; 64],
    pub software: [c_char; 64],
    pub exif_make: [c_char; 64],
    pub exif_model: [c_char; 64],
    pub lens: [c_char; 128],
    pub makernotes_lens: [c_char; 128],
    pub unique_camera_model: [c_char; 64],
    pub iso_speed: f32,
    pub shutter: f32,
    pub aperture: f32,
    pub focal_len: f32,
    pub timestamp: i64,
    pub timestamp_local: [c_char; 32],
    pub datetime_original: [c_char; 64],
    pub subsec_time_original: [c_char; 32],
    pub offset_time_original: [c_char; 32],
    pub gps_parsed: i32,
    pub gps_latitude: [f32; 3],
    pub gps_longitude: [f32; 3],
    pub gps_altitude: f32,
    pub gps_latref: c_char,
    pub gps_longref: c_char,
    pub gps_altref: c_char,
    pub gps_pad: c_char,
    pub flip: i32,
    pub width: u32,
    pub height: u32,
}

/// `genzo_lr_thumb`。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct Thumb {
    pub format: i32,
    pub width: u32,
    pub height: u32,
    pub length: u32,
    pub colors: i32,
}

impl Sizes {
    /// すべて 0 の値（シムが上書きする前の初期値）。
    pub(crate) const fn zeroed() -> Self {
        Self {
            raw_width: 0,
            raw_height: 0,
            width: 0,
            height: 0,
            top_margin: 0,
            left_margin: 0,
            raw_pitch: 0,
            flip: 0,
            filters: 0,
            colors: 0,
            dng_version: 0,
            is_foveon: 0,
            fuji_rotated: 0,
            raw_count: 0,
            cfa: [[0; CFA_COLS]; CFA_ROWS],
            cdesc: [0; 8],
        }
    }
}

impl Color {
    /// すべて 0 の値。
    pub(crate) const fn zeroed() -> Self {
        const DNG_ZERO: DngColor = DngColor {
            illuminant: 0,
            colormatrix: [[0.0; 3]; 4],
            calibration: [[0.0; 4]; 4],
        };
        Self {
            black: 0,
            maximum: 0,
            cblack: [0; CBLACK_SIZE],
            linear_max: [0; 4],
            cam_mul: [0.0; 4],
            pre_mul: [0.0; 4],
            cam_xyz: [[0.0; 3]; 4],
            rgb_cam: [[0.0; 4]; 3],
            cmatrix: [[0.0; 4]; 3],
            dng_color: [DNG_ZERO; 2],
            dng_analogbalance: [0.0; 4],
            raw_bps: 0,
            reserved: 0,
        }
    }
}

impl Meta {
    /// すべて 0 の値。
    pub(crate) const fn zeroed() -> Self {
        Self {
            make: [0; 64],
            model: [0; 64],
            normalized_make: [0; 64],
            normalized_model: [0; 64],
            software: [0; 64],
            exif_make: [0; 64],
            exif_model: [0; 64],
            lens: [0; 128],
            makernotes_lens: [0; 128],
            unique_camera_model: [0; 64],
            iso_speed: 0.0,
            shutter: 0.0,
            aperture: 0.0,
            focal_len: 0.0,
            timestamp: 0,
            timestamp_local: [0; 32],
            datetime_original: [0; 64],
            subsec_time_original: [0; 32],
            offset_time_original: [0; 32],
            gps_parsed: 0,
            gps_latitude: [0.0; 3],
            gps_longitude: [0.0; 3],
            gps_altitude: 0.0,
            gps_latref: 0,
            gps_longref: 0,
            gps_altref: 0,
            gps_pad: 0,
            flip: 0,
            width: 0,
            height: 0,
        }
    }
}

unsafe extern "C" {
    pub(crate) safe fn genzo_lr_version() -> *const c_char;
    pub(crate) safe fn genzo_lr_version_number() -> i32;
    pub(crate) safe fn genzo_lr_header_version_number() -> i32;
    pub(crate) safe fn genzo_lr_abi_check(
        sizes_size: usize,
        color_size: usize,
        meta_size: usize,
        thumb_size: usize,
    ) -> i32;
    pub(crate) safe fn genzo_lr_strerror(code: i32) -> *const c_char;

    pub(crate) safe fn genzo_lr_new() -> *mut GenzoLr;
    pub(crate) fn genzo_lr_free(h: *mut GenzoLr);

    pub(crate) fn genzo_lr_open_file(h: *mut GenzoLr, path: *const c_char) -> i32;
    #[cfg(windows)]
    pub(crate) fn genzo_lr_open_wfile(h: *mut GenzoLr, path: *const u16) -> i32;
    pub(crate) fn genzo_lr_open_buffer(h: *mut GenzoLr, data: *const c_void, len: usize) -> i32;
    pub(crate) fn genzo_lr_unpack(h: *mut GenzoLr) -> i32;

    pub(crate) fn genzo_lr_get_sizes(h: *mut GenzoLr, out: *mut Sizes) -> i32;
    pub(crate) fn genzo_lr_get_color(h: *mut GenzoLr, out: *mut Color) -> i32;
    pub(crate) fn genzo_lr_get_meta(h: *mut GenzoLr, out: *mut Meta) -> i32;
    pub(crate) fn genzo_lr_copy_raw(h: *mut GenzoLr, dst: *mut u16, dst_len: usize) -> i32;
    pub(crate) fn genzo_lr_error_count(h: *mut GenzoLr) -> i32;
    pub(crate) fn genzo_lr_decoder_name(h: *mut GenzoLr, dst: *mut c_char, dst_len: usize) -> i32;

    pub(crate) fn genzo_lr_unpack_thumb(h: *mut GenzoLr, out: *mut Thumb) -> i32;
    pub(crate) fn genzo_lr_copy_thumb(h: *mut GenzoLr, dst: *mut u8, dst_len: usize) -> i32;
}
