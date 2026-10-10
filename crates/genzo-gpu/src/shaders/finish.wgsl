// 段階 C の画素ごとのステージ（docs/04_architecture.md の 2.1 節のステージ 10・11・13・15・16）。
// 式と演算の順序は genzo-pipeline の finish の各モジュールの doc と同じ。
//
// binding 4（aux）: ステージ 11 はガイド（G 座標の log2 の輝度。行優先）、ステージ 16 は 1D LUT の表。

@group(0) @binding(4) var<storage, read> aux: array<f32>;

// ---------------------------------------------------------------------------
// ステージ 10: 露光量・コントラスト（finish::exposure）
// ---------------------------------------------------------------------------

@compute @workgroup_size(8, 8)
fn exposure_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let k = pf(P_GAIN);
    let c = load_global(out_global(gid));
    store_out(gid, sanitize3(vec3<f32>(c.x * k, c.y * k, c.z * k)));
}

// ContrastParams::apply_log2: min(L0 + γ·(l − L0), 64)。
fn contrast_log2(gamma: f32, l: f32) -> f32 {
    return min(LOG2_MIDDLE_GRAY + gamma * (l - LOG2_MIDDLE_GRAY), LOG2_CEILING);
}

@compute @workgroup_size(8, 8)
fn contrast_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let c = load_global(out_global(gid));
    let l = safe_log2(luminance(c));
    let r = exp2(contrast_log2(pf(P_GAMMA), l) - l);
    store_out(gid, sanitize3(vec3<f32>(c.x * r, c.y * r, c.z * r)));
}

// ---------------------------------------------------------------------------
// ステージ 11: トーン（finish::tone）
// ---------------------------------------------------------------------------

// σ(z) = 1 / (1 + exp2(−z))。
fn sigmoid2(z: f32) -> f32 {
    return 1.0 / (1.0 + exp2(-z));
}

// ToneStageParams::log2_gain。
fn tone_log2_gain(base: f32, l: f32) -> f32 {
    let detail = l - base;
    let b = base - LOG2_MIDDLE_GRAY;
    let base2 = base
        + pf(P_TONE_H) * HIGHLIGHTS_MAX_EV * sigmoid2((b - HIGHLIGHTS_CENTER_EV) / HIGHLIGHTS_WIDTH_EV)
        + pf(P_TONE_S) * SHADOWS_MAX_EV * sigmoid2(-(b - SHADOWS_CENTER_EV) / SHADOWS_WIDTH_EV);
    let l1 = base2 + detail;
    let d1 = l1 - LOG2_MIDDLE_GRAY;
    let l2 = min(
        l1
            + pf(P_TONE_W) * WHITES_MAX_EV * sigmoid2((d1 - WHITES_CENTER_EV) / WHITES_WIDTH_EV)
            + pf(P_TONE_B) * BLACKS_MAX_EV * sigmoid2(-(d1 - BLACKS_CENTER_EV) / BLACKS_WIDTH_EV),
        LOG2_CEILING,
    );
    return l2 - l;
}

// ガイドの画素 (x, y)（端の外は鏡映済みの番号）。
fn guide_at(x: u32, y: u32) -> f32 {
    return aux[y * pu(P_GUIDE_W) + x];
}

// ガイドを連続座標 (x, y) で双線形に引く（genzo-pipeline の interp::sample_bilinear_gray）。
fn sample_guide(x: f32, y: f32) -> f32 {
    let w = pu(P_GUIDE_W);
    let h = pu(P_GUIDE_H);
    let fx = select(-0.5, x - 0.5, is_finite_f32(x));
    let fy = select(-0.5, y - 0.5, is_finite_f32(y));
    let x0f = floor(fx);
    let y0f = floor(fy);
    let tx = fx - x0f;
    let ty = fy - y0f;
    // 極端な座標で x0 + 1 があふれないよう、鏡映の周期より十分大きい範囲に切り詰める
    // （CPU 版は i64 で 2^40。i32 の GPU 版は 2^30）。
    let x0 = i32(clamp(x0f, -COORD_LIMIT, COORD_LIMIT));
    let y0 = i32(clamp(y0f, -COORD_LIMIT, COORD_LIMIT));
    let xa = mirror_index(x0, w);
    let xb = mirror_index(x0 + 1, w);
    let ya = mirror_index(y0, h);
    let yb = mirror_index(y0 + 1, h);
    let a = guide_at(xa, ya);
    let b = guide_at(xb, ya);
    let c = guide_at(xa, yb);
    let d = guide_at(xb, yb);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    return top + (bottom - top) * ty;
}

@compute @workgroup_size(8, 8)
fn tone_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let g = out_global(gid);
    let c = load_global(g);
    // GuideLookup::coords: ((origin + x) as f32 + 0.5) × scale。
    let u = (f32(pi(P_GUIDE_OX) + g.x) + 0.5) * pf(P_GUIDE_SX);
    let v = (f32(pi(P_GUIDE_OY) + g.y) + 0.5) * pf(P_GUIDE_SY);
    let guide_value = sample_guide(u, v);
    let l = safe_log2(luminance(c));
    let base = contrast_log2(pf(P_GAMMA), guide_value + pf(P_TONE_EV));
    let r = exp2(tone_log2_gain(base, l));
    store_out(gid, sanitize3(vec3<f32>(c.x * r, c.y * r, c.z * r)));
}

