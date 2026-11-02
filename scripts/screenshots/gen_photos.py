"""README のスクリーンショット（docs/images）に使う、風景らしい画像を手続き的に作る。

実在の写真は使わない（権利と、撮影日時・位置などの私的な情報を README に載せないため）。
使い方: python3 scripts/screenshots/gen_photos.py <出力のフォルダ> [枚数]
出力: <n>.jpg（1500 × 1000。ルーペ・現像のプレビュー）と t<n>.jpg（480 × 320。サムネイル）。
要るもの: Python 3、numpy、Pillow。乱数の種は固定なので、何度作っても同じ画像になる。
"""
import json
import os
import sys

import numpy as np
from PIL import Image, ImageFilter
OUT = sys.argv[1]
W, H = 1500, 1000

def lerp(a, b, t): return a + (b - a) * t
def col(h): h = h.lstrip('#'); return np.array([int(h[i:i+2], 16) for i in (0, 2, 4)], float) / 255
def grad(stops, t):
    t = np.clip(t, 0, 1); out = np.zeros(t.shape + (3,))
    for (p0, c0), (p1, c1) in zip(stops, stops[1:]):
        m = (t >= p0) & (t <= p1); u = ((t - p0) / max(p1 - p0, 1e-6))[..., None]
        out[m] = (col(c0) * (1 - u) + col(c1) * u)[m]
    return out

def ridge(rng, w, base, amp, rough=0.55, octaves=7):
    x = np.linspace(0, 1, w); y = np.full(w, float(base))
    a = amp
    for o in range(octaves):
        f = 2 ** o * rng.uniform(0.8, 1.6); ph = rng.uniform(0, 6.28)
        y += a * (np.sin(x * f * 6.28 + ph) * 0.6 + np.sin(x * f * 13.1 + ph * 1.7) * 0.4)
        a *= rough
    return y

def sky(h, w, stops):
    yy = np.linspace(0, 1, h)[:, None].repeat(w, 1)
    return grad(stops, yy)

def sun(img, cx, cy, r, color, glow=6.0, strength=1.0):
    h, w, _ = img.shape
    yy, xx = np.mgrid[0:h, 0:w]
    d = np.sqrt((xx - cx) ** 2 + (yy - cy) ** 2)
    disk = np.clip((r - d) / 2, 0, 1)[..., None]
    halo = (np.exp(-d / (r * glow)) * 0.8 * strength)[..., None]
    c = col(color)
    img[:] = img * (1 - disk) + c * disk
    img[:] = 1 - (1 - img) * (1 - c * halo)

def layer(img, top, color, haze=None, hazet=0.0):
    h, w, _ = img.shape
    yy = np.arange(h)[:, None]
    m = (yy >= top[None, :]).astype(float)
    c = col(color)
    if haze is not None: c = c * (1 - hazet) + col(haze) * hazet
    # 下に向けて少し暗く
    shade = np.clip((yy - top[None, :]) / h, 0, 1)[..., None]
    cc = c[None, None, :] * (1 - 0.35 * shade)
    img[:] = img * (1 - m[..., None]) + cc * m[..., None]

def reflect(img, horizon, dim=0.75, ripple=True):
    h, w, _ = img.shape
    hz = int(horizon)
    src = img[:hz][::-1].copy()
    n = h - hz
    src = src[:n]
    if ripple:
        rng = np.random.default_rng(3)
        for i in range(n):
            s = int(np.sin(i * 0.35) * (2 + i * 0.02)) + int(rng.normal(0, 1 + i * 0.01))
            src[i] = np.roll(src[i], s, axis=0)
    img[hz:hz + n] = src * dim + np.array([0.02, 0.05, 0.1]) * (1 - dim)

def finish(img, rng, vignette=0.35, grain=0.02, blur=0.6):
    h, w, _ = img.shape
    yy, xx = np.mgrid[0:h, 0:w]
    d = ((xx - w / 2) / (w / 2)) ** 2 + ((yy - h / 2) / (h / 2)) ** 2
    img *= (1 - vignette * np.clip(d - 0.3, 0, 1.5) / 1.5)[..., None]
    img += rng.normal(0, grain, img.shape)
    im = Image.fromarray((np.clip(img, 0, 1) * 255).astype(np.uint8))
    if blur: im = im.filter(ImageFilter.GaussianBlur(blur))
    return im

def scene_sunset_sea(rng):
    img = sky(H, W, [(0, '#2b1d4f'), (0.35, '#a33f5f'), (0.55, '#f08a4b'), (0.62, '#ffd27a'), (1, '#ffd27a')])
    sun(img, W * rng.uniform(0.35, 0.65), H * 0.58, 45, '#fff2c8', glow=5)
    hz = int(H * 0.62)
    # 遠い島（水平線の上に描き、海への映り込みは reflect が作る）
    x = np.arange(W)
    bump = np.exp(-((x - W * 0.8) / (W * 0.13)) ** 2)
    top = hz - bump * (45 + ridge(rng, W, 0, 10) * 0.6)
    top = np.where(bump < 0.05, H, top)
    layer(img, top.astype(int), '#2a1a35')
    reflect(img, hz, dim=0.7)
    return img

