import { describe, expect, it } from "vitest";

import type { CropRect } from "../api/types";
import {
  MIN_CROP_FRACTION,
  MODEL_MIN_CROP_SIZE,
  aspectKey,
  dragCrop,
  fractionAspect,
  cropPixelAspect,
  gBoxSize,
  geometryInfo,
  infoAtAngle,
  matchAspect,
  maxAspectCrop,
  cropFractionAspect,
  orientedSize,
  perspectiveFrame,
  pixelAspect,
  refitCropForAngle,
  rotatedImageCorners,
  sanitizeCrop,
} from "./cropMath";

const rect = (left: number, top: number, right: number, bottom: number): CropRect => ({ left, top, right, bottom });
const w = (c: CropRect) => c.right - c.left;
const h = (c: CropRect) => c.bottom - c.top;
const inside = (c: CropRect) => c.left >= 0 && c.top >= 0 && c.right <= 1 + 1e-12 && c.bottom <= 1 + 1e-12;

describe("縦横比", () => {
  it("向きが 5〜8 なら幅と高さを入れ替える", () => {
    expect(orientedSize(6000, 4000, 1)).toEqual({ width: 6000, height: 4000 });
    expect(orientedSize(6000, 4000, 6)).toEqual({ width: 4000, height: 6000 });
    expect(orientedSize(6000, 4000, 8)).toEqual({ width: 4000, height: 6000 });
    expect(orientedSize(6000, 4000, 3)).toEqual({ width: 6000, height: 4000 });
  });
  it("G の外接矩形（genzo-pipeline の g_size_px）", () => {
    expect(gBoxSize(6000, 4000, 0)).toEqual({ width: 6000, height: 4000 });
    const g = gBoxSize(6000, 4000, 30);
    const c = Math.cos(Math.PI / 6);
    const s = Math.sin(Math.PI / 6);
    expect(g.width).toBeCloseTo(6000 * c + 4000 * s, 9);
    expect(g.height).toBeCloseTo(6000 * s + 4000 * c, 9);
    // 角度の符号では変わらない。
    expect(gBoxSize(6000, 4000, -30).width).toBeCloseTo(g.width, 9);
  });
  it("画素の縦横比を割合の縦横比にする", () => {
    expect(pixelAspect("original", 6000, 4000)).toBeCloseTo(1.5, 12);
    expect(pixelAspect({ ratio: { w: 4, h: 5 } }, 6000, 4000)).toBeCloseTo(0.8, 12);
    expect(pixelAspect(null, 6000, 4000)).toBeNull();
    // 3:2 の画像（角度 0）で元の比率なら、割合の比は 1（全体のまま）。
    expect(fractionAspect(1.5, 6000, 4000)).toBeCloseTo(1, 12);
    // 1:1 なら割合の幅は高さの 4000 / 6000。
    expect(fractionAspect(1, 6000, 4000)).toBeCloseTo(4000 / 6000, 12);
  });
  it("選択肢のキー", () => {
    expect(aspectKey(null)).toBe("free");
    expect(aspectKey("original")).toBe("original");
    expect(aspectKey({ ratio: { w: 16, h: 9 } })).toBe("16:9");
  });
  it("縦横比を選ぶと、その比で外接矩形に収まる最大の枠になる（中心は今の枠の中心）", () => {
    const k = fractionAspect(1, 6000, 4000)!;
    const c = maxAspectCrop(rect(0, 0, 1, 1), k);
    expect(w(c) / h(c)).toBeCloseTo(k, 12);
    expect(h(c)).toBeCloseTo(1, 12);
    expect((c.left + c.right) / 2).toBeCloseTo(0.5, 12);
    // 端に寄った枠でも最大の大きさにして、内側へずらす。
    const d = maxAspectCrop(rect(0.8, 0.1, 0.95, 0.3), 2);
    expect(w(d)).toBeCloseTo(1, 12);
    expect(h(d)).toBeCloseTo(0.5, 12);
    expect(d.left).toBeCloseTo(0, 12);
    expect(d.top).toBeCloseTo(0, 12);
    expect(inside(d)).toBe(true);
  });
  it("縦横比の計算に使う大きさ（元の画像と向き、なければ切り抜く前のプレビュー）", () => {
    const a = geometryInfo({ source: { width: 6000, height: 4000 }, orientation: 6, angleDeg: 0, uncropped: null })!;
    expect(a).toEqual({ ow: 4000, oh: 6000, gw: 4000, gh: 6000 });
    expect(cropFractionAspect("original", a)).toBeCloseTo(1, 12);
    // 向きが分からなければ、G の外接矩形から O を逆に求める。
    const g = gBoxSize(6000, 4000, 12);
    const b = geometryInfo({ source: null, orientation: null, angleDeg: 12, uncropped: g })!;
    expect(b.ow).toBeCloseTo(6000, 6);
    expect(b.oh).toBeCloseTo(4000, 6);
    expect(geometryInfo({ source: null, orientation: null, angleDeg: 45, uncropped: g })).toBeNull();
    expect(geometryInfo({ source: null, orientation: null, angleDeg: 0, uncropped: null })).toBeNull();
    expect(cropFractionAspect(null, a)).toBeNull();
  });
  it("角度を変えたときは幅を保って高さを合わせ、外に出るなら縮める", () => {
    const c = matchAspect(rect(0.1, 0.1, 0.9, 0.5), 1);
    expect(w(c)).toBeCloseTo(0.6, 12);
    expect(h(c)).toBeCloseTo(0.6, 12);
    expect((c.top + c.bottom) / 2).toBeCloseTo(0.3, 12);
    expect(inside(c)).toBe(true);
  });
});

