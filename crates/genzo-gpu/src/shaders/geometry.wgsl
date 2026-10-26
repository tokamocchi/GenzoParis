// ステージ 9: ジオメトリ（genzo-pipeline の finish::geometry。向き・回転・切り抜きを 1 回の
// リサンプリングで行う。Keys の 3 次畳み込み（a = −0.5）を自前で補間する。2.3 節）。
//
// 入力のタイルは S の格子（P_IN_*）、出力は C の格子（P_OUT_*）。P_AFF は G の格子の連続座標 →
// 入力の格子の連続座標のアフィン変換（GeometryPlan::lattice_to_input_f32。行ごとに a, b, t）。
// P_WARP が 1 なら歪曲補正（処理バージョン 2。GeometryPlan::warp の式）: P_PRE（G の格子 → D から中心を
// 引いたもの）・P_INV_R2・倍率の 1D LUT（aux の LUT 0）・P_POST（中心からの差 → 入力の格子）。
// P_PERSP が 1 なら遠近補正（処理バージョン 2。DEV-16。GeometryPlan::projective の map_f32 の式）: P_HOM（G の格子 →
// D から中心を引いたもの。同次座標の 3 × 3。行優先）・P_DLIM（中心からの差の上限）と、P_WARP が 1 なら歪曲補正の
// 表（P_INV_R2・LUT 0）、P_POST（中心からの差 → 入力の格子）。
//
// 同じモジュールの lens_main はステージ 7（genzo-pipeline の sensor::LensCorrectionStage。周辺減光の
// 利得と R・B の倍率色収差。1D LUT は aux の LUT 0〜2、バイキュービックはこのファイルの関数）。

@group(0) @binding(4) var<storage, read> aux: array<f32>;

// 1D LUT k（aux の P_LUT_OFF[k] から P_LUT_LEN[k] 点。定義域の下端は 0。genzo-pipeline の
// interp::Lut1d::eval と同じ式）。
fn lut_at(k: u32, x: f32) -> f32 {
    let off = pu(P_LUT_OFF + k);
    let len = pu(P_LUT_LEN + k);
    let last = f32(len - 1u);
    let t0 = x * pf(P_LUT_INV + k);
    // NaN は比較が偽になるので 0 にする。
    var t = 0.0;
    if (t0 >= 0.0) {
        t = min(t0, last);
    }
    let i = min(u32(t), len - 2u);
    let f = t - f32(i);
    let a = aux[off + i];
    let b = aux[off + i + 1u];
    return a + (b - a) * f;
}

// 入力の画素 (x, y)（入力の格子の座標）。タイルにない場合は格子の鏡映で折り返した位置、それもなければ
// タイルの最も近い画素（genzo-pipeline の geometry::fetch と同じ）。
fn geometry_fetch(x: i32, y: i32) -> vec3<f32> {
    let rx = pi(P_IN_X);
    let ry = pi(P_IN_Y);
    let w = i32(pu(P_IN_W));
    let h = i32(pu(P_IN_H));
    var lx = x - rx;
    var ly = y - ry;
    if (lx >= 0 && ly >= 0 && lx < w && ly < h) {
        return load_local(u32(lx), u32(ly));
    }
    let mx = i32(mirror_index(x, pu(P_GRID_W)));
    let my = i32(mirror_index(y, pu(P_GRID_H)));
    lx = mx - rx;
    ly = my - ry;
    if (lx >= 0 && ly >= 0 && lx < w && ly < h) {
        return load_local(u32(lx), u32(ly));
    }
    let cx = clamp(mx, rx, rx + w - 1);
    let cy = clamp(my, ry, ry + h - 1);
    return load_local(u32(cx - rx), u32(cy - ry));
}

