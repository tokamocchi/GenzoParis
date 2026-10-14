// カメラプロファイル（DCP）の色相彩度のマップ・ルックテーブルと、独自のルック（3D LUT）（DEV-17。処理バージョン 2）。
//
// - hsv_table_main: 行列 MAT → HSV の表（genzo-color の dcp::HsvTable::apply）→ 行列 MAT2。ステージ 8 の色相彩度の
//   マップ（genzo-pipeline の profile::CameraProfilePlan::apply。カメラ RGB → ProPhoto → 表 → 作業色空間）と、
//   ステージ 10 の後のルックテーブル（finish::look::LookTableParams::apply。作業色空間 → ProPhoto → 表 → 作業色空間）で
//   共通。binding 4（aux）は表の点（1 点 3 個の f32。明度 → 色相 → 彩度の並び）。
// - look_main: 独自のルック（finish::look::LookParams::apply）。binding 4（aux）は 3D LUT（lut3d.wgsl）。
//
// 式と演算の順序は CPU 版と同じ（色相の位置の倍率 HSV_HSCALE と、度 → 色相の単位の倍率 HSV_DEG_TO_H は CPU 版と同じ
// f32 の値）。入力は有限であること（GPU 版の前提。genzo-gpu の lib の doc）。

fn hsv_entry(v: u32, h: u32, s: u32) -> vec3<f32> {
    let hn = pu(P_HSV_DIMS);
    let sn = pu(P_HSV_DIMS + 1u);
    let i = ((v * hn + h) * sn + s) * 3u;
    return vec3<f32>(aux[i], aux[i + 1u], aux[i + 2u]);
}

// 明度の 1 つの面の中の双線形（色相 → 彩度の順。HsvTable::lookup の plane）。
fn hsv_plane(vi: u32, h0: u32, h1: u32, s0: u32, hf0: f32, hf1: f32, sf0: f32, sf1: f32) -> vec3<f32> {
    let e00 = hsv_entry(vi, h0, s0);
    let e01 = hsv_entry(vi, h1, s0);
    let e10 = hsv_entry(vi, h0, s0 + 1u);
    let e11 = hsv_entry(vi, h1, s0 + 1u);
    let a = vec3<f32>(hf0 * e00.x + hf1 * e01.x, hf0 * e00.y + hf1 * e01.y, hf0 * e00.z + hf1 * e01.z);
    let c = vec3<f32>(hf0 * e10.x + hf1 * e11.x, hf0 * e10.y + hf1 * e11.y, hf0 * e10.z + hf1 * e11.z);
    return vec3<f32>(sf0 * a.x + sf1 * c.x, sf0 * a.y + sf1 * c.y, sf0 * a.z + sf1 * c.z);
}

// 表を引く（HsvTable::lookup と同じ式・順序）。s は 0〜1。
fn hsv_lookup(h: f32, s: f32, v: f32) -> vec3<f32> {
    let hue_n = pu(P_HSV_DIMS);
    let sat_n = pu(P_HSV_DIMS + 1u);
    let val_n = pu(P_HSV_DIMS + 2u);
    let h_scaled = h * pf(P_HSV_HSCALE);
    let s_scaled = s * f32(sat_n - 1u);
    var h0 = min(u32(max(h_scaled, 0.0)), hue_n - 1u);
    var h1 = h0 + 1u;
    if (h0 >= hue_n - 1u) {
        h0 = hue_n - 1u;
        h1 = 0u;
    }
    let s0 = min(u32(max(s_scaled, 0.0)), sat_n - 2u);
    let hf1 = h_scaled - f32(h0);
    let sf1 = s_scaled - f32(s0);
    let hf0 = 1.0 - hf1;
    let sf0 = 1.0 - sf1;
    if (val_n < 2u) {
        return hsv_plane(0u, h0, h1, s0, hf0, hf1, sf0, sf1);
    }
    var ve = v;
    if (pu(P_HSV_SRGB) != 0u) {
        ve = srgb_encode(v);
    }
    let v_scaled = clamp(ve, 0.0, 1.0) * f32(val_n - 1u);
    let v0 = min(u32(v_scaled), val_n - 2u);
    let vf1 = v_scaled - f32(v0);
    let vf0 = 1.0 - vf1;
    let p0 = hsv_plane(v0, h0, h1, s0, hf0, hf1, sf0, sf1);
    let p1 = hsv_plane(v0 + 1u, h0, h1, s0, hf0, hf1, sf0, sf1);
    return vec3<f32>(vf0 * p0.x + vf1 * p1.x, vf0 * p0.y + vf1 * p1.y, vf0 * p0.z + vf1 * p1.z);
}

