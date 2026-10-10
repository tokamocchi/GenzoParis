// ステージ 8 の色変換と、面積平均の縮小（genzo-pipeline の sensor::color_matrix・resample）。
//
// 縮小の重みの表（binding 4、u32）: [0, 2 × 出力の長さ) は出力の画素ごとの (開始, 数)、P_TAP_OFF から
// (入力の番号, 重みの f32 のビット列) の列（AreaWeights と同じ表。入力の番号の小さい順）。
// 和は入力の番号の小さい順に acc = acc + w × v で足す（resample の決めごと）。

@group(0) @binding(4) var<storage, read> taps: array<u32>;

// ステージ 8: カメラ RGB → 作業色空間（Mat3F32::apply）。
@compute @workgroup_size(8, 8)
fn color_matrix_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    store_out(gid, sanitize3(mat_apply(P_MAT, load_global(out_global(gid)))));
}

// 横方向: 出力の (o, 行) = Σ w × 入力の (番号 − P_IN_X, 行)。出力の行は入力の行と同じ。
@compute @workgroup_size(8, 8)
fn downscale_h_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let o = gid.x;
    let start = taps[2u * o];
    let count = taps[2u * o + 1u];
    var acc = vec3<f32>(0.0);
    for (var t = 0u; t < count; t = t + 1u) {
        let e = pu(P_TAP_OFF) + 2u * (start + t);
        let i = taps[e];
        let w = bitcast<f32>(taps[e + 1u]);
        let p = load_local(i - u32(pi(P_IN_X)), gid.y);
        acc = vec3<f32>(acc.x + w * p.x, acc.y + w * p.y, acc.z + w * p.z);
    }
    store_out(gid, acc);
}

// 縦方向: 出力の (列, oy) = Σ w × 入力の (列, 番号 − P_IN_Y)。oy は全体での行（P_OUT_Y + gid.y）。
@compute @workgroup_size(8, 8)
fn downscale_v_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let oy = u32(pi(P_OUT_Y)) + gid.y;
    let start = taps[2u * oy];
    let count = taps[2u * oy + 1u];
    var acc = vec3<f32>(0.0);
    for (var t = 0u; t < count; t = t + 1u) {
        let e = pu(P_TAP_OFF) + 2u * (start + t);
        let j = taps[e];
        let w = bitcast<f32>(taps[e + 1u]);
        let p = load_local(gid.x, j - u32(pi(P_IN_Y)));
        acc = vec3<f32>(acc.x + w * p.x, acc.y + w * p.y, acc.z + w * p.z);
    }
    store_out(gid, acc);
}
