import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { MockBridge } from "../api/mock";
import { searchCache } from "./search";

function item(id: number) {
  return { variant_id: id } as never;
}

describe("検索結果のキャッシュ", () => {
  let mock: MockBridge;
  const ranges: string[] = [];
  beforeEach(() => {
    ranges.length = 0;
    mock = new MockBridge();
    installBridge(mock);
    mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
      const c = args?.command as { type: string; generation: number; start: number; len: number };
      ranges.push(`g${c.generation} s${c.start}`);
      return { type: "range", items: [item(c.generation * 100)] };
    });
  });

  it("前の世代のページの読み込み中に世代が変わっても、新しい世代のページを要求して埋める", async () => {
    // 世代 1 の応答だけを遅らせる。
    mock.delay = (c) => ((c.args?.command as { generation: number }).generation === 1 ? 30 : 0);
    searchCache.reset(1, 1);
    expect(searchCache.get(0)).toBeUndefined();
    searchCache.reset(2, 1);
    expect(searchCache.get(0)).toBeUndefined();
    await new Promise((r) => setTimeout(r, 60));
    expect([...ranges].sort()).toEqual(["g1 s0", "g2 s0"]);
    expect(searchCache.peek(0)).toEqual(item(200));
  });

  it("世代が変わった直後の fetch は、新しい世代の項目を返す", async () => {
    mock.delay = (c) => ((c.args?.command as { generation: number }).generation === 3 ? 30 : 0);
    searchCache.reset(3, 1);
    searchCache.get(0);
    searchCache.reset(4, 1);
    expect(await searchCache.fetch(0)).toEqual(item(400));
  });
});
