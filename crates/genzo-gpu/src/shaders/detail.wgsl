// 処理バージョン 2 のステージ 12（テクスチャ・明瞭度・かすみの除去）・14（シャープ）・16 の効果（切り抜き後の周辺光量
// 補正・粒子）（docs/04_architecture.md の 2.1 節）。式と演算の順序は genzo-pipeline の finish::frequency・presence・
// sharpen・effects の doc と同じ。finish.wgsl と同じモジュール（トーン・ガイドの関数を使う）。
//
// - ステージ 12・14 は、輝度の対数の面（luma_log_main）→ 横のぼかし（blur_h_main）→ 縦のぼかし（blur_v_main）の後に、
//   ぼかした面を binding 5（plane。範囲は P_PLANE_X・P_PLANE_Y・P_PLANE_W）で読んで仕上げる。ぼかしの重みは binding 4
//   （aux）。面の値は 1 画素 1 つの f32（行優先）で、src・dst をそのまま 1 チャンネルの配列として使う。
// - ステージ 12 の aux は、ガイド（P_GUIDE_W × P_GUIDE_H）と、その後ろ（P_HAZE_OFF から）のかすみの暗いチャンネル。

@group(0) @binding(5) var<storage, read_write> plane: array<f32>;

// ---------------------------------------------------------------------------
// 共通（finish::frequency）
// ---------------------------------------------------------------------------

// luma_log: log2(max(Y, 0) + LUMA_LOG_OFFSET)。
fn luma_log(c: vec3<f32>) -> f32 {
    return log2(max(luminance(c), 0.0) + LUMA_LOG_OFFSET);
}

// apply_luma_log_delta。
fn apply_luma_log_delta(c: vec3<f32>, delta: f32) -> vec3<f32> {
    let y = luminance(c);
    let k = exp2(delta);
    let yh = max(y, 0.0) + LUMA_LOG_OFFSET;
    let y2 = y + yh * (k - 1.0);
    return vec3<f32>(y2 + k * (c.x - y), y2 + k * (c.y - y), y2 + k * (c.z - y));
}

// soft_limit: x / (1 + |x| / limit)。
fn soft_limit(x: f32, limit: f32) -> f32 {
    return x / (1.0 + abs(x) / limit);
}

// smoothstep（frequency::smoothstep。WGSL の組み込みの関数は精度が規定されないので使わない）。
fn smooth_step(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// 輝度の対数の面: 入力（RGB、範囲 IN）→ 出力（面、範囲 OUT）。
@compute @workgroup_size(8, 8)
fn luma_log_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let c = load_global(out_global(gid));
    dst[gid.y * pu(P_OUT_W) + gid.x] = luma_log(c);
}

// 横のぼかし: 入力（面、範囲 IN）→ 出力（面、範囲 OUT）。窓の中を番号の小さい順に w[k]·v を足す。
@compute @workgroup_size(8, 8)
fn blur_h_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let g = out_global(gid);
    let r = i32(pu(P_BLUR_R));
    let row = u32(g.y - pi(P_IN_Y)) * pu(P_IN_W);
    let x0 = g.x - pi(P_IN_X) - r;
    var acc = 0.0;
    for (var k = 0; k <= 2 * r; k = k + 1) {
        acc = acc + aux[u32(k)] * src[row + u32(x0 + k)];
    }
    dst[gid.y * pu(P_OUT_W) + gid.x] = acc;
}

// 縦のぼかし: 入力（面、範囲 IN）→ 出力（面、範囲 OUT）。
@compute @workgroup_size(8, 8)
fn blur_v_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let g = out_global(gid);
    let r = i32(pu(P_BLUR_R));
    let col = u32(g.x - pi(P_IN_X));
    let y0 = g.y - pi(P_IN_Y) - r;
    var acc = 0.0;
    for (var k = 0; k <= 2 * r; k = k + 1) {
        acc = acc + aux[u32(k)] * src[u32(y0 + k) * pu(P_IN_W) + col];
    }
    dst[gid.y * pu(P_OUT_W) + gid.x] = acc;
}

