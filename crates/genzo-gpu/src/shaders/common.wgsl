// 共通の定義（すべてのカーネル。docs/04_architecture.md の 2.3 節）。
//
// - 先頭に Rust 側（kernels/mod.rs）が生成した定数（P_* のパラメータの位置と、CPU 版と同じビット列の
//   f32 の定数）が付く。
// - パラメータは u32 の配列（binding 2）で受け取り、f32・i32 はビット列のまま bitcast する
//   （uniform の配置の規則に左右されないため）。
// - binding 3 は原子的なカウンター（[0] は置き換えた NaN・無限大の数）。
// - 浮動小数点は f32。「高速だが不正確な演算」の書き方（近似の関数・逆数の掛け算への置き換えなど）は
//   しない。演算の順序は CPU 版の式と同じにする（WGSL は結合・融合を許すので、完全な一致は保証できない）。

@group(0) @binding(2) var<storage, read> params: array<u32>;
@group(0) @binding(3) var<storage, read_write> stats: array<atomic<u32>>;

fn pu(i: u32) -> u32 {
    return params[i];
}

fn pi(i: u32) -> i32 {
    return bitcast<i32>(params[i]);
}

fn pf(i: u32) -> f32 {
    return bitcast<f32>(params[i]);
}

const EXP_MASK: u32 = 0x7f800000u;

// 有限か（ビット列で判定する。コンパイラが NaN・無限大はないと仮定しても消えないように）。
fn is_finite_f32(v: f32) -> bool {
    return (bitcast<u32>(v) & EXP_MASK) != EXP_MASK;
}

// NaN か（ビット列で判定する）。
fn is_nan_f32(v: f32) -> bool {
    return (bitcast<u32>(v) & 0x7fffffffu) > EXP_MASK;
}

// |mag| に sign の符号を付ける（Rust の f32::copysign と同じ）。
fn copysign_f32(mag: f32, sign: f32) -> f32 {
    return bitcast<f32>((bitcast<u32>(mag) & 0x7fffffffu) | (bitcast<u32>(sign) & 0x80000000u));
}

// 0 以上の値の四捨五入（0.5 は切り上げ。Rust の f32::round と同じ。WGSL の round は偶数への丸め）。
fn round_half_up(x: f32) -> f32 {
    let f = floor(x);
    return select(f, f + 1.0, x - f >= 0.5);
}

fn count_non_finite(n: u32) {
    if (n > 0u) {
        atomicAdd(&stats[0], n);
    }
}

// NaN・無限大を 0 にする（2.6 節。置き換えた数を数える）。
fn sanitize1(v: f32) -> f32 {
    if (is_finite_f32(v)) {
        return v;
    }
    count_non_finite(1u);
    return 0.0;
}

fn sanitize3(c: vec3<f32>) -> vec3<f32> {
    var o = c;
    var n = 0u;
    if (!is_finite_f32(c.x)) {
        o.x = 0.0;
        n = n + 1u;
    }
    if (!is_finite_f32(c.y)) {
        o.y = 0.0;
        n = n + 1u;
    }
    if (!is_finite_f32(c.z)) {
        o.z = 0.0;
        n = n + 1u;
    }
    count_non_finite(n);
    return o;
}

// 鏡映（端の画素を重複させない反射。genzo-pipeline の border::mirror_index と同じ式）。
fn mirror_index(i: i32, n: u32) -> u32 {
    if (n <= 1u) {
        return 0u;
    }
    let nn = i32(n);
    let period = 2 * (nn - 1);
    var m = i % period;
    if (m < 0) {
        m = m + period;
    }
    return u32(select(period - m, m, m < nn));
}

// 行ベクトル r と列ベクトル v の積を左から順に足す（genzo-color の Mat3F32::apply と同じ順序）。
fn row_dot(r: vec3<f32>, v: vec3<f32>) -> f32 {
    return r.x * v.x + r.y * v.y + r.z * v.z;
}

// パラメータの 3 つの f32（位置 i から）。
fn pf3(i: u32) -> vec3<f32> {
    return vec3<f32>(pf(i), pf(i + 1u), pf(i + 2u));
}

// 行列（行優先の 9 個の f32。位置 i から）を掛ける。
fn mat_apply(i: u32, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(row_dot(pf3(i), v), row_dot(pf3(i + 3u), v), row_dot(pf3(i + 6u), v));
}