describe("角度を変えたときの切り抜き（縦横比の固定）", () => {
  const size = { ow: 6000, oh: 4000 };
  const pixelRatioAt = (c: CropRect, angle: number) => {
    const g = gBoxSize(size.ow, size.oh, angle);
    return cropPixelAspect(c, g.width, g.height);
  };
  const oneToOne = { ratio: { w: 1, h: 1 } } as const;
  const square = maxAspectCrop(
    rect(0, 0, 1, 1),
    cropFractionAspect(
      oneToOne,
      infoAtAngle(
        geometryInfo({
          source: { width: 6000, height: 4000 },
          orientation: 1,
          angleDeg: 0,
          uncropped: null,
        }),
        0,
      ),
    ),
  );

  it("割合のまま角度だけ変えると、画素の縦横比が選んだ比からずれる（直す前の振る舞いの確認）", () => {
    // 6000×4000・角度 0 で 1:1 の枠（left 0.1667〜right 0.8333・top 0〜bottom 1）。
    expect(square.left).toBeCloseTo(1 / 6, 12);
    expect(square.right).toBeCloseTo(5 / 6, 12);
    expect(pixelRatioAt(square, 0)).toBeCloseTo(1, 12);
    // そのまま 10° にすると、外接矩形は 6603.4×4981.1 で、画素の比は 0.884。
    const g = gBoxSize(6000, 4000, 10);
    expect(g.width).toBeCloseTo(6603.4, 1);
    expect(g.height).toBeCloseTo(4981.1, 1);
    expect(pixelRatioAt(square, 10)).toBeCloseTo(0.884, 3);
    // 元の比率（切り抜きなし）も 10° で 1.326 になる（本来は 1.5）。
    expect(pixelRatioAt(rect(0, 0, 1, 1), 10)).toBeCloseTo(1.326, 3);
  });
  it("合わせ直すと、どの角度でも選んだ比になる", () => {
    for (const angle of [10, -10, 3.5, 30, 45, -45]) {
      const c = refitCropForAngle(square, oneToOne, size, 0, angle);
      expect(pixelRatioAt(c, angle)).toBeCloseTo(1, 9);
      expect(inside(c)).toBe(true);
      const o = refitCropForAngle(rect(0, 0, 1, 1), "original", size, 0, angle);
      expect(pixelRatioAt(o, angle)).toBeCloseTo(1.5, 9);
      expect(inside(o)).toBe(true);
    }
  });
  it("枠の大きさと外接矩形の中心からの位置を画素で保つ（0° → 10° → 0° で元に戻る）", () => {
    const start = rect(0.3, 0.1, 0.6, 0.55); // 1800×1800 画素（1:1）
    expect(pixelRatioAt(start, 0)).toBeCloseTo(1, 12);
    const at10 = refitCropForAngle(start, oneToOne, size, 0, 10);
    const g = gBoxSize(6000, 4000, 10);
    expect(w(at10) * g.width).toBeCloseTo(1800, 6);
    expect(h(at10) * g.height).toBeCloseTo(1800, 6);
    expect(((at10.left + at10.right) / 2 - 0.5) * g.width).toBeCloseTo((0.45 - 0.5) * 6000, 6);
    expect(((at10.top + at10.bottom) / 2 - 0.5) * g.height).toBeCloseTo((0.325 - 0.5) * 4000, 6);
    const back = refitCropForAngle(at10, oneToOne, size, 10, 0);
    expect(back.left).toBeCloseTo(start.left, 9);
    expect(back.top).toBeCloseTo(start.top, 9);
    expect(back.right).toBeCloseTo(start.right, 9);
    expect(back.bottom).toBeCloseTo(start.bottom, 9);
  });
  it("縦長の画像の元の比率も、行って戻ると全体に戻る", () => {
    const portrait = { ow: 4000, oh: 6000 };
    const at10 = refitCropForAngle(rect(0, 0, 1, 1), "original", portrait, 0, 10);
    const g = gBoxSize(4000, 6000, 10);
    expect(cropPixelAspect(at10, g.width, g.height)).toBeCloseTo(4000 / 6000, 9);
    // 誤差で端からずれず、ちょうど全体に戻る（「切り抜きあり」にならない）。
    expect(refitCropForAngle(at10, "original", portrait, 10, 0)).toEqual(rect(0, 0, 1, 1));
    const landscape = refitCropForAngle(rect(0, 0, 1, 1), "original", size, 0, -10);
    expect(refitCropForAngle(landscape, "original", size, -10, 0)).toEqual(rect(0, 0, 1, 1));
  });
  it("外接矩形に収まらなければ、比を保って縮め、内側へずらす", () => {
    // 30° の外接矩形いっぱいの 16:9 の枠を 0° に戻すと、O より大きいので縮める。
    const k = cropFractionAspect(
      { ratio: { w: 16, h: 9 } },
      infoAtAngle({ ow: 6000, oh: 4000, gw: 6000, gh: 4000 }, 30),
    );
    const big = maxAspectCrop(rect(0.5, 0.5, 0.9, 0.9), k);
    const c = refitCropForAngle(big, { ratio: { w: 16, h: 9 } }, size, 30, 0);
    expect(inside(c)).toBe(true);
    expect(pixelRatioAt(c, 0)).toBeCloseTo(16 / 9, 9);
    expect(Math.max(w(c), h(c))).toBeCloseTo(1, 9);
  });
  it("自由・大きさが分からないときはそのまま", () => {
    const c = rect(0.1, 0.2, 0.7, 0.9);
    expect(refitCropForAngle(c, null, size, 0, 10)).toBe(c);
    expect(refitCropForAngle(c, oneToOne, null, 0, 10)).toBe(c);
  });
});