// HSV（色相 0〜6）→ RGB（genzo-color の dcp::hsv_to_rgb）。
fn hsv_to_rgb(h_in: f32, s: f32, v: f32) -> vec3<f32> {
    if (s <= 0.0) {
        return vec3<f32>(v, v, v);
    }
    var h = h_in;
    if (h < 0.0) {
        h = h + 6.0;
    }
    if (h >= 6.0) {
        h = h - 6.0;
    }
    if (h < 0.0) {
        h = h + 6.0;
    }
    if (h >= 6.0) {
        h = h - 6.0;
    }
    let i = min(u32(max(h, 0.0)), 5u);
    let f = h - f32(i);
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    if (i == 0u) {
        return vec3<f32>(v, t, p);
    } else if (i == 1u) {
        return vec3<f32>(q, v, p);
    } else if (i == 2u) {
        return vec3<f32>(p, v, t);
    } else if (i == 3u) {
        return vec3<f32>(p, q, v);
    } else if (i == 4u) {
        return vec3<f32>(t, p, v);
    }
    return vec3<f32>(v, p, q);
}

// HSV の表を 1 画素に当てる（HsvTable::apply と同じ式・順序）。
fn hsv_apply(rgb: vec3<f32>) -> vec3<f32> {
    let r = rgb.x;
    let g = rgb.y;
    let b = rgb.z;
    let v = max(max(r, g), b);
    if (v <= 0.0) {
        return rgb;
    }
    let gap = v - min(min(r, g), b);
    var h = 0.0;
    var s = 0.0;
    if (gap > 0.0) {
        if (r == v) {
            h = (g - b) / gap;
            if (h < 0.0) {
                h = h + 6.0;
            }
        } else if (g == v) {
            h = 2.0 + (b - r) / gap;
        } else {
            h = 4.0 + (r - g) / gap;
        }
        s = gap / v;
    }
    let d = hsv_lookup(h, min(s, 1.0), v);
    let h2 = h + d.x * HSV_DEG_TO_H;
    var s2: f32;
    if (s <= 1.0) {
        s2 = min(s * d.y, 1.0);
    } else {
        s2 = min(s * d.y, s);
    }
    var v2: f32;
    if (pu(P_HSV_SRGB) != 0u) {
        v2 = srgb_decode(srgb_encode(v) * d.z);
    } else {
        v2 = v * d.z;
    }
    return hsv_to_rgb(h2, s2, v2);
}

// 行列 MAT → HSV の表 → 行列 MAT2（ステージ 8 の色相彩度のマップと、ステージ 10 の後のルックテーブル）。
@compute @workgroup_size(8, 8)
fn hsv_table_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let c = load_global(out_global(gid));
    let p = mat_apply(P_MAT, c);
    store_out(gid, sanitize3(mat_apply(P_MAT2, hsv_apply(p))));
}

// 独自のルック（finish::look::LookParams::apply。B3 → リニア sRGB → 0〜1 に収めて符号化 → DOMAIN → 3D LUT → リニア →
// 適用量で混ぜて色域の外の分を足し戻す → B3）。
@compute @workgroup_size(8, 8)
fn look_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let c = load_global(out_global(gid));
    let l = mat_apply(P_MAT, c);
    let lc = vec3<f32>(clamp(l.x, 0.0, 1.0), clamp(l.y, 0.0, 1.0), clamp(l.z, 0.0, 1.0));
    let e = vec3<f32>(srgb_encode(lc.x), srgb_encode(lc.y), srgb_encode(lc.z));
    let dmin = pf3(P_DOMAIN_MIN);
    let dsc = pf3(P_DOMAIN_SCALE);
    let pos = vec3<f32>((e.x - dmin.x) * dsc.x, (e.y - dmin.y) * dsc.y, (e.z - dmin.z) * dsc.z);
    let o = lut3d_apply(pos);
    let od = vec3<f32>(srgb_decode(o.x), srgb_decode(o.y), srgb_decode(o.z));
    let a = pf(P_AMOUNT);
    let r = vec3<f32>(l.x + a * (od.x - lc.x), l.y + a * (od.y - lc.y), l.z + a * (od.z - lc.z));
    store_out(gid, sanitize3(mat_apply(P_MAT2, r)));
}
