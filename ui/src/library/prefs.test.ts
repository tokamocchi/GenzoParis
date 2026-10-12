import { describe, expect, it } from "vitest";

import { catalogKey, hashString } from "./prefs";

describe("カタログごとの鍵", () => {
  it("FNV-1a（32 bit）のハッシュを 16 進 8 桁にする", () => {
    // FNV-1a の基準値（空文字列はオフセットの値そのもの、"a" は 0xe40c292c）。
    expect(hashString("")).toBe("811c9dc5");
    expect(hashString("a")).toBe("e40c292c");
    expect(hashString("/home/user/写真/catalog.genzo")).toMatch(/^[0-9a-f]{8}$/);
  });

  it("カタログのパスで鍵を分け、カタログを開いていなければ null", () => {
    const a = catalogKey("folderOpen", "/home/user/a/catalog.genzo");
    const b = catalogKey("folderOpen", "/home/user/b/catalog.genzo");
    expect(a).toMatch(/^folderOpen\.[0-9a-f]{8}$/);
    expect(a).not.toBe(b);
    expect(catalogKey("folderOpen", "/home/user/a/catalog.genzo")).toBe(a);
    expect(catalogKey("folderOpen", null)).toBeNull();
    expect(catalogKey("folderOpen", "")).toBeNull();
  });
});
