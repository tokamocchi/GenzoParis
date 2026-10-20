// 設定の「このアプリについて」（DIST-05）: ライセンスの文書の並び・見出しと、アプリのコマンドの呼び出しのテスト。

import { beforeEach, describe, expect, it } from "vitest";

import * as app from "../api/app";
import { installBridge } from "../api/bridge";
import { MOCK_LICENSE_DOCUMENTS, MockBridge, mockLicenseCommands } from "../api/mock";
import {
  APP_LICENSE_NOTE,
  licenseOptionLabel,
  licenseTitle,
  noLicenseDocumentsMessage,
  orderLicenseDocuments,
} from "./licensesLogic";

describe("ライセンスの文書の並びと見出し", () => {
  it("案内・アプリ自体・Rust・npm・LibRaw の順、ほかはパスの順で後ろ", () => {
    expect(orderLicenseDocuments(MOCK_LICENSE_DOCUMENTS.documents).map((d) => d.path)).toEqual([
      "README.txt",
      "GenzoParis/LICENSE.txt",
      "third-party/rust.txt",
      "third-party/npm.txt",
      "LibRaw/README.txt",
      "LibRaw/LICENSE.LGPL",
      "LibRaw/COPYRIGHT",
      "LibRaw/build-recipe/vcpkg-ports-libraw/portfile.cmake",
    ]);
    // 元の配列は変えない。
    expect(MOCK_LICENSE_DOCUMENTS.documents[0].path).toBe("GenzoParis/LICENSE.txt");
  });

  it("見出しと大きさ", () => {
    expect(licenseTitle("third-party/npm.txt")).toBe("画面のライブラリ（npm のパッケージ）のライセンス");
    expect(licenseTitle("LibRaw/build-recipe/homebrew-libraw.rb")).toBe("LibRaw/build-recipe/homebrew-libraw.rb");
    expect(licenseOptionLabel({ path: "GenzoParis/LICENSE.txt", size: 2048 })).toBe(
      "GenzoParis のライセンス（GPL-3.0-or-later）（2.00 KB）",
    );
    expect(APP_LICENSE_NOTE).toContain("GPL-3.0-or-later");
  });

  it("文書がないときの案内", () => {
    expect(noLicenseDocumentsMessage(null)).toContain("フォルダが見つかりません");
    expect(noLicenseDocumentsMessage("/x/licenses")).toContain("文書がありません");
  });
});

describe("ライセンスの文書のコマンド", () => {
  let mock: MockBridge;
  beforeEach(() => {
    mock = new MockBridge();
    mockLicenseCommands(mock);
    installBridge(mock);
  });

  it("一覧・読む・フォルダを開く", async () => {
    expect(await app.licenseDocuments()).toEqual(MOCK_LICENSE_DOCUMENTS);
    expect(await app.readLicenseDocument("third-party/rust.txt")).toBe("third-party/rust.txt の本文");
    await app.openLicenseFolder();
    expect(mock.calls).toEqual([
      { command: "license_documents", args: undefined },
      { command: "read_license_document", args: { path: "third-party/rust.txt" } },
      { command: "open_license_folder", args: undefined },
    ]);
  });
});
