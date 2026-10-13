// ステージ 6: ノイズ軽減（genzo-pipeline の sensor::denoise。処理バージョン 2）。CPU 版と同じ式・順序で、1 回の
// ディスパッチで 1 つの手順を行う:
//
// - nr_forward_main: RGB（B1）→ 平方根の領域の (Y, D1, D2)（1 画素 3 個の f32。RGB と同じ並び）。
// - nr_smooth_h_main / nr_smooth_v_main: 段 j の B3 スプラインの 5 タップ（間隔 P_HOLE）を横・縦にかける。P_MASK の
//   ビット（1: Y、2: D1、4: D2）が 0 の面は計算せず、中心の値をそのまま写す（量が 0 の面は CPU 版と同じく元の値になる）。
// - nr_detail_main: 細部 c_j − c_{j+1} を縮小して足す（binding 1 の dst は足し込む先。binding 0 が c_j、binding 4 が
//   c_{j+1}。範囲は P_AUX_*）。
// - nr_inverse_main: c_L ＋ 足し込んだ細部（binding 4）を B1 に戻す。

@group(0) @binding(4) var<storage, read> aux_f32: array<f32>;

fn nr_load_aux(g: vec2<i32>) -> vec3<f32> {
    let i = (u32(g.y - pi(P_AUX_Y)) * pu(P_AUX_W) + u32(g.x - pi(P_AUX_X))) * 3u;
    return vec3<f32>(aux_f32[i], aux_f32[i + 1u], aux_f32[i + 2u]);
}

// B3 スプラインの 5 タップの和（CPU 版の b3_sum と同じ順序）。
fn nr_b3(a0: vec3<f32>, a1: vec3<f32>, a2: vec3<f32>, a3: vec3<f32>, a4: vec3<f32>) -> vec3<f32> {
    let w0 = 0.0625;
    let w1 = 0.25;
    let w2 = 0.375;
    return (((a0 * w0 + a1 * w1) + a2 * w2) + a3 * w1) + a4 * w0;
}

// 面ごとの計算の有無（P_MASK）で、計算しない面は中心の値にする。
fn nr_masked(smoothed: vec3<f32>, center: vec3<f32>) -> vec3<f32> {
    let m = pu(P_MASK);
    return vec3<f32>(
        select(center.x, smoothed.x, (m & 1u) != 0u),
        select(center.y, smoothed.y, (m & 2u) != 0u),
        select(center.z, smoothed.z, (m & 4u) != 0u),
    );
}

// non-negative garrote（CPU 版の garrote と同じ）。
fn nr_garrote(w: f32, t: f32) -> f32 {
    if (t <= 0.0) {
        return w;
    }
    if (abs(w) <= t) {
        return 0.0;
    }
    return w - t * t / w;
}

@compute @workgroup_size(8, 8)
fn nr_forward_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let c = load_global(out_global(gid));
    let eps = pf(P_EPS);
    let s = vec3<f32>(sqrt(max(c.x, 0.0) + eps), sqrt(max(c.y, 0.0) + eps), sqrt(max(c.z, 0.0) + eps));
    store_out(gid, sanitize3(vec3<f32>((s.x + 2.0 * s.y + s.z) * 0.25, s.x - s.y, s.z - s.y)));
}

@compute @workgroup_size(8, 8)
fn nr_smooth_h_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let g = out_global(gid);
    let h = pi(P_HOLE);
    let c = load_global(g);
    let v = nr_b3(
        load_global(g - vec2<i32>(2 * h, 0)),
        load_global(g - vec2<i32>(h, 0)),
        c,
        load_global(g + vec2<i32>(h, 0)),
        load_global(g + vec2<i32>(2 * h, 0)),
    );
    store_out(gid, sanitize3(nr_masked(v, c)));
}

@compute @workgroup_size(8, 8)
fn nr_smooth_v_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let g = out_global(gid);
    let h = pi(P_HOLE);
    let c = load_global(g);
    let v = nr_b3(
        load_global(g - vec2<i32>(0, 2 * h)),
        load_global(g - vec2<i32>(0, h)),
        c,
        load_global(g + vec2<i32>(0, h)),
        load_global(g + vec2<i32>(0, 2 * h)),
    );
    store_out(gid, sanitize3(nr_masked(v, c)));
}

@compute @workgroup_size(8, 8)
fn nr_detail_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let g = out_global(gid);
    let w = load_global(g) - nr_load_aux(g);
    let tl = pf(P_T_LUMA);
    let tc = pf(P_T_CHROMA);
    let i = (gid.y * pu(P_OUT_W) + gid.x) * 3u;
    let acc = vec3<f32>(dst[i], dst[i + 1u], dst[i + 2u]);
    let sum = acc + vec3<f32>(nr_garrote(w.x, tl), nr_garrote(w.y, tc), nr_garrote(w.z, tc));
    store_out(gid, sanitize3(sum));
}

@compute @workgroup_size(8, 8)
fn nr_inverse_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let g = out_global(gid);
    let p = load_global(g) + nr_load_aux(g);
    let gg = p.x - (p.y + p.z) * 0.25;
    let s = vec3<f32>(p.y + gg, gg, p.z + gg);
    let eps = pf(P_EPS);
    let r = max(s.x, 0.0);
    let gr = max(s.y, 0.0);
    let b = max(s.z, 0.0);
    store_out(gid, sanitize3(vec3<f32>(max(r * r - eps, 0.0), max(gr * gr - eps, 0.0), max(b * b - eps, 0.0))));
}
