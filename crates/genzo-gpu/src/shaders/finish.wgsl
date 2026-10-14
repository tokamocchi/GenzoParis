// 段階 C の画素ごとのステージ（docs/04_architecture.md の 2.1 節のステージ 10・11・13・15・16）。
// 式と演算の順序は genzo-pipeline の finish の各モジュールの doc と同じ。
//
// binding 4（aux）: ステージ 11 はガイド（G 座標の log2 の輝度。行優先）、ステージ 15・16 は 1D LUT の表。
// ステージ 10・11・13 は、ローカル補正（処理バージョン 2。DEV-18）があれば画素の局所の値（local.wgsl）を足して計算する。

@group(0) @binding(4) var<storage, read> aux: array<f32>;

// ---------------------------------------------------------------------------
// ステージ 10: 露光量・コントラスト（finish::exposure）
// ---------------------------------------------------------------------------

@compute @workgroup_size(8, 8)
fn exposure_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let g = out_global(gid);
    var k = pf(P_GAIN);
    let c = load_global(g);
    if (local_on()) {
        // ExposureParams::apply_local。
        let dt = local_value(LP_TEMPERATURE, g);
        let di = local_value(LP_TINT, g);
        let de = local_value(LP_EXPOSURE, g);
        if (de != 0.0) {
            k = exp2(clamp(pf(P_LOCAL_EV) + de, -LOCAL_EXPOSURE_LIMIT, LOCAL_EXPOSURE_LIMIT));
        }
        if (dt != 0.0 || di != 0.0) {
            let wb = local_wb_gains(dt, di);
            store_out(gid, sanitize3(vec3<f32>(c.x * (k * wb.x), c.y * (k * wb.y), c.z * (k * wb.z))));
            return;
        }
    }
    store_out(gid, sanitize3(vec3<f32>(c.x * k, c.y * k, c.z * k)));
}

// ContrastParams::apply_log2: min(L0 + γ·(l − L0), 64)。
fn contrast_log2(gamma: f32, l: f32) -> f32 {
    return min(LOG2_MIDDLE_GRAY + gamma * (l - LOG2_MIDDLE_GRAY), LOG2_CEILING);
}

// ContrastParams::apply: 輝度の倍率 r と、色の成分の倍率 k（中間グレーより暗い部分は
// CONTRAST_CHROMA_GAIN_MAX まで）。k = r なら RGB × r、それ以外は Y·r + k·(RGB − Y)。
fn contrast_apply(gamma: f32, c: vec3<f32>) -> vec3<f32> {
    let y = luminance(c);
    let l = safe_log2(y);
    let r = exp2(contrast_log2(gamma, l) - l);
    var k = r;
    if (y < MIDDLE_GRAY) {
        k = min(r, CONTRAST_CHROMA_GAIN_MAX);
    }
    if (k == r) {
        return vec3<f32>(c.x * r, c.y * r, c.z * r);
    }
    let y2 = y * r;
    return vec3<f32>(y2 + k * (c.x - y), y2 + k * (c.y - y), y2 + k * (c.z - y));
}

// ContrastParams::with_local の γ（dc は 0 でないこと）。
fn local_gamma(dc: f32) -> f32 {
    return 1.0 + CONTRAST_STRENGTH * add_unit(pf(P_LOCAL_CU), dc, -2.0, 2.0);
}

@compute @workgroup_size(8, 8)
fn contrast_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let g = out_global(gid);
    let c = load_global(g);
    var gamma = pf(P_GAMMA);
    if (local_on()) {
        // ContrastStageParams::apply_local。
        let dc = local_value(LP_CONTRAST, g);
        if (dc == 0.0) {
            if (local_only()) {
                store_out(gid, sanitize3(c));
                return;
            }
        } else {
            gamma = local_gamma(dc);
        }
    }
    store_out(gid, sanitize3(contrast_apply(gamma, c)));
}

// ---------------------------------------------------------------------------
// ステージ 11: トーン（finish::tone）
// ---------------------------------------------------------------------------

// σ(z) = 1 / (1 + exp2(−z))。
fn sigmoid2(z: f32) -> f32 {
    return 1.0 / (1.0 + exp2(-z));
}

// ステージ 11 の値（ToneStageParams のハイライト・シャドウ・白・黒・EV・γ）。
struct ToneP {
    h: f32,
    s: f32,
    w: f32,
    b: f32,
    ev: f32,
    gamma: f32,
}

