import { describe, expect, it } from "vitest";

import { L1Status } from "./l1Status";

describe("L1Status（L1 の作成待ち）", () => {
  it("404 になった写真は、作り終えた知らせまで要求しない", () => {
    const s = new L1Status(60_000);
    expect(s.state(7, "r1", 0)).toBe("none");
    expect(s.markMissing(7, "r1", s.stamp(), 10)).toBe(true);
    expect(s.state(7, "r1", 20)).toBe("waiting");
    // ほかの写真には影響しない。
    expect(s.state(8, "r1", 20)).toBe("none");
    s.markUpdated(7);
    expect(s.state(7, "r1", 30)).toBe("none");
  });

  it("rev が変われば（作り直された）要求してよい", () => {
    const s = new L1Status(60_000);
    s.markMissing(7, "r1", s.stamp(), 0);
    expect(s.state(7, "r2", 1)).toBe("none");
    expect(s.state(7, "r1", 1)).toBe("waiting");
  });

  it("要求の後に作り終えていれば、その 404 は使わない（知らせと 404 の順序の入れ替わり）", () => {
    const s = new L1Status(60_000);
    const stamp = s.stamp();
    s.markUpdated(7);
    expect(s.markMissing(7, "r1", stamp, 5)).toBe(false);
    expect(s.state(7, "r1", 6)).toBe("none");
    // 知らせの後に始めた要求の 404 は使う。
    expect(s.markMissing(7, "r1", s.stamp(), 7)).toBe(true);
    expect(s.state(7, "r1", 8)).toBe("waiting");
  });

  it("ほかの写真の知らせは、要求の 404 を古くしない", () => {
    const s = new L1Status(60_000);
    const stamp = s.stamp();
    s.markUpdated(8);
    expect(s.markMissing(7, "r1", stamp, 5)).toBe(true);
  });

  it("作れなかった写真は rev によらず要求しない（作成待ちで上書きしない）", () => {
    const s = new L1Status(60_000);
    s.markMissing(7, "r1", s.stamp(), 0);
    s.markFailed(7, 10);
    expect(s.state(7, "r1", 20)).toBe("failed");
    expect(s.state(7, "r2", 20)).toBe("failed");
    expect(s.markMissing(7, "r1", s.stamp(), 30)).toBe(true);
    expect(s.state(7, "r1", 40)).toBe("failed");
    // 作り終えた知らせで消える。
    s.markUpdated(7);
    expect(s.state(7, "r1", 50)).toBe("none");
  });

  it("期限を過ぎた印は使わない（知らせの取りこぼしへの備え）", () => {
    const s = new L1Status(1000);
    s.markMissing(7, "r1", s.stamp(), 0);
    expect(s.state(7, "r1", 999)).toBe("waiting");
    expect(s.state(7, "r1", 1000)).toBe("none");
    s.markFailed(8, 0);
    expect(s.state(8, null, 1500)).toBe("none");
    // 期限を過ぎた失敗の印は、新しい 404 で作成待ちにする。
    s.markFailed(9, 0);
    s.markMissing(9, "r1", s.stamp(), 2000);
    expect(s.state(9, "r1", 2001)).toBe("waiting");
  });

  it("clear・reset で忘れる", () => {
    const s = new L1Status(60_000);
    s.markMissing(7, "r1", s.stamp(), 0);
    s.markMissing(8, "r1", s.stamp(), 0);
    s.clear(7);
    expect(s.state(7, "r1", 1)).toBe("none");
    expect(s.state(8, "r1", 1)).toBe("waiting");
    s.reset();
    expect(s.state(8, "r1", 1)).toBe("none");
  });

  it("覚えておく数に上限がある（古いものから忘れる）", () => {
    const s = new L1Status(60_000, 2);
    s.markMissing(1, "r", s.stamp(), 0);
    s.markMissing(2, "r", s.stamp(), 0);
    s.markMissing(3, "r", s.stamp(), 0);
    expect(s.state(1, "r", 1)).toBe("none");
    expect(s.state(2, "r", 1)).toBe("waiting");
    expect(s.state(3, "r", 1)).toBe("waiting");
  });
});
