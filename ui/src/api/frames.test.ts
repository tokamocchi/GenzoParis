import { describe, expect, it } from "vitest";

import { decodeFrame } from "./frames";

function encode(header: object, rgba: number[]): ArrayBuffer {
  const json = new TextEncoder().encode(JSON.stringify(header));
  const buf = new ArrayBuffer(4 + json.length + rgba.length);
  new DataView(buf).setUint32(0, json.length, true);
  new Uint8Array(buf, 4, json.length).set(json);
  new Uint8Array(buf, 4 + json.length).set(rgba);
  return buf;
}

describe("decodeFrame", () => {
  it("ヘッダーと RGBA を読む", () => {
    const f = decodeFrame<{ width: number; height: number; generation: number }>(
      encode({ width: 2, height: 1, generation: 7 }, [1, 2, 3, 255, 4, 5, 6, 255]),
    );
    expect(f?.header.generation).toBe(7);
    expect(Array.from(f!.pixels)).toEqual([1, 2, 3, 255, 4, 5, 6, 255]);
  });
  it("空ならプレビューなし", () => {
    expect(decodeFrame(new ArrayBuffer(0))).toBeNull();
  });
  it("長さが合わなければ例外", () => {
    expect(() => decodeFrame(encode({ width: 2, height: 2 }, [0, 0, 0, 0]))).toThrow();
  });
});
