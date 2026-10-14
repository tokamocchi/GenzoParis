// ローカル補正（マスク。処理バージョン 2。DEV-18・DEV-19。docs/04_architecture.md の 2.1 節「ローカル補正」）。
// 式と演算の順序は genzo-pipeline の local の doc と同じ。finish.wgsl と同じモジュール（aux・lut_eval を使う）。
//
// - local_main: 基準の画像（ステージ 9 の出力。src）から、局所の値の面（dst。面の番号の順に OUT の画素数ずつ）を作る。
//   aux の先頭はステージ 15 の輝度のカーブの表（P_LUT_OFF の表 0。RAW のとき）、P_LOCAL_DATA からマスクの記録、その後ろに
//   ブラシのラスター（genzo-gpu の stages.rs の local_params の doc）。
// - ステージ 10〜14 のカーネルは、P_LOCAL_FLAGS の 1 があれば binding 6（local_field。範囲は P_LOCAL_X・P_LOCAL_Y・
//   P_LOCAL_W、面の画素数は P_LOCAL_N）から画素の局所の値を引く（項目 p の面の番号は P_LOCAL_SLOT + p。面がなければ
//   0xffffffff）。P_LOCAL_FLAGS の 2 は「全体の値が何もせず、ローカル補正だけで有効」（局所の値が 0 の画素は変えない）。

@group(0) @binding(6) var<storage, read> local_field: array<f32>;

const LOCAL_FLAG_ON: u32 = 1u;
const LOCAL_FLAG_ONLY: u32 = 2u;

// genzo-pipeline の LocalParam の番号。
const LP_TEMPERATURE: u32 = 0u;
const LP_TINT: u32 = 1u;
const LP_EXPOSURE: u32 = 2u;
const LP_CONTRAST: u32 = 3u;
const LP_HIGHLIGHTS: u32 = 4u;
const LP_SHADOWS: u32 = 5u;
const LP_WHITES: u32 = 6u;
const LP_BLACKS: u32 = 7u;
const LP_TEXTURE: u32 = 8u;
const LP_CLARITY: u32 = 9u;
const LP_DEHAZE: u32 = 10u;
const LP_SATURATION: u32 = 11u;
const LP_SHARPNESS: u32 = 12u;

// このステージが局所の値を使うか。
fn local_on() -> bool {
    return (pu(P_LOCAL_FLAGS) & LOCAL_FLAG_ON) != 0u;
}

// ローカル補正だけで有効か。
fn local_only() -> bool {
    return (pu(P_LOCAL_FLAGS) & LOCAL_FLAG_ONLY) != 0u;
}

// 全体での座標 g の画素の、項目 p の局所の値（面がなければ 0）。
fn local_value(p: u32, g: vec2<i32>) -> f32 {
    let slot = pu(P_LOCAL_SLOT + p);
    if (slot == 0xffffffffu) {
        return 0.0;
    }
    let i = u32(g.y - pi(P_LOCAL_Y)) * pu(P_LOCAL_W) + u32(g.x - pi(P_LOCAL_X));
    return local_field[slot * pu(P_LOCAL_N) + i];
}

// local::add_unit: clamp(unit + d / 100, lo, hi)。
fn add_unit(unit: f32, d: f32, lo: f32, hi: f32) -> f32 {
    return clamp(unit + d / 100.0, lo, hi);
}

// local::white_balance_gains（作業色空間の RGB の倍率。中立色の輝度を保つ。和は ±100 に収める）。
fn local_wb_gains(temperature: f32, tint: f32) -> vec3<f32> {
    let t = add_unit(0.0, temperature, -1.0, 1.0);
    let k = add_unit(0.0, tint, -1.0, 1.0);
    let half = 0.5 * LOCAL_TINT_EV * k;
    let e = vec3<f32>(
        exp2(LOCAL_TEMPERATURE_EV * t + half),
        exp2(-LOCAL_TINT_EV * k),
        exp2(-LOCAL_TEMPERATURE_EV * t + half),
    );
    let y = luminance(e);
    return vec3<f32>(e.x / y, e.y / y, e.z / y);
}

// ---------------------------------------------------------------------------
// 局所の値の面（local_main）
// ---------------------------------------------------------------------------

// local::step（e1 > e0 でなければ x ≥ e1 で 1）。
fn local_step(e0: f32, e1: f32, x: f32) -> f32 {
    if (e1 > e0) {
        let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
        return t * t * (3.0 - 2.0 * t);
    }
    return select(0.0, 1.0, x >= e1);
}

// SceneToDisplayParams::curve（P_S2D_MODE = 1 は RAW の表、0 は RAW 以外の恒等）。
fn s2d_curve(y: f32) -> f32 {
    if (pu(P_S2D_MODE) == 1u) {
        if (y >= LOG_FLOOR) {
            return lut_eval(0u, log2(y));
        }
        return y * pf(P_S2D_FLOOR_RATIO);
    }
    if (y > 1.0) {
        return 1.0;
    }
    return y;
}

// BrushRaster::sample（ラスターの記録の位置 r: 値の位置・幅・高さ・原点 x・原点 y・1 / セル）。範囲の外は 0。
fn brush_at(off: u32, w: u32, h: u32, x: i32, y: i32) -> f32 {
    if (x < 0 || y < 0 || x >= i32(w) || y >= i32(h)) {
        return 0.0;
    }
    return aux[off + u32(y) * w + u32(x)];
}