// ぼかした面の、全体での座標 g（P_PLANE_* の範囲の中であること）の値。
fn plane_at(g: vec2<i32>) -> f32 {
    return plane[u32(g.y - pi(P_PLANE_Y)) * pu(P_PLANE_W) + u32(g.x - pi(P_PLANE_X))];
}

// ---------------------------------------------------------------------------
// ステージ 12: テクスチャ・明瞭度・かすみの除去（finish::presence）
// P_PR_FLAGS: 1 = ぼかし（テクスチャか明瞭度）、2 = テクスチャ、4 = 明瞭度、8 = かすみの除去、16 = ガイドを使う。
// ---------------------------------------------------------------------------

// aux の位置 off からの、ガイドと同じ寸法の面を連続座標 (x, y) で双線形に引く（sample_guide と同じ式）。
fn sample_aux(off: u32, x: f32, y: f32) -> f32 {
    let w = pu(P_GUIDE_W);
    let h = pu(P_GUIDE_H);
    let fx = select(-0.5, x - 0.5, is_finite_f32(x));
    let fy = select(-0.5, y - 0.5, is_finite_f32(y));
    let x0f = floor(fx);
    let y0f = floor(fy);
    let tx = fx - x0f;
    let ty = fy - y0f;
    let x0 = i32(clamp(x0f, -COORD_LIMIT, COORD_LIMIT));
    let y0 = i32(clamp(y0f, -COORD_LIMIT, COORD_LIMIT));
    let xa = mirror_index(x0, w);
    let xb = mirror_index(x0 + 1, w);
    let ya = mirror_index(y0, h);
    let yb = mirror_index(y0 + 1, h);
    let a = aux[off + ya * w + xa];
    let b = aux[off + ya * w + xb];
    let c = aux[off + yb * w + xa];
    let d = aux[off + yb * w + xb];
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    return top + (bottom - top) * ty;
}

// PresenceStageParams::local_base。
fn presence_local_base(g: f32) -> f32 {
    let b = contrast_log2(pf(P_GAMMA), g + pf(P_TONE_EV));
    return min(b + tone_log2_gain(b, b), LOG2_CEILING);
}

// PresenceStageParams::delta。
fn presence_delta(flags: u32, p: f32, ps: f32, g: f32) -> f32 {
    var delta = 0.0;
    if ((flags & 2u) != 0u) {
        delta = delta + pf(P_PR_TEX) * soft_limit(p - ps, TEXTURE_LIMIT);
    }
    if ((flags & 4u) != 0u) {
        let t = presence_local_base(g);
        let bp = log2(exp2(t) + LUMA_LOG_OFFSET);
        let m = (t - LOG2_MIDDLE_GRAY) / CLARITY_MID_WIDTH;
        let w = 1.0 / (1.0 + m * m);
        delta = delta + pf(P_PR_CLA) * w * soft_limit(ps - bp, CLARITY_LIMIT);
    }
    return delta;
}

// PresenceStageParams::dehaze_pixel。
fn dehaze_pixel(c: vec3<f32>, d: f32) -> vec3<f32> {
    let k = pf(P_PR_DEHAZE);
    let a = pf3(P_PR_AIR);
    if (k > 0.0) {
        let t = max(1.0 - DEHAZE_OMEGA * k * d, DEHAZE_T_MIN);
        let keep = 1.0 - t;
        let j = vec3<f32>((c.x - a.x * keep) / t, (c.y - a.y * keep) / t, (c.z - a.z * keep) / t);
        return vec3<f32>(max(j.x, min(c.x, 0.0)), max(j.y, min(c.y, 0.0)), max(j.z, min(c.z, 0.0)));
    }
    let h = -k * DEHAZE_ADD_MAX * (DEHAZE_ADD_FLOOR + (1.0 - DEHAZE_ADD_FLOOR) * d);
    return vec3<f32>(c.x + h * (a.x - c.x), c.y + h * (a.y - c.y), c.z + h * (a.z - c.z));
}

