// 色の計算の共通部分（genzo-color・genzo-pipeline の f32 の式と同じ順序）。
//
// 定数（LUMA_*・SRGB_*・ADOBE_*・OK_*・LOG_* など）は Rust 側が CPU 版の値のビット列から生成する
// （16 進の浮動小数点の表記なので、10 進への丸めの差が出ない）。

// BT.2020 の輝度（genzo-pipeline の finish::luminance。左から順に足す）。
fn luminance(c: vec3<f32>) -> f32 {
    return LUMA_R * c.x + LUMA_G * c.y + LUMA_B * c.z;
}

// log2(max(v, 2^-16))（genzo-pipeline の sanitize::safe_log2）。
fn safe_log2(v: f32) -> f32 {
    return log2(max(v, LOG_FLOOR));
}

// IEC 61966-2-1: リニア → 符号化値（genzo-color の srgb_encode_f32。符号を保つ）。
fn srgb_encode(x: f32) -> f32 {
    let a = abs(x);
    var e: f32;
    if (a <= SRGB_ENCODE_THRESHOLD) {
        e = SRGB_LINEAR_SLOPE * a;
    } else {
        e = SRGB_ONE_PLUS_OFFSET * pow(a, SRGB_INV_GAMMA) - SRGB_OFFSET;
    }
    return copysign_f32(e, x);
}

// IEC 61966-2-1: 符号化値 → リニア（genzo-color の srgb_decode_f32）。
fn srgb_decode(v: f32) -> f32 {
    let a = abs(v);
    var l: f32;
    if (a <= SRGB_DECODE_THRESHOLD) {
        l = a / SRGB_LINEAR_SLOPE;
    } else {
        l = pow((a + SRGB_OFFSET) / SRGB_ONE_PLUS_OFFSET, SRGB_GAMMA);
    }
    return copysign_f32(l, v);
}

// Adobe RGB (1998): リニア → 符号化値（genzo-color の adobe_rgb_encode_f32）。pow(0, y) は環境に
// よって未定義なので、0 は 0 のまま返す。
fn adobe_encode(x: f32) -> f32 {
    let a = abs(x);
    if (a == 0.0) {
        return x;
    }
    return copysign_f32(pow(a, ADOBE_INV_GAMMA), x);
}

// 伝達関数（0: リニア、1: IEC 61966-2-1、2: Adobe RGB のガンマ）。
fn transfer_encode(kind: u32, v: f32) -> f32 {
    if (kind == 1u) {
        return srgb_encode(v);
    }
    if (kind == 2u) {
        return adobe_encode(v);
    }
    return v;
}

// 符号付きの立方根。WGSL に cbrt がないので pow(|x|, 1/3) にニュートン法の補正を 1 回かける
// （libm の cbrtf との差を小さくするため。完全には一致しない）。
fn cbrt_f32(x: f32) -> f32 {
    let a = abs(x);
    if (a == 0.0) {
        return x;
    }
    var y = pow(a, ONE_THIRD);
    if (a > CBRT_NEWTON_MIN && a < CBRT_NEWTON_MAX) {
        y = y - (y * y * y - a) / (3.0 * y * y);
    }
    return copysign_f32(y, x);
}

// リニア BT.2020 → OKLab（genzo-color の linear_bt2020_to_oklab）。
fn oklab_from_bt2020(c: vec3<f32>) -> vec3<f32> {
    let lms = vec3<f32>(row_dot(OK_LMS_R0, c), row_dot(OK_LMS_R1, c), row_dot(OK_LMS_R2, c));
    let r = vec3<f32>(cbrt_f32(lms.x), cbrt_f32(lms.y), cbrt_f32(lms.z));
    return vec3<f32>(row_dot(OK_M2_R0, r), row_dot(OK_M2_R1, r), row_dot(OK_M2_R2, r));
}

// OKLab → リニア BT.2020（genzo-color の oklab_to_linear_bt2020）。
fn oklab_to_bt2020(lab: vec3<f32>) -> vec3<f32> {
    let c = vec3<f32>(row_dot(OK_M2I_R0, lab), row_dot(OK_M2I_R1, lab), row_dot(OK_M2I_R2, lab));
    let cubes = vec3<f32>(c.x * c.x * c.x, c.y * c.y * c.y, c.z * c.z * c.z);
    return vec3<f32>(
        row_dot(OK_RGB_R0, cubes),
        row_dot(OK_RGB_R1, cubes),
        row_dot(OK_RGB_R2, cubes),
    );
}

// 色域の境界までの相対的な距離の 1 成分（genzo-color の GamutCompressor::compress）。
fn gamut_dist(di: f32, y: f32) -> f32 {
    if (di > 0.0) {
        return di / (1.0 - y);
    }
    if (di < 0.0) {
        return -di / y;
    }
    return 0.0;
}

// 色域の圧縮（genzo-color の GamutCompressor::compress と同じ式。w は輝度の係数、inv_power は
// CPU 版が求めた 1 / power）。
fn gamut_compress(rgb: vec3<f32>, w: vec3<f32>, threshold: f32, power: f32, inv_power: f32) -> vec3<f32> {
    let c = vec3<f32>(
        select(0.0, rgb.x, is_finite_f32(rgb.x)),
        select(0.0, rgb.y, is_finite_f32(rgb.y)),
        select(0.0, rgb.z, is_finite_f32(rgb.z)),
    );
    let y = w.x * c.x + w.y * c.y + w.z * c.z;
    if (is_nan_f32(y) || y <= 0.0) {
        return vec3<f32>(0.0);
    }
    if (y >= 1.0) {
        return vec3<f32>(1.0);
    }
    let d = vec3<f32>(c.x - y, c.y - y, c.z - y);
    let t = max(max(max(0.0, gamut_dist(d.x, y)), gamut_dist(d.y, y)), gamut_dist(d.z, y));
    if (t <= threshold) {
        return c;
    }
    let k = threshold;
    let x = (t - k) / (1.0 - k);
    var f: f32;
    if (x <= 1.0) {
        f = x / pow(1.0 + pow(x, power), inv_power);
    } else {
        f = 1.0 / pow(pow(x, -power) + 1.0, inv_power);
    }
    let t_new = k + (1.0 - k) * f;
    var scale = 0.0;
    if (is_finite_f32(t)) {
        scale = t_new / t;
    }
    return vec3<f32>(
        clamp(y + scale * d.x, 0.0, 1.0),
        clamp(y + scale * d.y, 0.0, 1.0),
        clamp(y + scale * d.z, 0.0, 1.0),
    );
}
