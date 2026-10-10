/*
 * GenzoParis の LibRaw シムの実装（genzo_libraw_shim.h を参照）。
 *
 * - LibRaw の C++ API だけを使い、Rust 側に平坦な構造体で値を渡す。
 * - LibRaw の関数は内部の例外を捕まえてエラーコードを返すが、念のためすべての関数を
 *   try / catch で囲み、C++ の例外が Rust 側へ抜けないようにする。
 * - 文字列はすべて NUL で終わるように切り詰めてコピーする。
 */
#include "genzo_libraw_shim.h"

#include <cstdint>
#include <cstring>
#include <ctime>
#include <new>

#include <libraw.h>

static_assert(GENZO_LR_CBLACK_SIZE == LIBRAW_CBLACK_SIZE,
              "GENZO_LR_CBLACK_SIZE は LIBRAW_CBLACK_SIZE と同じ値にする");

namespace {

/* EXIF のコールバックで読み取る文字列。 */
struct ExifCapture {
  char datetime_original[64];
  char subsec_time_original[32];
  char offset_time_original[32];
  char make[64];
  char model[64];
};

/* LibRaw の EXIF のコールバックに渡されるタグの番号（LibRaw 0.21 の src/metadata の実装）:
 * - EXIF IFD のタグ: そのままの番号
 * - TIFF の IFD のタグ: tag | ((ifd + 1) << 20)
 * - GPS IFD のタグ: tag | 0x50000、Kodak: tag | 0x20000、Panasonic: tag | 0x30000 */
constexpr int kTagDateTimeOriginal = 0x9003;
constexpr int kTagOffsetTimeOriginal = 0x9011;
constexpr int kTagSubSecTimeOriginal = 0x9291;
constexpr int kTagMake = 0x010f;
constexpr int kTagModel = 0x0110;
/* TIFF の型: ASCII と UNDEFINED。 */
constexpr int kTypeAscii = 2;
constexpr int kTypeUndefined = 7;

void copy_str(char *dst, size_t cap, const char *src, size_t src_cap) {
  if (cap == 0) return;
  size_t n = 0;
  if (src) {
    while (n < src_cap && n + 1 < cap && src[n] != '\0') n++;
    std::memcpy(dst, src, n);
  }
  dst[n] = '\0';
}

/* コールバックの中で、現在位置から len バイトの文字列を読む。最初に見つかった値だけを使う。
 * 読んだ後の位置は LibRaw がコールバックの後で元に戻す（fseek(ifp, savepos)）。 */
void read_ascii(void *ifp, int type, int len, char *dst, size_t cap) {
  if (dst[0] != '\0') return;
  if (type != kTypeAscii && type != kTypeUndefined) return;
  if (len <= 0 || cap < 2) return;
  size_t want = static_cast<size_t>(len);
  if (want > cap - 1) want = cap - 1;
  LibRaw_abstract_datastream *stream = static_cast<LibRaw_abstract_datastream *>(ifp);
  int got = stream->read(dst, 1, want);
  if (got < 0) got = 0;
  if (static_cast<size_t>(got) > want) got = static_cast<int>(want);
  dst[got] = '\0';
}

void exif_callback(void *context, int tag, int type, int len, unsigned int /*ord*/, void *ifp,
                   INT64 /*base*/) {
  try {
    ExifCapture *cap = static_cast<ExifCapture *>(context);
    if (!cap || !ifp) return;
    switch (tag) {
      case kTagDateTimeOriginal:
        read_ascii(ifp, type, len, cap->datetime_original, sizeof cap->datetime_original);
        return;
      case kTagOffsetTimeOriginal:
        read_ascii(ifp, type, len, cap->offset_time_original, sizeof cap->offset_time_original);
        return;
      case kTagSubSecTimeOriginal:
        read_ascii(ifp, type, len, cap->subsec_time_original, sizeof cap->subsec_time_original);
        return;
      default:
        break;
    }
    /* TIFF の IFD（(ifd + 1) << 20 が付いたもの）の Make / Model。 */
    if (tag > 0 && (tag >> 20) >= 1 && (tag & 0xf0000) == 0) {
      const int t = tag & 0xffff;
      if (t == kTagMake) {
        read_ascii(ifp, type, len, cap->make, sizeof cap->make);
      } else if (t == kTagModel) {
        read_ascii(ifp, type, len, cap->model, sizeof cap->model);
      }
    }
  } catch (...) {
    /* コールバックから例外を投げない。 */
  }
}

}  // namespace

struct genzo_lr {
  LibRaw *raw;
  ExifCapture exif;
};