@compute @workgroup_size(8, 8)
fn presence_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let flags = pu(P_PR_FLAGS);
    let g = out_global(gid);
    let c = load_global(g);
    var gv = 0.0;
    var dv = 0.0;
    if ((flags & 16u) != 0u) {
        // GuideLookup::coords。
        let u = (f32(pi(P_GUIDE_OX) + g.x) + 0.5) * pf(P_GUIDE_SX);
        let v = (f32(pi(P_GUIDE_OY) + g.y) + 0.5) * pf(P_GUIDE_SY);
        gv = sample_aux(0u, u, v);
        if ((flags & 8u) != 0u) {
            dv = sample_aux(pu(P_HAZE_OFF), u, v);
        }
    }
    var out = c;
    if ((flags & 1u) != 0u) {
        let delta = presence_delta(flags, luma_log(c), plane_at(g), gv);
        if (delta != 0.0) {
            out = apply_luma_log_delta(c, delta);
        }
    }
    if ((flags & 8u) != 0u) {
        out = dehaze_pixel(out, dv);
    }
    store_out(gid, sanitize3(out));
}

// ---------------------------------------------------------------------------
// ステージ 14: シャープ（finish::sharpen）
// ---------------------------------------------------------------------------

// SharpenStageParams::shaped。
fn sharpen_shaped(d: f32) -> f32 {
    let t = pf(P_SH_THRESHOLD);
    var f = d;
    if (t > 0.0) {
        let d2 = d * d;
        f = d * d2 / (d2 + t * t);
    }
    return soft_limit(f, pf(P_SH_LIMIT));
}

@compute @workgroup_size(8, 8)
fn sharpen_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let g = out_global(gid);
    let c = load_global(g);
    let mask_edge = pf(P_SH_MASK);
    var m = 1.0;
    if (mask_edge > 0.0) {
        let gx = (plane_at(g + vec2<i32>(1, 0)) - plane_at(g - vec2<i32>(1, 0))) * 0.5;
        let gy = (plane_at(g + vec2<i32>(0, 1)) - plane_at(g - vec2<i32>(0, 1))) * 0.5;
        let e = sqrt(gx * gx + gy * gy) * pf(P_SH_GSCALE);
        m = smooth_step(0.5 * mask_edge, 1.5 * mask_edge, e);
    }
    let delta = pf(P_SH_AMOUNT) * m * sharpen_shaped(luma_log(c) - plane_at(g));
    if (delta == 0.0) {
        store_out(gid, sanitize3(c));
        return;
    }
    store_out(gid, sanitize3(apply_luma_log_delta(c, delta)));
}

// ---------------------------------------------------------------------------
// ステージ 16 の効果: 切り抜き後の周辺光量補正・粒子（finish::effects）
// P_FX_FLAGS: 1 = 周辺光量補正、2 = 粒子。
// ---------------------------------------------------------------------------

// VignetteParams::weight。
fn vignette_weight(g: vec2<i32>) -> f32 {
    let u = abs((f32(g.x) + 0.5) - pf(P_VG_CENTER)) * pf(P_VG_INV);
    let v = abs((f32(g.y) + 0.5) - pf(P_VG_CENTER + 1u)) * pf(P_VG_INV + 1u);
    let q = pf(P_VG_EXP);
    let s = exp2(q * log2(max(u, 1e-30))) + exp2(q * log2(max(v, 1e-30)));
    let d = exp2(log2(max(s, 1e-30)) / q);
    return smooth_step(pf(P_VG_EDGE), pf(P_VG_EDGE + 1u), d * pf(P_VG_INV_CORNER));
}

// VignetteParams::apply。
fn vignette_apply(c: vec3<f32>, w: f32) -> vec3<f32> {
    let amount = pf(P_VG_AMOUNT);
    if (amount < 0.0) {
        let protect = pf(P_VG_HL) * smooth_step(VIGNETTE_HL_LO, VIGNETTE_HL_HI, luminance(c));
        let k = 1.0 - (-amount) * VIGNETTE_DARK_MAX * w * (1.0 - protect);
        return vec3<f32>(c.x * k, c.y * k, c.z * k);
    }
    let k = amount * VIGNETTE_LIGHT_MAX * w;
    return vec3<f32>(c.x + k * (1.0 - c.x), c.y + k * (1.0 - c.y), c.z + k * (1.0 - c.z));
}

