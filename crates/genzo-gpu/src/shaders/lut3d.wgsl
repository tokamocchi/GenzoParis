// 3D LUT の四面体補間（genzo-color の Lut3d::apply。17a の画面と、独自のルック（DEV-17。profile.wgsl）で共通）。
//
// binding 4（aux）: 3D LUT の格子の値（R が最も速く変わる並び。1 点 3 個の f32）。格子の数はパラメータ LUT_N。

@group(0) @binding(4) var<storage, read> aux: array<f32>;

fn lut3d_at(n: u32, r: u32, g: u32, b: u32) -> vec3<f32> {
    let i = ((b * n + g) * n + r) * 3u;
    return vec3<f32>(aux[i], aux[i + 1u], aux[i + 2u]);
}

struct LutAxis {
    base: u32,
    frac: f32,
}

// 入力の 1 成分を格子の番号と小数部にする（0〜1 に収める。NaN は 0）。
fn lut_axis(v: f32, n1: f32, max_base: u32) -> LutAxis {
    var vv = 0.0;
    if (!is_nan_f32(v)) {
        vv = clamp(v, 0.0, 1.0);
    }
    let x = vv * n1;
    let i = min(u32(floor(x)), max_base);
    return LutAxis(i, x - f32(i));
}

// 四面体補間（genzo-color の Lut3d::apply と同じ四面体の選び方・重み・足す順序）。
fn lut3d_apply(rgb: vec3<f32>) -> vec3<f32> {
    let n = pu(P_LUT_N);
    let n1 = f32(n - 1u);
    let max_base = n - 2u;
    let ar = lut_axis(rgb.x, n1, max_base);
    let ag = lut_axis(rgb.y, n1, max_base);
    let ab = lut_axis(rgb.z, n1, max_base);
    let r0 = ar.base;
    let g0 = ag.base;
    let b0 = ab.base;
    let r1 = r0 + 1u;
    let g1 = g0 + 1u;
    let b1 = b0 + 1u;
    let fr = ar.frac;
    let fg = ag.frac;
    let fb = ab.frac;
    var w0: f32;
    var w1: f32;
    var w2: f32;
    var w3: f32;
    var p1: vec3<u32>;
    var p2: vec3<u32>;
    if (fr >= fg) {
        if (fg >= fb) {
            // fr ≥ fg ≥ fb: R → G → B
            w0 = 1.0 - fr;
            p1 = vec3<u32>(r1, g0, b0);
            w1 = fr - fg;
            p2 = vec3<u32>(r1, g1, b0);
            w2 = fg - fb;
            w3 = fb;
        } else if (fr >= fb) {
            // fr ≥ fb > fg: R → B → G
            w0 = 1.0 - fr;
            p1 = vec3<u32>(r1, g0, b0);
            w1 = fr - fb;
            p2 = vec3<u32>(r1, g0, b1);
            w2 = fb - fg;
            w3 = fg;
        } else {
            // fb > fr ≥ fg: B → R → G
            w0 = 1.0 - fb;
            p1 = vec3<u32>(r0, g0, b1);
            w1 = fb - fr;
            p2 = vec3<u32>(r1, g0, b1);
            w2 = fr - fg;
            w3 = fg;
        }
    } else if (fb >= fg) {
        // fb ≥ fg > fr: B → G → R
        w0 = 1.0 - fb;
        p1 = vec3<u32>(r0, g0, b1);
        w1 = fb - fg;
        p2 = vec3<u32>(r0, g1, b1);
        w2 = fg - fr;
        w3 = fr;
    } else if (fb >= fr) {
        // fg > fb ≥ fr: G → B → R
        w0 = 1.0 - fg;
        p1 = vec3<u32>(r0, g1, b0);
        w1 = fg - fb;
        p2 = vec3<u32>(r0, g1, b1);
        w2 = fb - fr;
        w3 = fr;
    } else {
        // fg > fr > fb: G → R → B
        w0 = 1.0 - fg;
        p1 = vec3<u32>(r0, g1, b0);
        w1 = fg - fr;
        p2 = vec3<u32>(r1, g1, b0);
        w2 = fr - fb;
        w3 = fb;
    }
    let c0 = lut3d_at(n, r0, g0, b0);
    let c1 = lut3d_at(n, p1.x, p1.y, p1.z);
    let c2 = lut3d_at(n, p2.x, p2.y, p2.z);
    let c3 = lut3d_at(n, r1, g1, b1);
    return vec3<f32>(
        w0 * c0.x + w1 * c1.x + w2 * c2.x + w3 * c3.x,
        w0 * c0.y + w1 * c1.y + w2 * c2.y + w3 * c3.y,
        w0 * c0.z + w1 * c1.z + w2 * c2.z + w3 * c3.z,
    );
}