describe("枠のドラッグ", () => {
  const start = rect(0.2, 0.2, 0.8, 0.8);
  it("移動は大きさを保ち、0〜1 に収める", () => {
    const c = dragCrop(start, "move", 0.5, -0.5, null);
    expect(c.left).toBeCloseTo(0.4, 12);
    expect(c.top).toBe(0);
    expect(w(c)).toBeCloseTo(0.6, 12);
    expect(h(c)).toBeCloseTo(0.6, 12);
  });
  it("自由: 辺はその辺だけ、角は 2 辺を動かし、最小の大きさを保つ", () => {
    expect(dragCrop(start, "e", 0.1, 0.3, null)).toEqual(rect(0.2, 0.2, 0.9, 0.8));
    const c = dragCrop(start, "nw", -0.5, 0.1, null);
    expect(c.left).toBe(0);
    expect(c.top).toBeCloseTo(0.3, 12);
    const tiny = dragCrop(start, "w", 0.9, 0, null);
    expect(w(tiny)).toBeCloseTo(MIN_CROP_FRACTION, 12);
    expect(tiny.right).toBe(0.8);
  });
  it("自由: 最小より狭い枠を端の方向へ動かしても 0〜1 の外に出ない", () => {
    // コアの下限（0.001）は画面の下限（MIN_CROP_FRACTION）より小さいので、保存済みの枠ではありうる。
    const narrow = rect(0, 0.2, 0.005, 0.8);
    const a = dragCrop(narrow, "w", -0.1, 0, null);
    expect(inside(a)).toBe(true);
    expect(a.left).toBe(0);
    expect(a.right).toBe(0.005);
    const b = dragCrop(rect(0.996, 0.2, 1, 0.8), "e", 0.1, 0, null);
    expect(inside(b)).toBe(true);
    const c = dragCrop(rect(0.2, 0, 0.8, 0.004), "nw", -0.1, -0.1, null);
    expect(inside(c)).toBe(true);
    const d = dragCrop(rect(0.2, 0.997, 0.8, 1), "s", 0, 0.3, null);
    expect(inside(d)).toBe(true);
  });
  it("縦横比の固定: 角は反対の角を固定し、比を保つ", () => {
    const k = 1.5;
    const c = dragCrop(rect(0.1, 0.1, 0.4, 0.3), "se", 0.2, 0.05, k);
    expect(c.left).toBe(0.1);
    expect(c.top).toBe(0.1);
    expect(w(c) / h(c)).toBeCloseTo(k, 12);
    // 指した位置（0.6, 0.35）を覆う。
    expect(c.right).toBeGreaterThanOrEqual(0.6 - 1e-12);
    expect(c.bottom).toBeGreaterThanOrEqual(0.35 - 1e-12);
    expect(inside(c)).toBe(true);
  });
  it("縦横比の固定: 端に当たったら比を保って止まる", () => {
    const k = 1;
    const c = dragCrop(rect(0.5, 0.5, 0.6, 0.6), "se", 2, 0.1, k);
    expect(c.right).toBeCloseTo(1, 12);
    expect(c.bottom).toBeCloseTo(1, 12);
    expect(w(c) / h(c)).toBeCloseTo(k, 12);
    const d = dragCrop(rect(0.5, 0.5, 0.6, 0.6), "nw", -2, -0.1, 2);
    expect(d.left).toBeCloseTo(0, 12);
    expect(w(d) / h(d)).toBeCloseTo(2, 12);
    expect(inside(d)).toBe(true);
  });
  it("縦横比の固定: 辺は反対の辺を固定し、もう一方は中心を保つ", () => {
    const k = 1;
    const c = dragCrop(start, "e", -0.2, 0, k);
    expect(c.left).toBe(0.2);
    expect(w(c)).toBeCloseTo(0.4, 12);
    expect(h(c)).toBeCloseTo(0.4, 12);
    expect((c.top + c.bottom) / 2).toBeCloseTo(0.5, 12);
    const d = dragCrop(rect(0.2, 0.1, 0.4, 0.3), "s", 0, 0.5, k);
    expect(d.top).toBe(0.1);
    expect(w(d) / h(d)).toBeCloseTo(k, 12);
    expect(inside(d)).toBe(true);
  });
});