// effects::tent_cdf。
fn tent_cdf(t: f32) -> f32 {
    if (t <= -1.0) {
        return 0.0;
    }
    if (t <= 0.0) {
        let a = t + 1.0;
        return 0.5 * a * a;
    }
    if (t < 1.0) {
        let a = 1.0 - t;
        return 1.0 - 0.5 * a * a;
    }
    return 1.0;
}

// effects::grain_span: (正確に平均する範囲の下端, 上端, 倍率)。
fn grain_span(a: f32, b: f32) -> vec3<f32> {
    let span = b - a;
    if (span > GRAIN_MAX_SPAN) {
        let c = 0.5 * (a + b);
        let h = 0.5 * GRAIN_MAX_SPAN;
        return vec3<f32>(c - h, c + h, sqrt(GRAIN_MAX_SPAN / span));
    }
    return vec3<f32>(a, b, 1.0);
}

// GrainParams::octave。
fn grain_octave(o: u32, x0: f32, x1: f32, y0: f32, y1: f32) -> f32 {
    let k = pf(P_GR_INV_CELL + o);
    let sx = grain_span(x0 * k, x1 * k);
    let sy = grain_span(y0 * k, y1 * k);
    let wx = sx.y - sx.x;
    let wy = sy.y - sy.x;
    let i0 = i32(floor(sx.x));
    let i1 = i32(ceil(sx.y));
    let j0 = i32(floor(sy.x));
    let j1 = i32(ceil(sy.y));
    let salt = pu(P_GR_SALT) + o;
    let seed = pu(P_GR_SEED);
    var acc = 0.0;
    for (var j = j0; j <= j1; j = j + 1) {
        let fj = f32(j);
        let wyj = (tent_cdf(sy.y - fj) - tent_cdf(sy.x - fj)) / wy;
        for (var i = i0; i <= i1; i = i + 1) {
            let fi = f32(i);
            let wxi = (tent_cdf(sx.y - fi) - tent_cdf(sx.x - fi)) / wx;
            acc = acc + dither_noise(i, j, salt, seed) * (wxi * wyj);
        }
    }
    return acc * (sx.z * sy.z);
}

// GrainParams::value。
fn grain_value(g: vec2<i32>) -> f32 {
    let gx = pi(P_GR_ORIGIN) + g.x;
    let gy = pi(P_GR_ORIGIN + 1u) + g.y;
    let x0 = f32(gx) * pf(P_GR_INV_DENSITY);
    let x1 = f32(gx + 1) * pf(P_GR_INV_DENSITY);
    let y0 = f32(gy) * pf(P_GR_INV_DENSITY + 1u);
    let y1 = f32(gy + 1) * pf(P_GR_INV_DENSITY + 1u);
    var n = pf(P_GR_W) * grain_octave(0u, x0, x1, y0, y1);
    if (pf(P_GR_W + 1u) != 0.0) {
        n = n + pf(P_GR_W + 1u) * grain_octave(1u, x0, x1, y0, y1);
    }
    return n;
}

// GrainParams::apply。
fn grain_apply(c: vec3<f32>, n: f32) -> vec3<f32> {
    let y = luminance(c);
    let e = srgb_encode(clamp(y, 0.0, 1.0));
    let e2 = clamp(e + pf(P_GR_STRENGTH) * n * (4.0 * e * (1.0 - e)), 0.0, 1.0);
    let y2 = srgb_decode(e2);
    if (y > LOG_FLOOR) {
        let k = y2 / y;
        return vec3<f32>(c.x * k, c.y * k, c.z * k);
    }
    let d = y2 - y;
    return vec3<f32>(c.x + d, c.y + d, c.z + d);
}

@compute @workgroup_size(8, 8)
fn effects_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!in_output(gid)) {
        return;
    }
    let flags = pu(P_FX_FLAGS);
    let g = out_global(gid);
    var c = load_global(g);
    if ((flags & 1u) != 0u) {
        c = vignette_apply(c, vignette_weight(g));
    }
    if ((flags & 2u) != 0u) {
        c = grain_apply(c, grain_value(g));
    }
    store_out(gid, sanitize3(c));
}
