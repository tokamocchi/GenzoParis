/*
 * GenzoParis の LibRaw シム（C の API）。
 *
 * LibRaw の C++ API（libraw.h）を使い、Rust 側には平坦な C の構造体と関数だけを公開する。
 * Rust 側で libraw_data_t の巨大な構造体のレイアウトを再現しないため
 * （docs/04_architecture.md の 1.2 節、05 の PoC-2）。
 *
 * - C++ の例外はシムの中ですべて捕まえ、エラーコードにする。
 * - 構造体のレイアウトは crates/genzo-raw/src/libraw/ffi.rs と一致させる
 *   （genzo_lr_abi_check で大きさを照合する）。
 * - LibRaw のインスタンスは genzo_lr ごとに 1 つ。1 つの genzo_lr を複数のスレッドから
 *   同時に使ってはいけない（LibRaw の API-notes の「Thread safety」）。
 */
#ifndef GENZO_LIBRAW_SHIM_H
#define GENZO_LIBRAW_SHIM_H

#include <stddef.h>
#include <stdint.h>
#ifdef _WIN32
#include <wchar.h>
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* シム独自のエラーコード。LibRaw のエラーコード（0〜-100013）、errno（正の値）と重ならない。 */
#define GENZO_LR_E_NULL_ARG (-200001)
#define GENZO_LR_E_EXCEPTION (-200002)
#define GENZO_LR_E_BAD_ALLOC (-200003)
#define GENZO_LR_E_NO_BAYER_DATA (-200004)
#define GENZO_LR_E_BAD_GEOMETRY (-200005)
#define GENZO_LR_E_BUFFER_SIZE (-200006)
#define GENZO_LR_E_NO_THUMB_DATA (-200007)
#define GENZO_LR_E_ABI_MISMATCH (-200008)
/* 埋め込みサムネイルのデータがファイルの終わりを超えている（途中で切れている）。 */
#define GENZO_LR_E_THUMB_TRUNCATED (-200009)

/* LibRaw の LIBRAW_CBLACK_SIZE と同じ値（シムの中で static_assert で確かめる）。 */
#define GENZO_LR_CBLACK_SIZE 4104
/* COLOR(row, col) を取得する範囲。filters（32bit）は 8 行 × 2 列の周期を表すため。 */
#define GENZO_LR_CFA_ROWS 8
#define GENZO_LR_CFA_COLS 2

typedef struct genzo_lr genzo_lr;

/* 寸法と CFA の配置（imgdata.sizes / imgdata.idata）。 */
typedef struct genzo_lr_sizes {
  uint32_t raw_width;
  uint32_t raw_height;
  uint32_t width;
  uint32_t height;
  uint32_t top_margin;
  uint32_t left_margin;
  uint32_t raw_pitch; /* raw_image の 1 行のバイト数（unpack の後で有効） */
  int32_t flip;
  uint32_t filters;
  int32_t colors;
  uint32_t dng_version;
  uint32_t is_foveon;
  uint32_t fuji_rotated; /* is_fuji_rotated() != 0 */
  uint32_t raw_count;
  /* COLOR(row, col)（有効画素の左上を原点とする座標）。filters < 1000 のときは -1。 */
  int32_t cfa[GENZO_LR_CFA_ROWS][GENZO_LR_CFA_COLS];
  char cdesc[8];
} genzo_lr_sizes;

/* DNG の色の情報（imgdata.color.dng_color[i]）。 */
typedef struct genzo_lr_dng_color {
  uint32_t illuminant;
  float colormatrix[4][3];
  float calibration[4][4];
} genzo_lr_dng_color;

/* 黒レベル・白レベル・WB・行列（imgdata.color）。 */
typedef struct genzo_lr_color {
  uint32_t black;
  uint32_t maximum;
  uint32_t cblack[GENZO_LR_CBLACK_SIZE];
  int64_t linear_max[4];
  float cam_mul[4];
  float pre_mul[4];
  float cam_xyz[4][3];
  float rgb_cam[3][4];
  float cmatrix[3][4];
  genzo_lr_dng_color dng_color[2];
  float dng_analogbalance[4];
  uint32_t raw_bps;
  uint32_t reserved;
} genzo_lr_color;