describe("コアに送る形", () => {
  it("0〜1 に収め、順序と最小の大きさを直す", () => {
    expect(sanitizeCrop(rect(-0.1, 0.2, 1.0000001, 0.8))).toEqual(rect(0, 0.2, 1, 0.8));
    expect(sanitizeCrop(rect(0.8, 0.9, 0.2, 0.1))).toEqual(rect(0.2, 0.1, 0.8, 0.9));
    const s = sanitizeCrop(rect(1, 0.5, 1, 0.5));
    expect(s.right - s.left).toBeGreaterThanOrEqual(MODEL_MIN_CROP_SIZE);
    expect(s.right).toBeLessThanOrEqual(1);
    expect(s.bottom - s.top).toBeGreaterThanOrEqual(MODEL_MIN_CROP_SIZE);
  });
});

describe("回転した元の画像の四隅", () => {
  it("角度 0 では外接矩形の四隅", () => {
    expect(rotatedImageCorners(6000, 4000, 0)).toEqual([
      [0, 0],
      [1, 0],
      [1, 1],
      [0, 1],
    ]);
  });
  it("回転しても四隅は外接矩形の辺に接する", () => {
    const c = rotatedImageCorners(6000, 4000, 10);
    const xs = c.map((p) => p[0]);
    const ys = c.map((p) => p[1]);
    expect(Math.min(...xs)).toBeCloseTo(0, 12);
    expect(Math.max(...xs)).toBeCloseTo(1, 12);
    expect(Math.min(...ys)).toBeCloseTo(0, 12);
    expect(Math.max(...ys)).toBeCloseTo(1, 12);
    // 正の角度は画面上の反時計回り: 左上の角（O の (0, 0)）は外接矩形の左の辺に来る。
    expect(c[0][0]).toBeCloseTo(0, 12);
  });
});

