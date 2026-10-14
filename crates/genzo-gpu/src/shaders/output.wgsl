// ステージ 17a 画面・17b 書き出し（genzo-pipeline の finish::output。docs/04_architecture.md の 2.6 節）。
//
// binding 4（aux）: 17a の 3D LUT の格子の値（R が最も速く変わる並び。1 点 3 個の f32）。3D LUT の補間は lut3d.wgsl
// （独自のルックと共通）。

// 17a: DisplayTransform::apply（色域の圧縮 → IEC 61966-2-1 で符号化 → 3D LUT → 0〜1 に収める）。
@compute @workgroup_size(8, 8)
fn display_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let c = load_global(out_global(gid));
    // DisplayGamut::compress_working: 表示先のリニア RGB → 圧縮 → 作業色空間。
    let d = mat_apply(P_W2D, c);
    let comp = gamut_compress(d, pf3(P_GC_LUMA));
    let g = mat_apply(P_D2W, comp);
    let e = vec3<f32>(srgb_encode(g.x), srgb_encode(g.y), srgb_encode(g.z));
    let o = lut3d_apply(e);
    store_out(gid, sanitize3(vec3<f32>(clamp(o.x, 0.0, 1.0), clamp(o.y, 0.0, 1.0), clamp(o.z, 0.0, 1.0))));
}

// 17b: ExportTransform::apply（行列 → 色域の圧縮 → 伝達関数 → 0〜1 に収める）。
@compute @workgroup_size(8, 8)
fn export_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let c = load_global(out_global(gid));
    let l = gamut_compress(mat_apply(P_MAT, c), pf3(P_GC_LUMA));
    let k = pu(P_TRANSFER);
    store_out(gid, sanitize3(vec3<f32>(
        clamp(transfer_encode(k, l.x), 0.0, 1.0),
        clamp(transfer_encode(k, l.y), 0.0, 1.0),
        clamp(transfer_encode(k, l.z), 0.0, 1.0),
    )));
}
