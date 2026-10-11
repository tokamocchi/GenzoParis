// ステージ 17b の量子化と決定的なディザリング（genzo-pipeline の finish::output::quantize_u8・
// quantize_u16。IQ-08）。
//
// 1 スレッドが 12 バイト（8bit は 4 画素、16bit は 2 画素）を作り、u32 の 3 語に詰めて書く
// （語をまたぐ書き込みの競合を避けるため）。語の中は小さい番地が下位のバイト（リトルエンディアン）。
// 画素の並びは R, G, B, R, G, B, …（行優先）。ノイズは画素の画像全体での座標
// （P_QORIGIN_X + x, P_QORIGIN_Y + y）で決まる。

@group(0) @binding(0) var<storage, read> src: array<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<u32>;

// lowbias32（output::hash32。乗算はラップする）。
fn hash32(x0: u32) -> u32 {
    var x = x0;
    x = x ^ (x >> 16u);
    x = x * 0x7feb352du;
    x = x ^ (x >> 15u);
    x = x * 0x846ca68bu;
    x = x ^ (x >> 16u);
    return x;
}

// 三角分布のノイズ（output::dither_noise）。
fn dither_noise(x: i32, y: i32, c: u32, seed: u32) -> f32 {
    let h = hash32(bitcast<u32>(x) ^ hash32(bitcast<u32>(y) ^ hash32(c ^ seed)));
    let u1 = f32(hash32(h) >> 8u) * DITHER_SCALE;
    let u2 = f32(hash32(h ^ 0x9e3779b9u) >> 8u) * DITHER_SCALE;
    return u1 + u2 - 1.0;
}

// output::quantize_value。
fn quantize_value(v: f32, maxv: f32, noise: f32) -> u32 {
    if (is_nan_f32(v) || v <= 0.0) {
        return 0u;
    }
    if (v >= 1.0) {
        return u32(maxv);
    }
    return u32(clamp(floor(v * maxv + 0.5 + noise), 0.0, maxv));
}

// 値の番号 k（画素 k / 3 のチャンネル k % 3）の量子化した値。範囲の外は 0。
fn quantized(k: u32, n_values: u32, maxv: f32) -> u32 {
    if (k >= n_values) {
        return 0u;
    }
    let p = k / 3u;
    let ch = k % 3u;
    let w = pu(P_OUT_W);
    let x = i32(p % w);
    let y = i32(p / w);
    let v = src[k];
    var noise = 0.0;
    if (pu(P_QDITHER) != 0u) {
        noise = dither_noise(pi(P_QORIGIN_X) + x, pi(P_QORIGIN_Y) + y, ch, pu(P_QSEED));
    }
    return quantize_value(v, maxv, noise);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let t = gid.y * pu(P_QSTRIDE) + gid.x;
    let n_values = pu(P_OUT_W) * pu(P_OUT_H) * 3u;
    let first = t * 12u / select(1u, 2u, pu(P_QBITS) == 16u);
    if (first >= n_values) {
        return;
    }
    for (var j = 0u; j < 3u; j = j + 1u) {
        var word = 0u;
        if (pu(P_QBITS) == 16u) {
            // 2 つの 16bit の値。
            for (var b = 0u; b < 2u; b = b + 1u) {
                let k = first + j * 2u + b;
                word = word | (quantized(k, n_values, 65535.0) << (b * 16u));
            }
        } else {
            // 4 つの 8bit の値。
            for (var b = 0u; b < 4u; b = b + 1u) {
                let k = first + j * 4u + b;
                word = word | (quantized(k, n_values, 255.0) << (b * 8u));
            }
        }
        dst[t * 3u + j] = word;
    }
}