def scene_mountain_lake(rng):
    img = sky(H, W, [(0, '#3d78c4'), (0.5, '#a9cdef'), (1, '#e9f2fa')])
    hz = int(H * 0.66)
    pal = ['#8aa7c9', '#5f7fa8', '#3e5b7e', '#26394f']
    for i, c in enumerate(pal):
        top = ridge(rng, W, H * (0.30 + i * 0.09), H * (0.12 - i * 0.02))
        layer(img, top.astype(int), c)
        if i == 0:  # 雪
            yy = np.arange(H)[:, None]
            snow = ((yy >= top[None, :]) & (yy < top[None, :] + 35)).astype(float)[..., None]
            img[:] = img * (1 - snow * 0.75) + np.array([0.95, 0.97, 1.0]) * snow * 0.75
    # 森の帯
    top = ridge(rng, W, hz - 30, 8, rough=0.8, octaves=9)
    layer(img, top.astype(int), '#1d3a2a')
    reflect(img, hz, dim=0.65)
    return img

def scene_night_city(rng):
    img = sky(H, W, [(0, '#050a1f'), (0.6, '#17285a'), (0.75, '#4a3a6a'), (1, '#120c1c')])
    stars = rng.random((H, W)) > 0.9993
    img[stars] = 1
    x = 0
    yy = np.arange(H)
    while x < W:
        bw = int(rng.uniform(40, 120)); bh = int(rng.uniform(H * 0.15, H * 0.55))
        top = H - bh - int(H * 0.08)
        img[top:H, x:x + bw] = col('#0b0f1c') * rng.uniform(0.7, 1.3)
        for wy in range(top + 8, H - 10, 14):
            for wx in range(x + 6, x + bw - 6, 12):
                if rng.random() < 0.38:
                    c = col(rng.choice(['#ffd27a', '#ffe9b0', '#9fd3ff']))
                    img[wy:wy + 6, wx:wx + 5] = c * rng.uniform(0.6, 1)
        x += bw + int(rng.uniform(2, 10))
    hz = int(H * 0.92)
    reflect(img, hz, dim=0.5)
    return img

def scene_forest(rng):
    img = sky(H, W, [(0, '#cfe3e8'), (1, '#f2efe0')])
    for i, (c, b) in enumerate([('#a9c3b0', 0.45), ('#7aa08a', 0.55), ('#4f7a62', 0.66), ('#2e523e', 0.78), ('#183626', 0.9)]):
        top = ridge(rng, W, H * b, H * 0.04, rough=0.7)
        layer(img, top.astype(int), c)
        # 木
        for _ in range(int(25 + i * 18)):
            tx = rng.uniform(0, W); tb = top[int(min(tx, W - 1))]; th = rng.uniform(40, 90) * (1 + i * 0.5)
            tw = th * 0.32
            ys = np.arange(int(tb - th), int(tb) + 1)
            for yv in ys:
                if 0 <= yv < H:
                    half = (yv - (tb - th)) / th * tw / 2
                    x0, x1 = int(max(0, tx - half)), int(min(W, tx + half))
                    img[yv, x0:x1] = col(c) * 0.85
    return img

def scene_dunes(rng):
    img = sky(H, W, [(0, '#5ea3d9'), (0.5, '#bfe0f5'), (1, '#f6e6c8')])
    for i, b in enumerate([0.5, 0.62, 0.75, 0.88]):
        top = ridge(rng, W, H * b, H * 0.05, rough=0.35, octaves=3)
        yy = np.arange(H)[:, None]
        m = (yy >= top[None, :])
        slope = np.gradient(top)[None, :]
        light = np.clip(0.5 - slope * 0.08, 0, 1)
        cc = lerp(col('#a8612e')[None, None, :], col('#f2c07a')[None, None, :], light[..., None])
        cc = cc * (1 - 0.15 * i / 3)
        img[:] = np.where(m[..., None], np.broadcast_to(cc, (H, W, 3)), img)
    return img

def scene_aurora(rng):
    img = sky(H, W, [(0, '#020611'), (0.7, '#0b1a33'), (1, '#0b1a33')])
    stars = rng.random((H, W)) > 0.999
    img[stars] = 0.9
    yy, xx = np.mgrid[0:H, 0:W]
    for k in range(3):
        cy = H * (0.25 + 0.1 * k) + 60 * np.sin(xx / W * 6.28 * rng.uniform(0.8, 1.6) + rng.uniform(0, 6))
        d = (yy - cy)
        band = np.exp(-(np.clip(d, 0, None) / 180) ** 2) * (d > -20) * np.clip((d + 20) / 20, 0, 1)
        band *= 0.6 + 0.4 * np.sin(xx / rng.uniform(20, 50)) ** 2
        c = col(['#3dffa0', '#2ee6c8', '#a86bff'][k])
        img += band[..., None] * c * [0.7, 0.5, 0.35][k]
    top = ridge(rng, W, H * 0.8, H * 0.04)
    layer(img, top.astype(int), '#03070d')
    return img