extern "C" {

const char *genzo_lr_version(void) {
  try {
    return LibRaw::version();
  } catch (...) {
    return "";
  }
}

int32_t genzo_lr_version_number(void) {
  try {
    return LibRaw::versionNumber();
  } catch (...) {
    return 0;
  }
}

int32_t genzo_lr_header_version_number(void) { return LIBRAW_VERSION; }

int32_t genzo_lr_abi_check(size_t sizes_size, size_t color_size, size_t meta_size,
                           size_t thumb_size) {
  if (sizes_size != sizeof(genzo_lr_sizes) || color_size != sizeof(genzo_lr_color) ||
      meta_size != sizeof(genzo_lr_meta) || thumb_size != sizeof(genzo_lr_thumb)) {
    return GENZO_LR_E_ABI_MISMATCH;
  }
  return 0;
}

const char *genzo_lr_strerror(int32_t code) {
  try {
    return LibRaw::strerror(code);
  } catch (...) {
    return "";
  }
}

genzo_lr *genzo_lr_new(void) {
  try {
    genzo_lr *h = new (std::nothrow) genzo_lr;
    if (!h) return nullptr;
    std::memset(&h->exif, 0, sizeof h->exif);
    /* 既定のデータエラーのコールバックは標準エラー出力に書くため、無効にする。
     * 致命的でないデータの誤りの数は genzo_lr_error_count で取得する。 */
    h->raw = new (std::nothrow) LibRaw(LIBRAW_OPTIONS_NO_DATAERR_CALLBACK);
    if (!h->raw) {
      delete h;
      return nullptr;
    }
    h->raw->set_exifparser_handler(exif_callback, &h->exif);
    return h;
  } catch (...) {
    return nullptr;
  }
}

void genzo_lr_free(genzo_lr *h) {
  if (!h) return;
  try {
    delete h->raw;
  } catch (...) {
  }
  delete h;
}

int32_t genzo_lr_open_file(genzo_lr *h, const char *path) {
  if (!h || !h->raw || !path) return GENZO_LR_E_NULL_ARG;
  try {
    std::memset(&h->exif, 0, sizeof h->exif);
    return h->raw->open_file(path);
  } catch (const std::bad_alloc &) {
    return GENZO_LR_E_BAD_ALLOC;
  } catch (...) {
    return GENZO_LR_E_EXCEPTION;
  }
}

#ifdef _WIN32
int32_t genzo_lr_open_wfile(genzo_lr *h, const wchar_t *path) {
  if (!h || !h->raw || !path) return GENZO_LR_E_NULL_ARG;
  try {
    std::memset(&h->exif, 0, sizeof h->exif);
    return h->raw->open_file(path);
  } catch (const std::bad_alloc &) {
    return GENZO_LR_E_BAD_ALLOC;
  } catch (...) {
    return GENZO_LR_E_EXCEPTION;
  }
}
#endif

int32_t genzo_lr_open_buffer(genzo_lr *h, const void *data, size_t len) {
  if (!h || !h->raw || !data) return GENZO_LR_E_NULL_ARG;
  try {
    std::memset(&h->exif, 0, sizeof h->exif);
    return h->raw->open_buffer(data, len);
  } catch (const std::bad_alloc &) {
    return GENZO_LR_E_BAD_ALLOC;
  } catch (...) {
    return GENZO_LR_E_EXCEPTION;
  }
}

int32_t genzo_lr_unpack(genzo_lr *h) {
  if (!h || !h->raw) return GENZO_LR_E_NULL_ARG;
  try {
    return h->raw->unpack();
  } catch (const std::bad_alloc &) {
    return GENZO_LR_E_BAD_ALLOC;
  } catch (...) {
    return GENZO_LR_E_EXCEPTION;
  }
}

int32_t genzo_lr_get_sizes(genzo_lr *h, genzo_lr_sizes *out) {
  if (!h || !h->raw || !out) return GENZO_LR_E_NULL_ARG;
  try {
    std::memset(out, 0, sizeof *out);
    LibRaw &lr = *h->raw;
    const libraw_image_sizes_t &s = lr.imgdata.sizes;
    const libraw_iparams_t &id = lr.imgdata.idata;
    out->raw_width = s.raw_width;
    out->raw_height = s.raw_height;
    out->width = s.width;
    out->height = s.height;
    out->top_margin = s.top_margin;
    out->left_margin = s.left_margin;
    out->raw_pitch = s.raw_pitch;
    out->flip = s.flip;
    out->filters = id.filters;
    out->colors = id.colors;
    out->dng_version = id.dng_version;
    out->is_foveon = id.is_foveon;
    out->fuji_rotated = lr.is_fuji_rotated() ? 1u : 0u;
    out->raw_count = id.raw_count;
    for (int row = 0; row < GENZO_LR_CFA_ROWS; row++) {
      for (int col = 0; col < GENZO_LR_CFA_COLS; col++) {
        /* filters < 1000 は特殊な配置（1: Leaf の 16×16、9: X-Trans）なので返さない。 */
        out->cfa[row][col] = id.filters >= 1000 ? lr.COLOR(row, col) : -1;
      }
    }
    copy_str(out->cdesc, sizeof out->cdesc, id.cdesc, sizeof id.cdesc);
    return 0;
  } catch (...) {
    return GENZO_LR_E_EXCEPTION;
  }
}

int32_t genzo_lr_get_color(genzo_lr *h, genzo_lr_color *out) {
  if (!h || !h->raw || !out) return GENZO_LR_E_NULL_ARG;
  try {
    std::memset(out, 0, sizeof *out);
    const libraw_colordata_t &c = h->raw->imgdata.color;
    out->black = c.black;
    out->maximum = c.maximum;
    std::memcpy(out->cblack, c.cblack, sizeof out->cblack);
    for (int i = 0; i < 4; i++) {
      out->linear_max[i] = static_cast<int64_t>(c.linear_max[i]);
      out->cam_mul[i] = c.cam_mul[i];
      out->pre_mul[i] = c.pre_mul[i];
      out->dng_analogbalance[i] = c.dng_levels.analogbalance[i];
    }
    std::memcpy(out->cam_xyz, c.cam_xyz, sizeof out->cam_xyz);
    std::memcpy(out->rgb_cam, c.rgb_cam, sizeof out->rgb_cam);
    std::memcpy(out->cmatrix, c.cmatrix, sizeof out->cmatrix);
    for (int i = 0; i < 2; i++) {
      out->dng_color[i].illuminant = c.dng_color[i].illuminant;
      std::memcpy(out->dng_color[i].colormatrix, c.dng_color[i].colormatrix,
                  sizeof out->dng_color[i].colormatrix);
      std::memcpy(out->dng_color[i].calibration, c.dng_color[i].calibration,
                  sizeof out->dng_color[i].calibration);
    }
    out->raw_bps = c.raw_bps;
    return 0;
  } catch (...) {
    return GENZO_LR_E_EXCEPTION;
  }
}

int32_t genzo_lr_get_meta(genzo_lr *h, genzo_lr_meta *out) {
  if (!h || !h->raw || !out) return GENZO_LR_E_NULL_ARG;
  try {
    std::memset(out, 0, sizeof *out);
    LibRaw &lr = *h->raw;
    const libraw_iparams_t &id = lr.imgdata.idata;
    const libraw_imgother_t &o = lr.imgdata.other;
    copy_str(out->make, sizeof out->make, id.make, sizeof id.make);
    copy_str(out->model, sizeof out->model, id.model, sizeof id.model);
    copy_str(out->normalized_make, sizeof out->normalized_make, id.normalized_make,
             sizeof id.normalized_make);
    copy_str(out->normalized_model, sizeof out->normalized_model, id.normalized_model,
             sizeof id.normalized_model);
    copy_str(out->software, sizeof out->software, id.software, sizeof id.software);
    copy_str(out->exif_make, sizeof out->exif_make, h->exif.make, sizeof h->exif.make);
    copy_str(out->exif_model, sizeof out->exif_model, h->exif.model, sizeof h->exif.model);
    copy_str(out->lens, sizeof out->lens, lr.imgdata.lens.Lens, sizeof lr.imgdata.lens.Lens);
    copy_str(out->makernotes_lens, sizeof out->makernotes_lens, lr.imgdata.lens.makernotes.Lens,
             sizeof lr.imgdata.lens.makernotes.Lens);
    copy_str(out->unique_camera_model, sizeof out->unique_camera_model,
             lr.imgdata.color.UniqueCameraModel, sizeof lr.imgdata.color.UniqueCameraModel);
    out->iso_speed = o.iso_speed;
    out->shutter = o.shutter;
    out->aperture = o.aperture;
    out->focal_len = o.focal_len;
    out->timestamp = static_cast<int64_t>(o.timestamp);
    if (o.timestamp > 0) {
      /* LibRaw は EXIF の日時を mktime（プロセスのローカルタイムゾーン）で time_t にしている。
       * 同じプロセスの localtime で戻すと、元の日時の表記になる（夏時間の切り替わりを除く）。 */
      std::time_t t = o.timestamp;
      std::tm tmv;
      std::memset(&tmv, 0, sizeof tmv);
#ifdef _WIN32
      const bool ok = localtime_s(&tmv, &t) == 0;
#else
      const bool ok = localtime_r(&t, &tmv) != nullptr;
#endif
      if (ok) {
        std::strftime(out->timestamp_local, sizeof out->timestamp_local, "%Y:%m:%d %H:%M:%S",
                      &tmv);
      }
    }
    copy_str(out->datetime_original, sizeof out->datetime_original, h->exif.datetime_original,
             sizeof h->exif.datetime_original);
    copy_str(out->subsec_time_original, sizeof out->subsec_time_original,
             h->exif.subsec_time_original, sizeof h->exif.subsec_time_original);
    copy_str(out->offset_time_original, sizeof out->offset_time_original,
             h->exif.offset_time_original, sizeof h->exif.offset_time_original);
    const libraw_gps_info_t &g = o.parsed_gps;
    out->gps_parsed = g.gpsparsed ? 1 : 0;
    for (int i = 0; i < 3; i++) {
      out->gps_latitude[i] = g.latitude[i];
      out->gps_longitude[i] = g.longitude[i];
    }
    out->gps_altitude = g.altitude;
    out->gps_latref = g.latref;
    out->gps_longref = g.longref;
    out->gps_altref = g.altref;
    out->flip = lr.imgdata.sizes.flip;
    out->width = lr.imgdata.sizes.width;
    out->height = lr.imgdata.sizes.height;
    return 0;
  } catch (...) {
    return GENZO_LR_E_EXCEPTION;
  }
}

int32_t genzo_lr_copy_raw(genzo_lr *h, uint16_t *dst, size_t dst_len) {
  if (!h || !h->raw || !dst) return GENZO_LR_E_NULL_ARG;
  try {
    LibRaw &lr = *h->raw;
    const ushort *src = lr.imgdata.rawdata.raw_image;
    if (!src) return GENZO_LR_E_NO_BAYER_DATA;
    const libraw_image_sizes_t &s = lr.imgdata.sizes;
    const size_t width = s.width;
    const size_t height = s.height;
    const size_t left = s.left_margin;
    const size_t top = s.top_margin;
    const size_t pitch = s.raw_pitch;
    /* 有効画素の範囲が RAW のバッファの中にあることを確かめる（LibRaw の copy_bayer は
     * はみ出す部分を 0 のままにするが、ここではエラーにする）。 */
    if (width == 0 || height == 0 || left + width > s.raw_width || top + height > s.raw_height ||
        pitch % 2 != 0 || pitch < static_cast<size_t>(s.raw_width) * 2) {
      return GENZO_LR_E_BAD_GEOMETRY;
    }
    if (dst_len != width * height) return GENZO_LR_E_BUFFER_SIZE;
    const size_t stride = pitch / 2;
    for (size_t row = 0; row < height; row++) {
      std::memcpy(dst + row * width, src + (top + row) * stride + left, width * sizeof(uint16_t));
    }
    return 0;
  } catch (...) {
    return GENZO_LR_E_EXCEPTION;
  }
}

int32_t genzo_lr_error_count(genzo_lr *h) {
  if (!h || !h->raw) return GENZO_LR_E_NULL_ARG;
  try {
    return h->raw->error_count();
  } catch (...) {
    return GENZO_LR_E_EXCEPTION;
  }
}

int32_t genzo_lr_decoder_name(genzo_lr *h, char *dst, size_t dst_len) {
  if (!h || !h->raw || !dst || dst_len == 0) return GENZO_LR_E_NULL_ARG;
  try {
    const char *name = h->raw->unpack_function_name();
    copy_str(dst, dst_len, name, name ? std::strlen(name) : 0);
    return 0;
  } catch (...) {
    return GENZO_LR_E_EXCEPTION;
  }
}

int32_t genzo_lr_unpack_thumb(genzo_lr *h, genzo_lr_thumb *out) {
  if (!h || !h->raw || !out) return GENZO_LR_E_NULL_ARG;
  try {
    std::memset(out, 0, sizeof *out);
    const int rc = h->raw->unpack_thumb();
    if (rc != LIBRAW_SUCCESS) return rc;
    const libraw_thumbnail_t &t = h->raw->imgdata.thumbnail;
    out->format = static_cast<int32_t>(t.tformat);
    out->width = t.twidth;
    out->height = t.theight;
    out->length = t.tlength;
    out->colors = t.tcolors;
    return 0;
  } catch (const std::bad_alloc &) {
    return GENZO_LR_E_BAD_ALLOC;
  } catch (...) {
    return GENZO_LR_E_EXCEPTION;
  }
}

int32_t genzo_lr_copy_thumb(genzo_lr *h, uint8_t *dst, size_t dst_len) {
  if (!h || !h->raw || !dst) return GENZO_LR_E_NULL_ARG;
  try {
    const libraw_thumbnail_t &t = h->raw->imgdata.thumbnail;
    if (!t.thumb) return GENZO_LR_E_NO_THUMB_DATA;
    if (dst_len != t.tlength) return GENZO_LR_E_BUFFER_SIZE;
    std::memcpy(dst, t.thumb, dst_len);
    return 0;
  } catch (...) {
    return GENZO_LR_E_EXCEPTION;
  }
}

}  // extern "C"