// ---------------------------------------------------------------------------
// ステージ 13: 彩度・自然な彩度（finish::color）
// ---------------------------------------------------------------------------

// ColorStageParams::chroma_gain。
fn chroma_gain(da: f32, db: f32) -> f32 {
    let ks = 1.0 + pf(P_SAT);
    let vib = pf(P_VIB);
    if (vib == 0.0) {
        return ks;
    }
    let c = sqrt(da * da + db * db);
    let x = 1.0 - min(c / VIBRANCE_CHROMA_REF, 1.0);
    let wc = x * x;
    var skin = 0.0;
    if (vib > 0.0 && c > 0.0) {
        let cos_d = (da * pf(P_SKIN_COS) + db * pf(P_SKIN_SIN)) / c;
        let cw = pf(P_SKIN_COS_W);
        let t = max((cos_d - cw) / (1.0 - cw), 0.0);
        skin = t * t;
    }
    let kv = 1.0 + vib * VIBRANCE_MAX * wc * (1.0 - SKIN_PROTECTION * skin);
    return ks * kv;
}

@compute @workgroup_size(8, 8)
fn color_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let lab = oklab_from_bt2020(load_global(out_global(gid)));
    let an = lab.x * pf(P_NEUTRAL_A);
    let bn = lab.x * pf(P_NEUTRAL_B);
    let da = lab.y - an;
    let db = lab.z - bn;
    let k = chroma_gain(da, db);
    store_out(gid, sanitize3(oklab_to_bt2020(vec3<f32>(lab.x, an + da * k, bn + db * k))));
}

// ---------------------------------------------------------------------------
// ステージ 15: シーン → ディスプレイ（finish::scene_to_display）
// ---------------------------------------------------------------------------

fn s2d_curve(y: f32, k: f32) -> f32 {
    if (y <= k) {
        return y;
    }
    let t = min((y - k) / (1.0 - k), S2D_T_MAX);
    return k + (1.0 - k) * (t / (1.0 + t));
}

@compute @workgroup_size(8, 8)
fn scene_to_display_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let knee = pf(P_KNEE);
    var c = load_global(out_global(gid));
    let y = luminance(c);
    var y2 = y;
    if (y > knee) {
        y2 = s2d_curve(y, knee);
        let r = y2 / y;
        c = vec3<f32>(c.x * r, c.y * r, c.z * r);
    }
    let m = max(max(c.x, c.y), c.z);
    if (m > 1.0) {
        let s = (1.0 - y2) / (m - y2);
        c = vec3<f32>(y2 + s * (c.x - y2), y2 + s * (c.y - y2), y2 + s * (c.z - y2));
    }
    store_out(gid, sanitize3(c));
}

// ---------------------------------------------------------------------------
// ステージ 16: トーンカーブ（finish::tone_curve）
// ---------------------------------------------------------------------------

// 1D LUT k（0: 輝度、1〜3: R・G・B）の評価（genzo-pipeline の interp::Lut1d::eval）。
fn lut_eval(k: u32, x: f32) -> f32 {
    let off = pu(P_LUT_OFF + k);
    let len = pu(P_LUT_LEN + k);
    let last = f32(len - 1u);
    let t0 = (x - pf(P_LUT_LO + k)) * pf(P_LUT_INV + k);
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

// 0〜1 の外を傾き 1 で延長した評価（tone_curve::eval_extended）。
fn lut_eval_extended(k: u32, e: f32) -> f32 {
    let inside = clamp(e, 0.0, 1.0);
    return lut_eval(k, inside) + (e - inside);
}

fn curve_channel(k: u32, v: f32) -> f32 {
    return srgb_decode(lut_eval_extended(k, srgb_encode(v)));
}

@compute @workgroup_size(8, 8)
fn tone_curve_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let flags = pu(P_CURVE_FLAGS);
    var c = load_global(out_global(gid));
    if ((flags & 1u) != 0u) {
        let y = luminance(c);
        let y2 = curve_channel(0u, y);
        var r: f32;
        if (y >= LOG_FLOOR) {
            r = abs(y2 - pf(P_BLACK_LIFT)) / y;
        } else {
            r = pf(P_RATIO_MIN);
        }
        c = vec3<f32>(y2 + r * (c.x - y), y2 + r * (c.y - y), y2 + r * (c.z - y));
    }
    if ((flags & 2u) != 0u) {
        c.x = curve_channel(1u, c.x);
    }
    if ((flags & 4u) != 0u) {
        c.y = curve_channel(2u, c.y);
    }
    if ((flags & 8u) != 0u) {
        c.z = curve_channel(3u, c.z);
    }
    store_out(gid, sanitize3(c));
}