fn brush_sample(r: u32, u: f32, v: f32) -> f32 {
    if (aux[r + 6u] == 0.0) {
        return 0.0;
    }
    let off = u32(aux[r]);
    let w = u32(aux[r + 1u]);
    let h = u32(aux[r + 2u]);
    let inv = aux[r + 5u];
    let fx = (u - aux[r + 3u]) * inv - 0.5;
    let fy = (v - aux[r + 4u]) * inv - 0.5;
    if (!(is_finite_f32(fx) && is_finite_f32(fy))) {
        return 0.0;
    }
    let x0f = floor(fx);
    let y0f = floor(fy);
    if (x0f < -1.0 || y0f < -1.0 || x0f >= f32(w) || y0f >= f32(h)) {
        return 0.0;
    }
    let tx = fx - x0f;
    let ty = fy - y0f;
    let x0 = i32(x0f);
    let y0 = i32(y0f);
    let a = brush_at(off, w, h, x0, y0);
    let b = brush_at(off, w, h, x0 + 1, y0);
    let c = brush_at(off, w, h, x0, y0 + 1);
    let d = brush_at(off, w, h, x0 + 1, y0 + 1);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    return top + (bottom - top) * ty;
}

// PreparedShape::value（部品の記録の位置 r。[種類, 合成, 反転, 値 13 個]）。
fn component_value(r: u32, u: f32, v: f32, ch: vec2<f32>, luma: f32) -> f32 {
    let kind = u32(aux[r]);
    let q = r + 3u;
    if (kind == 0u) {
        // 線形。
        let inv_len2 = aux[q + 4u];
        if (inv_len2 == 0.0) {
            return 0.0;
        }
        let t = ((u - aux[q]) * aux[q + 2u] + (v - aux[q + 1u]) * aux[q + 3u]) * inv_len2;
        return 1.0 - local_step(0.0, 1.0, t);
    }
    if (kind == 1u) {
        // 円形。
        let du = u - aux[q];
        let dv = v - aux[q + 1u];
        let cs = aux[q + 2u];
        let sn = aux[q + 3u];
        let qx = (du * cs + dv * sn) * aux[q + 4u];
        let qy = (dv * cs - du * sn) * aux[q + 5u];
        let rho = sqrt(qx * qx + qy * qy);
        return 1.0 - local_step(aux[q + 6u], 1.0, rho);
    }
    if (kind == 2u) {
        return brush_sample(q, u, v);
    }
    if (kind == 3u) {
        // 色域の範囲（見本の数・lo・hi・見本 5 つ）。
        let n = u32(aux[q]);
        let lo = aux[q + 1u];
        let hi = aux[q + 2u];
        var c = 0.0;
        for (var i = 0u; i < n; i = i + 1u) {
            let da = ch.x - aux[q + 3u + 2u * i];
            let db = ch.y - aux[q + 4u + 2u * i];
            let d = sqrt(da * da + db * db);
            c = max(c, 1.0 - local_step(lo, hi, d));
        }
        return c;
    }
    // 輝度の範囲（lo・hi・滑らかさ）。
    let lo = aux[q];
    let hi = aux[q + 1u];
    let s = aux[q + 2u];
    if (s > 0.0) {
        return local_step(lo - s, lo, luma) * (1.0 - local_step(hi, hi + s, luma));
    }
    return select(0.0, 1.0, lo <= luma && luma <= hi);
}

@compute @workgroup_size(8, 8)
fn local_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let g = out_global(gid);
    // local::g_coords。
    let u = (f32(pi(P_LOCAL_OX) + g.x) + 0.5) * pf(P_LOCAL_SX);
    let v = (f32(pi(P_LOCAL_OY) + g.y) + 0.5) * pf(P_LOCAL_SY);
    var ch = vec2<f32>(0.0, 0.0);
    var luma = 0.0;
    if (pu(P_LOCAL_RANGE) != 0u) {
        // local::range_inputs。
        let c = load_global(g);
        let lab = oklab_from_bt2020(c);
        let d = max(lab.x, COLOR_L_FLOOR);
        ch = vec2<f32>(lab.y / d, lab.z / d);
        let y = luminance(c) * pf(P_LOCAL_GAIN);
        luma = srgb_encode(clamp(s2d_curve(y), 0.0, 1.0));
    }
    let k = pu(P_LOCAL_PLANES);
    var acc: array<f32, 13>;
    for (var p = 0u; p < 13u; p = p + 1u) {
        acc[p] = 0.0;
    }
    var at = pu(P_LOCAL_DATA);
    let masks = pu(P_LOCAL_MASKS);
    for (var m = 0u; m < masks; m = m + 1u) {
        let nc = u32(aux[at]);
        var w = aux[at + 1u];
        let dofs = at + 2u;
        var co = dofs + k;
        for (var i = 0u; i < nc; i = i + 1u) {
            var x = component_value(co, u, v, ch, luma);
            if (aux[co + 2u] != 0.0) {
                x = 1.0 - x;
            }
            let mode = u32(aux[co + 1u]);
            if (mode == 0u) {
                w = max(w, x);
            } else if (mode == 1u) {
                w = w * (1.0 - x);
            } else {
                w = w * x;
            }
            co = co + 16u;
        }
        if (w != 0.0) {
            for (var p = 0u; p < k; p = p + 1u) {
                acc[p] = acc[p] + w * aux[dofs + p];
            }
        }
        at = co;
    }
    let n = pu(P_OUT_W) * pu(P_OUT_H);
    let idx = gid.y * pu(P_OUT_W) + gid.x;
    for (var p = 0u; p < k; p = p + 1u) {
        dst[p * n + idx] = acc[p];
    }
}