describe("変形（遠近補正。DEV-16）の外接矩形と四隅", () => {
  const tilt = { vertical: -40, horizontal: 15, aspect: 20, scale: 100, offset_x: 0, offset_y: 0 };

  it("変形の外接矩形はコアと同じ値（genzo-pipeline の perspective_frame_matches_the_ui_values と同じ数値）", () => {
    const f = perspectiveFrame(600, 400, 7, tilt)!;
    expect(f.gw).toBeCloseTo(716.487918941, 6);
    expect(f.gh).toBeCloseTo(570.003459435, 6);
    const f2 = perspectiveFrame(600, 400, 7, { ...tilt, scale: 120, offset_x: 30, offset_y: -10 })!;
    const [cx, cy] = f2.map(300, 200);
    expect(cx).toBeCloseTo(437.430696754, 6);
    expect(cy).toBeCloseTo(350.322573434, 6);
    const [x0, y0] = f2.map(0, 0);
    expect(x0).toBeCloseTo(18.351208106, 6);
    expect(y0).toBeCloseTo(113.540434291, 6);
    // 拡大縮小・ずらしは外接矩形に入らない。
    expect(f2.gw).toBeCloseTo(f.gw, 9);
    expect(f2.gh).toBeCloseTo(f.gh, 9);
  });

  it("変形を渡すと、外接矩形・角度を変えたときの大きさ・四隅が変形を含む（プレビューを待たずに今の値）", () => {
    expect(gBoxSize(600, 400, 7, null)).toEqual(gBoxSize(600, 400, 7));
    const g = gBoxSize(600, 400, 7, tilt);
    expect(g.width).toBeCloseTo(716.487918941, 6);
    const info = geometryInfo({
      source: { width: 600, height: 400 },
      orientation: 1,
      angleDeg: 0,
      uncropped: null,
      transform: tilt,
    })!;
    expect(info.transform).toEqual(tilt);
    const at7 = infoAtAngle(info, 7)!;
    expect(at7.gw).toBeCloseTo(716.487918941, 6);
    expect(at7.transform).toEqual(tilt);
    // 四隅は外接矩形の中（割合が 0〜1）で、少なくとも 1 つは各辺に接する。
    const corners = rotatedImageCorners(600, 400, 7, tilt);
    for (const [u, v] of corners) {
      expect(u).toBeGreaterThanOrEqual(-1e-9);
      expect(u).toBeLessThanOrEqual(1 + 1e-9);
      expect(v).toBeGreaterThanOrEqual(-1e-9);
      expect(v).toBeLessThanOrEqual(1 + 1e-9);
    }
    expect(Math.min(...corners.map((c) => c[0]))).toBeCloseTo(0, 9);
    expect(Math.max(...corners.map((c) => c[1]))).toBeCloseTo(1, 9);
    // 向きが分からず、変形があるときは求めない（プレビューから逆にたどれない）。
    expect(
      geometryInfo({ source: null, orientation: null, angleDeg: 0, uncropped: { width: 700, height: 500 }, transform: tilt }),
    ).toBeNull();
  });
});