// Keys の 3 次畳み込みの重み（geometry::cubic_weights）。
fn cubic_weights(t: f32) -> vec4<f32> {
    return vec4<f32>(
        ((-0.5 * t + 1.0) * t - 0.5) * t,
        (1.5 * t - 2.5) * t * t + 1.0,
        ((-1.5 * t + 2.0) * t + 0.5) * t,
        (0.5 * t - 0.5) * t * t,
    );
}

struct CubicBase {
    i: i32,
    t: f32,
}

// 補間のタップの左上の番号と小数部（geometry::cubic_base。i32 なので切り詰めの範囲は 2^30）。
fn cubic_base(v: f32) -> CubicBase {
    let f = select(-0.5, v - 0.5, is_finite_f32(v));
    let i = floor(f);
    let t = f - i;
    return CubicBase(i32(clamp(i, -COORD_LIMIT, COORD_LIMIT)), t);
}

// 1 つのチャンネル ch のバイキュービックの 1 行（geometry::sample_bicubic_channel）。
fn cubic_row_channel(ix: i32, yy: i32, wx: vec4<f32>, ch: u32) -> f32 {
    let p0 = geometry_fetch(ix - 1, yy)[ch];
    let p1 = geometry_fetch(ix, yy)[ch];
    let p2 = geometry_fetch(ix + 1, yy)[ch];
    let p3 = geometry_fetch(ix + 2, yy)[ch];
    return wx.x * p0 + wx.y * p1 + wx.z * p2 + wx.w * p3;
}

// 入力の格子の連続座標 (x, y) の 1 つのチャンネル ch のバイキュービック。
fn bicubic_channel(x: f32, y: f32, ch: u32) -> f32 {
    let bx = cubic_base(x);
    let by = cubic_base(y);
    let wx = cubic_weights(bx.t);
    let wy = cubic_weights(by.t);
    let r0 = cubic_row_channel(bx.i, by.i - 1, wx, ch);
    let r1 = cubic_row_channel(bx.i, by.i, wx, ch);
    let r2 = cubic_row_channel(bx.i, by.i + 1, wx, ch);
    let r3 = cubic_row_channel(bx.i, by.i + 2, wx, ch);
    return wy.x * r0 + wy.y * r1 + wy.z * r2 + wy.w * r3;
}