def scene_flowers(rng):
    img = sky(H, W, [(0, '#4f8fd6'), (0.55, '#cde4f7'), (0.56, '#7fae4c'), (1, '#2f5a1e')])
    hz = int(H * 0.55)
    top = ridge(rng, W, hz, 10)
    layer(img, top.astype(int), '#6e9b45')
    pal = [col(c) for c in ['#ff5a7a', '#ffd23f', '#ffffff', '#c96bff', '#ff8c3a']]
    n = 9000
    ys = hz + (rng.random(n) ** 1.6) * (H - hz)
    xs = rng.random(n) * W
    for x, y in zip(xs, ys):
        r = 1 + (y - hz) / (H - hz) * 9
        c = pal[rng.integers(len(pal))] * rng.uniform(0.75, 1)
        y0, y1, x0, x1 = int(y - r), int(y + r), int(x - r), int(x + r)
        img[max(0, y0):min(H, y1), max(0, x0):min(W, x1)] = c
    return img

def scene_mist(rng):
    img = sky(H, W, [(0, '#d9d4cc'), (1, '#f4efe6')])
    for i, c in enumerate(['#b9b3ad', '#9b958f', '#7a746f', '#56514d', '#33302e']):
        top = ridge(rng, W, H * (0.35 + 0.13 * i), H * 0.06, rough=0.5)
        layer(img, top.astype(int), c)
    sun(img, W * 0.7, H * 0.25, 30, '#fff8ec', glow=8, strength=0.6)
    return img

def scene_sakura(rng):
    img = sky(H, W, [(0, '#9fc6ea'), (1, '#f3e8f0')])
    yy, xx = np.mgrid[0:H, 0:W]
    for _ in range(260):
        cx, cy, r = rng.uniform(0, W), rng.uniform(0, H * 0.75), rng.uniform(20, 90)
        a = np.exp(-(((xx - cx) ** 2 + (yy - cy) ** 2) / (r * r)) ** 2)[..., None] * rng.uniform(0.3, 0.8)
        c = col(rng.choice(['#ffd1e1', '#ffb7cf', '#fbe3ee', '#f59ab8']))
        img[:] = img * (1 - a) + c * a
    # 幹
    x = int(W * 0.18)
    img[int(H * 0.2):, x - 22:x + 22] = col('#3b2a26')
    top = ridge(rng, W, H * 0.9, 6)
    layer(img, top.astype(int), '#6d8a4e')
    return img

def scene_coast(rng):
    img = sky(H, W, [(0, '#2f78c2'), (0.45, '#9fd0f0'), (0.46, '#1d6fa3'), (0.75, '#2fa3b5'), (1, '#e9d8b0')])
    yy, xx = np.mgrid[0:H, 0:W]
    shore = H * 0.78 + 30 * np.sin(xx / 120.0) + 10 * np.sin(xx / 37.0)
    foam = np.exp(-((yy - shore) / 6) ** 2)[..., None]
    img[:] = img * (1 - foam * 0.8) + foam * 0.8
    m = (yy > shore)[..., None]
    img[:] = np.where(m, col('#e7d3a6') * (1 - 0.2 * (yy / H))[..., None], img)
    # 崖
    top = ridge(rng, W, H * 0.3, H * 0.05); x = np.arange(W); top = np.maximum(top, H * 0.3 + (x - W * 0.28) * 1.6); top = np.minimum(top, H)
    layer(img, top.astype(int), '#4a5a3a')
    return img

SCENES = [scene_sunset_sea, scene_mountain_lake, scene_night_city, scene_forest, scene_dunes, scene_aurora,
          scene_flowers, scene_mist, scene_sakura, scene_coast]

if __name__ == '__main__':
    n = int(sys.argv[2]) if len(sys.argv) > 2 else 30
    os.makedirs(OUT, exist_ok=True)
    meta = []
    for i in range(n):
        rng = np.random.default_rng(100 + i)
        f = SCENES[i % len(SCENES)]
        img = f(rng)
        im = finish(img, rng)
        im.save(f'{OUT}/{i + 1}.jpg', quality=90)
        im.resize((480, 320), Image.LANCZOS).save(f'{OUT}/t{i + 1}.jpg', quality=88)
        meta.append(f.__name__)
    with open(f'{OUT}/meta.json', 'w') as fp:
        json.dump(meta, fp)
    print('ok', n)