// 全体の値（P_TONE_*・P_GAMMA）。
fn tone_global() -> ToneP {
    return ToneP(pf(P_TONE_H), pf(P_TONE_S), pf(P_TONE_W), pf(P_TONE_B), pf(P_TONE_EV), pf(P_GAMMA));
}

// ToneStageParams::with_local（局所の明るさの項目の局所の値を足す）。
fn tone_with_local(tp: ToneP, g: vec2<i32>) -> ToneP {
    var o = tp;
    o.h = add_unit(tp.h, local_value(LP_HIGHLIGHTS, g), -2.0, 2.0);
    o.s = add_unit(tp.s, local_value(LP_SHADOWS, g), -2.0, 2.0);
    o.w = add_unit(tp.w, local_value(LP_WHITES, g), -2.0, 2.0);
    o.b = add_unit(tp.b, local_value(LP_BLACKS, g), -2.0, 2.0);
    let de = local_value(LP_EXPOSURE, g);
    if (de != 0.0) {
        o.ev = clamp(tp.ev + de, -LOCAL_EXPOSURE_LIMIT, LOCAL_EXPOSURE_LIMIT);
    }
    let dc = local_value(LP_CONTRAST, g);
    if (dc != 0.0) {
        o.gamma = local_gamma(dc);
    }
    return o;
}

// 局所の明るさの項目（露光量・コントラスト・ハイライト・シャドウ・白・黒）のどれかの局所の値が 0 でないか。
fn local_base_changed(g: vec2<i32>) -> bool {
    return local_value(LP_EXPOSURE, g) != 0.0
        || local_value(LP_CONTRAST, g) != 0.0
        || local_tone_changed(g);
}

// ハイライト・シャドウ・白・黒のどれかの局所の値が 0 でないか。
fn local_tone_changed(g: vec2<i32>) -> bool {
    return local_value(LP_HIGHLIGHTS, g) != 0.0
        || local_value(LP_SHADOWS, g) != 0.0
        || local_value(LP_WHITES, g) != 0.0
        || local_value(LP_BLACKS, g) != 0.0;
}