/* 撮影情報（imgdata.idata / lens / other と、EXIF のコールバックで読んだ文字列）。 */
typedef struct genzo_lr_meta {
  char make[64];
  char model[64];
  char normalized_make[64];
  char normalized_model[64];
  char software[64];
  char exif_make[64];  /* IFD の Make（271）の元の文字列 */
  char exif_model[64]; /* IFD の Model（272）の元の文字列 */
  char lens[128];
  char makernotes_lens[128];
  char unique_camera_model[64];
  float iso_speed;
  float shutter;
  float aperture;
  float focal_len;
  int64_t timestamp;
  char timestamp_local[32]; /* timestamp を localtime で "YYYY:MM:DD HH:MM:SS" にしたもの */
  char datetime_original[64];
  char subsec_time_original[32];
  char offset_time_original[32];
  int32_t gps_parsed;
  float gps_latitude[3];
  float gps_longitude[3];
  float gps_altitude;
  char gps_latref;
  char gps_longref;
  char gps_altref;
  char gps_pad;
  int32_t flip;
  uint32_t width;
  uint32_t height;
} genzo_lr_meta;

/* 埋め込みサムネイル（imgdata.thumbnail。unpack_thumb の後）。 */
typedef struct genzo_lr_thumb {
  int32_t format; /* LibRaw_thumbnail_formats */
  uint32_t width;
  uint32_t height;
  uint32_t length;
  int32_t colors;
} genzo_lr_thumb;

/* LibRaw の版（実行時に読み込んだライブラリの LibRaw::version()）。 */
const char *genzo_lr_version(void);
/* 実行時のライブラリの版の番号（LibRaw::versionNumber()）。 */
int32_t genzo_lr_version_number(void);
/* シムをビルドしたときのヘッダの版の番号（LIBRAW_VERSION）。 */
int32_t genzo_lr_header_version_number(void);
/* 構造体の大きさを照合する。一致すれば 0、違えば GENZO_LR_E_ABI_MISMATCH。 */
int32_t genzo_lr_abi_check(size_t sizes_size, size_t color_size, size_t meta_size,
                           size_t thumb_size);
/* LibRaw のエラーコードの説明（LibRaw::strerror）。 */
const char *genzo_lr_strerror(int32_t code);

/* インスタンスを作る。失敗したら NULL。 */
genzo_lr *genzo_lr_new(void);
/* インスタンスを破棄する（NULL でもよい）。 */
void genzo_lr_free(genzo_lr *h);

/* ファイルを開いてメタデータを解析する（読み取り専用で開く。DATA-01）。 */
int32_t genzo_lr_open_file(genzo_lr *h, const char *path);
#ifdef _WIN32
/* ワイド文字（UTF-16）のパスで開く。LibRaw が Unicode のパスに対応していなければ
 * LIBRAW_NOT_IMPLEMENTED（-8）。 */
int32_t genzo_lr_open_wfile(genzo_lr *h, const wchar_t *path);
#endif
/* メモリ上のデータを開く。data は genzo_lr を破棄するまで有効でなければならない。 */
int32_t genzo_lr_open_buffer(genzo_lr *h, const void *data, size_t len);
/* RAW データを展開する。 */
int32_t genzo_lr_unpack(genzo_lr *h);

int32_t genzo_lr_get_sizes(genzo_lr *h, genzo_lr_sizes *out);
int32_t genzo_lr_get_color(genzo_lr *h, genzo_lr_color *out);
int32_t genzo_lr_get_meta(genzo_lr *h, genzo_lr_meta *out);
/* 有効画素の範囲（width × height）の u16 データを行優先でコピーする。dst_len は画素数。 */
int32_t genzo_lr_copy_raw(genzo_lr *h, uint16_t *dst, size_t dst_len);
/* unpack() で検出した、致命的でないデータの誤りの数（LibRaw::error_count）。 */
int32_t genzo_lr_error_count(genzo_lr *h);
/* 展開に使う関数の名前（LibRaw::unpack_function_name。記録方式の確認用）。 */
int32_t genzo_lr_decoder_name(genzo_lr *h, char *dst, size_t dst_len);

/* 埋め込みサムネイルを読み込み、形式と大きさを返す。データがファイルの終わりを超えている
 * 場合は GENZO_LR_E_THUMB_TRUNCATED。 */
int32_t genzo_lr_unpack_thumb(genzo_lr *h, genzo_lr_thumb *out);
/* 読み込んだサムネイルのバイト列をコピーする。dst_len は genzo_lr_thumb.length と同じ。 */
int32_t genzo_lr_copy_thumb(genzo_lr *h, uint8_t *dst, size_t dst_len);

#ifdef __cplusplus
}
#endif

#endif /* GENZO_LIBRAW_SHIM_H */
