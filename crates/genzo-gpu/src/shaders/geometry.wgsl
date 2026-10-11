// ステージ 9: ジオメトリ（genzo-pipeline の finish::geometry。向き・回転・切り抜きを 1 回の
// リサンプリングで行う。Keys の 3 次畳み込み（a = −0.5）を自前で補間する。2.3 節）。
//
// 入力のタイルは S の格子（P_IN_*）、出力は C の格子（P_OUT_*）。P_AFF は G の格子の連続座標 →
// 入力の格子の連続座標のアフィン変換（GeometryPlan::lattice_to_input_f32。行ごとに a, b, t）。

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
    let x = pf(P_AFF) * xx + pf(P_AFF + 1u) * yy + pf(P_AFF + 2u);
    let y = pf(P_AFF + 3u) * xx + pf(P_AFF + 4u) * yy + pf(P_AFF + 5u);
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