// ToneStageParams::log2_gain。
fn tone_log2_gain(tp: ToneP, base: f32, l: f32) -> f32 {
    let detail = l - base;
    let b = base - LOG2_MIDDLE_GRAY;
    let base2 = base
        + tp.h * HIGHLIGHTS_MAX_EV * sigmoid2((b - HIGHLIGHTS_CENTER_EV) / HIGHLIGHTS_WIDTH_EV)
        + tp.s * SHADOWS_MAX_EV * sigmoid2(-(b - SHADOWS_CENTER_EV) / SHADOWS_WIDTH_EV);
    let l1 = base2 + detail;
    let d1 = l1 - LOG2_MIDDLE_GRAY;
    let l2 = min(
        l1
            + tp.w * WHITES_MAX_EV * sigmoid2((d1 - WHITES_CENTER_EV) / WHITES_WIDTH_EV)
            + tp.b * BLACKS_MAX_EV * sigmoid2(-(d1 - BLACKS_CENTER_EV) / BLACKS_WIDTH_EV),
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
    var tp = tone_global();
    if (local_on()) {
        // ToneStageParams::apply_local。
        if (local_only() && !local_tone_changed(g)) {
            store_out(gid, sanitize3(c));
            return;
        }
        if (local_base_changed(g)) {
            tp = tone_with_local(tp, g);
        }
    }
    let l = safe_log2(luminance(c));
    let base = contrast_log2(tp.gamma, guide_value + tp.ev);
    let r = exp2(tone_log2_gain(tp, base, l));
    store_out(gid, sanitize3(vec3<f32>(c.x * r, c.y * r, c.z * r)));
}

// ---------------------------------------------------------------------------
// ステージ 13: 彩度・自然な彩度・カラーミキサー・カラーグレーディング・白黒（finish::color）
// P_COLOR_FLAGS: 1 = カラーミキサー、2 = 白黒、4 = 白黒ミキサー、8 = カラーグレーディング、16 = 彩度・自然な彩度。
// 色相の角度・回転の sin / cos は CPU 版と同じ多項式（GPU の atan2・sin・cos の精度に左右されないため）。
// ---------------------------------------------------------------------------

const COLOR_FLAG_MIXER: u32 = 1u;
const COLOR_FLAG_BW: u32 = 2u;
const COLOR_FLAG_BW_MIX: u32 = 4u;
const COLOR_FLAG_GRADING: u32 = 8u;
const COLOR_FLAG_SAT: u32 = 16u;

// ColorStageParams::chroma_gain（sat は彩度 / 100。ローカル補正では局所の値を足したもの）。
fn chroma_gain(sat: f32, da: f32, db: f32) -> f32 {
    let ks = 1.0 + sat;
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

// color::atan_unit（t は 0〜1）。
fn atan_unit(t: f32) -> f32 {
    var u = t;
    var base = 0.0;
    if (t > POLY_TAN_PI_12) {
        u = (t * POLY_SQRT3 - 1.0) / (t + POLY_SQRT3);
        base = POLY_FRAC_PI_6;
    }
    let u2 = u * u;
    var q = POLY_INV9 - u2 * POLY_INV11;
    q = u2 * q - POLY_INV7;
    q = u2 * q + POLY_INV5;
    q = u2 * q - POLY_INV3;
    return base + (u + (u * u2) * q);
}

// color::hue_angle（0〜2π）。
fn hue_angle(da: f32, db: f32) -> f32 {
    let ax = abs(da);
    let ay = abs(db);
    let mx = max(ax, ay);
    let mn = min(ax, ay);
    if (!(mx > 0.0)) {
        return 0.0;
    }
    var a = atan_unit(mn / mx);
    if (ay > ax) {
        a = POLY_FRAC_PI_2 - a;
    }
    if (da < 0.0) {
        a = POLY_PI - a;
    }
    if (db < 0.0) {
        a = POLY_TAU - a;
    }
    return a;
}

// color::small_sin_cos（x = sin、y = cos）。
fn small_sin_cos(theta: f32) -> vec2<f32> {
    let t2 = theta * theta;
    var qs = POLY_INV362880 * t2 - POLY_INV5040;
    qs = qs * t2 + POLY_INV120;
    qs = qs * t2 - POLY_INV6;
    let sn = theta + (theta * t2) * qs;
    var qc = POLY_INV40320 * t2 - POLY_INV720;
    qc = qc * t2 + POLY_INV24;
    qc = qc * t2 - 0.5;
    let cs = 1.0 + t2 * qc;
    return vec2<f32>(sn, cs);
}

// color::chroma_ramp。
fn chroma_ramp(c: f32) -> f32 {
    let r = min(c * POLY_INV_CHROMA_RAMP, 1.0);
    return r * r * (3.0 - 2.0 * r);
}

// 色相の帯（HueBandTable::locate の帯 k と、帯 k + 1 の重み s）。
struct HueBand {
    k: u32,
    s: f32,
}

fn band_locate(h0: f32) -> HueBand {
    var h = h0;
    if (h < pf(P_HUE_C)) {
        h = h + POLY_TAU;
    }
    var k = 0u;
    for (var i = 1u; i < 8u; i = i + 1u) {
        if (h >= pf(P_HUE_C + i)) {
            k = i;
        }
    }
    let t = clamp((h - pf(P_HUE_C + k)) * pf(P_HUE_INV + k), 0.0, 1.0);
    return HueBand(k, t * t * (3.0 - 2.0 * t));
}

// color::band_mix（帯ごとの値はパラメータの位置 base から 8 個）。
fn band_mix(base: u32, b: HueBand) -> f32 {
    let a = pf(base + b.k);
    return a + (pf(base + ((b.k + 1u) & 7u)) - a) * b.s;
}

// GradingParams::step。
fn grading_step(t: f32) -> f32 {
    let u = clamp((t - 0.5) * pf(P_CG_ZONE + 6u) + 0.5, 0.0, 1.0);
    return u * u * (3.0 - 2.0 * u);
}

// GradingParams::zones（シャドウ・中間調・ハイライトの重み）。P_CG_ZONE: 中間グレーの L、0.5 / L、
// 0.5 / (Lw − L)、x0、1 / x0、1 / (1 − x0)、なめらかさ。
fn grading_zones(l: f32) -> vec3<f32> {
    let pivot = pf(P_CG_ZONE);
    var x: f32;
    if (l <= pivot) {
        x = l * pf(P_CG_ZONE + 1u);
    } else {
        x = 0.5 + (l - pivot) * pf(P_CG_ZONE + 2u);
    }
    x = clamp(x, 0.0, 1.0);
    let x0 = pf(P_CG_ZONE + 3u);
    if (x < x0) {
        let m = grading_step(x * pf(P_CG_ZONE + 4u));
        return vec3<f32>(1.0 - m, m, 0.0);
    }
    let h = grading_step((x - x0) * pf(P_CG_ZONE + 5u));
    return vec3<f32>(0.0, 1.0 - h, h);
}

@compute @workgroup_size(8, 8)
fn color_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let g = out_global(gid);
    let c0 = load_global(g);
    var sat = pf(P_SAT);
    var use_sat = (pu(P_COLOR_FLAGS) & COLOR_FLAG_SAT) != 0u;
    if (local_on()) {
        // ColorStageParams::apply_local。
        let ds = local_value(LP_SATURATION, g);
        if (ds == 0.0) {
            if (local_only()) {
                store_out(gid, sanitize3(c0));
                return;
            }
        } else {
            sat = add_unit(pf(P_SAT), ds, -1.0, 2.0);
            use_sat = sat != 0.0 || pf(P_VIB) != 0.0;
        }
    }
    let lab = oklab_from_bt2020(c0);
    let flags = pu(P_COLOR_FLAGS);
    let na = pf(P_NEUTRAL_A);
    let nb = pf(P_NEUTRAL_B);
    var l = lab.x;
    var da = lab.y - l * na;
    var db = lab.z - l * nb;
    if ((flags & COLOR_FLAG_BW) != 0u) {
        if ((flags & COLOR_FLAG_BW_MIX) != 0u) {
            let cc = sqrt(da * da + db * db);
            if (cc > 0.0) {
                let g = chroma_ramp(cc);
                let band = band_locate(hue_angle(da, db));
                let e = band_mix(P_BW_MIX, band) * g;
                if (e != 0.0) {
                    l = l * exp2(e);
                }
            }
        }
        da = 0.0;
        db = 0.0;
    } else {
        if ((flags & COLOR_FLAG_MIXER) != 0u) {
            let cc = sqrt(da * da + db * db);
            if (cc > 0.0) {
                let g = chroma_ramp(cc);
                let band = band_locate(hue_angle(da, db));
                let theta = band_mix(P_HSL_H, band) * g;
                let gain = 1.0 + band_mix(P_HSL_S, band) * g;
                let e = band_mix(P_HSL_L, band) * g;
                if (theta != 0.0) {
                    let sc = small_sin_cos(theta);
                    let ra = da * sc.y - db * sc.x;
                    let rb = da * sc.x + db * sc.y;
                    da = ra;
                    db = rb;
                }
                da = da * gain;
                db = db * gain;
                if (e != 0.0) {
                    let m = exp2(e);
                    l = l * m;
                    da = da * m;
                    db = db * m;
                }
            }
        }
        if (use_sat) {
            let k = chroma_gain(sat, da, db);
            da = da * k;
            db = db * k;
        }
    }
    if ((flags & COLOR_FLAG_GRADING) != 0u) {
        let w = grading_zones(l);
        let ta = w.x * pf(P_CG_TINT) + w.y * pf(P_CG_TINT + 2u) + w.z * pf(P_CG_TINT + 4u) + pf(P_CG_TINT + 6u);
        let tb = w.x * pf(P_CG_TINT + 1u) + w.y * pf(P_CG_TINT + 3u) + w.z * pf(P_CG_TINT + 5u) + pf(P_CG_TINT + 7u);
        let e = w.x * pf(P_CG_EV) + w.y * pf(P_CG_EV + 1u) + w.z * pf(P_CG_EV + 2u) + pf(P_CG_EV + 3u);
        da = da + l * ta;
        db = db + l * tb;
        if (e != 0.0) {
            let m = exp2(e);
            l = l * m;
            da = da * m;
            db = db * m;
        }
    }
    store_out(gid, sanitize3(oklab_to_bt2020(vec3<f32>(l, l * na + da, l * nb + db))));
}

// ---------------------------------------------------------------------------
// ステージ 15: シーン → ディスプレイ（finish::scene_to_display）
// P_S2D_MODE = 1: RAW（輝度のカーブ。対数の軸の 1D LUT を aux の表 0 で引く。2^-16 より暗い輝度は
// 一定の比率 P_S2D_FLOOR_RATIO）。0: RAW 以外（恒等。輝度 1 を超える画素だけ 1 にする）。
// ---------------------------------------------------------------------------

@compute @workgroup_size(8, 8)
fn scene_to_display_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    var c = load_global(out_global(gid));
    let y = luminance(c);
    var y2 = y;
    var r = 1.0;
    if (pu(P_S2D_MODE) == 1u) {
        if (y >= LOG_FLOOR) {
            y2 = lut_eval(0u, log2(y));
            r = y2 / y;
        } else {
            r = pf(P_S2D_FLOOR_RATIO);
            y2 = y * r;
        }
    } else if (y > 1.0) {
        y2 = 1.0;
        r = 1.0 / y;
    }
    c = vec3<f32>(c.x * r, c.y * r, c.z * r);
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