fn cubic_row(ix: i32, yy: i32, wx: vec4<f32>) -> vec3<f32> {
    let p0 = geometry_fetch(ix - 1, yy);
    let p1 = geometry_fetch(ix, yy);
    let p2 = geometry_fetch(ix + 1, yy);
    let p3 = geometry_fetch(ix + 2, yy);
    return vec3<f32>(
        wx.x * p0.x + wx.y * p1.x + wx.z * p2.x + wx.w * p3.x,
        wx.x * p0.y + wx.y * p1.y + wx.z * p2.y + wx.w * p3.y,
        wx.x * p0.z + wx.y * p1.z + wx.z * p2.z + wx.w * p3.z,
    );
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    // C の画素 → G の格子の画素（切り抜きの左上 + 出力のタイルの位置）。
    let g = out_global(gid);
    let xx = f32(pi(P_CROP_X) + g.x) + 0.5;
    let yy = f32(pi(P_CROP_Y) + g.y) + 0.5;
    var x = 0.0;
    var y = 0.0;
    if (pu(P_PERSP) != 0u) {
        // 遠近補正（ProjectivePlan::map_f32 と同じ式。genzo-pipeline の perspective::W_MIN と同じ下限）。
        let xh = pf(P_HOM) * xx + pf(P_HOM + 1u) * yy + pf(P_HOM + 2u);
        let yh = pf(P_HOM + 3u) * xx + pf(P_HOM + 4u) * yy + pf(P_HOM + 5u);
        let w = pf(P_HOM + 6u) * xx + pf(P_HOM + 7u) * yy + pf(P_HOM + 8u);
        let inv = 1.0 / max(w, 1e-4);
        let lim = pf(P_DLIM);
        var dx = clamp(xh * inv, -lim, lim);
        var dy = clamp(yh * inv, -lim, lim);
        if (pu(P_WARP) != 0u) {
            let u = (dx * dx + dy * dy) * pf(P_INV_R2);
            let k = lut_at(0u, u);
            dx = k * dx;
            dy = k * dy;
        }
        x = pf(P_POST) * dx + pf(P_POST + 1u) * dy + pf(P_POST + 2u);
        y = pf(P_POST + 3u) * dx + pf(P_POST + 4u) * dy + pf(P_POST + 5u);
    } else if (pu(P_WARP) != 0u) {
        // 歪曲補正（GeometryPlan::warp の map_f32 と同じ式）。
        let dx = pf(P_PRE) * xx + pf(P_PRE + 1u) * yy + pf(P_PRE + 2u);
        let dy = pf(P_PRE + 3u) * xx + pf(P_PRE + 4u) * yy + pf(P_PRE + 5u);
        let u = (dx * dx + dy * dy) * pf(P_INV_R2);
        let k = lut_at(0u, u);
        let ex = k * dx;
        let ey = k * dy;
        x = pf(P_POST) * ex + pf(P_POST + 1u) * ey + pf(P_POST + 2u);
        y = pf(P_POST + 3u) * ex + pf(P_POST + 4u) * ey + pf(P_POST + 5u);
    } else {
        x = pf(P_AFF) * xx + pf(P_AFF + 1u) * yy + pf(P_AFF + 2u);
        y = pf(P_AFF + 3u) * xx + pf(P_AFF + 4u) * yy + pf(P_AFF + 5u);
    }
    let bx = cubic_base(x);
    let by = cubic_base(y);
    let wx = cubic_weights(bx.t);
    let wy = cubic_weights(by.t);
    let r0 = cubic_row(bx.i, by.i - 1, wx);
    let r1 = cubic_row(bx.i, by.i, wx);
    let r2 = cubic_row(bx.i, by.i + 1, wx);
    let r3 = cubic_row(bx.i, by.i + 2, wx);
    let c = vec3<f32>(
        wy.x * r0.x + wy.y * r1.x + wy.z * r2.x + wy.w * r3.x,
        wy.x * r0.y + wy.y * r1.y + wy.z * r2.y + wy.w * r3.y,
        wy.x * r0.z + wy.y * r1.z + wy.z * r2.z + wy.w * r3.z,
    );
    store_out(gid, sanitize3(c));
}

// ステージ 7: レンズ補正の周辺減光・倍率色収差（sensor::LensCorrectionStage と apply_lens_correction の式）。
// P_CENTER は中心（入力の格子の連続座標）、P_SPG は格子の 1 画素あたりの S の画素、P_INV_R2 は 1 / R²。
// P_LFLAGS の bit 0 が周辺減光（LUT 0）、bit 1 が色収差（LUT 1 = R、LUT 2 = B）。
@compute @workgroup_size(8, 8)
fn lens_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let g = out_global(gid);
    let cx = pf(P_CENTER);
    let cy = pf(P_CENTER + 1u);
    let gx = f32(g.x) + 0.5 - cx;
    let gy = f32(g.y) + 0.5 - cy;
    let sx = gx * pf(P_SPG);
    let sy = gy * pf(P_SPG);
    let u = (sx * sx + sy * sy) * pf(P_INV_R2);
    let c = load_global(g);
    let flags = pu(P_LFLAGS);
    var r = c.x;
    var b = c.z;
    if ((flags & 2u) != 0u) {
        let kr = lut_at(1u, u);
        let kb = lut_at(2u, u);
        r = bicubic_channel(cx + kr * gx, cy + kr * gy, 0u);
        b = bicubic_channel(cx + kb * gx, cy + kb * gy, 2u);
    }
    var gain = 1.0;
    if ((flags & 1u) != 0u) {
        gain = lut_at(0u, u);
    }
    store_out(gid, sanitize3(vec3<f32>(gain * r, gain * c.y, gain * b)));
}
