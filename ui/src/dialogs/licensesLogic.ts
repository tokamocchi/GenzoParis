// 設定の「このアプリについて」（DIST-05）: 同梱したライセンスの文書の並びと見出し、案内の文言。
//
// 文書は配布物の licenses フォルダにある（Windows はインストール先、macOS は .app の Contents/Resources。
// crates/genzo-app/licenses/README.txt）。アプリ（genzo-app の licenses.rs）は一覧と、一覧にある文書の本文だけを返す
// （LibRaw のソースのアーカイブは一覧に載らない。フォルダを開いて使う）。

import type { LicenseDocument } from "../api/types";
import { formatBytes } from "../format";

/** 見出しを決めている文書（この順に並べる。ほかの文書はパスの順で後ろに並べる）。 */
const KNOWN_DOCUMENTS: { path: string; title: string }[] = [
  { path: "README.txt", title: "案内（このフォルダの中身）" },
  { path: "GenzoParis/LICENSE.txt", title: "GenzoParis のライセンス（GPL-3.0-or-later）" },
  { path: "third-party/rust.txt", title: "Rust のライブラリ（crate）のライセンス" },
  { path: "third-party/npm.txt", title: "画面のライブラリ（npm のパッケージ）のライセンス" },
  { path: "third-party/native.txt", title: "同梱した C / C++ のライブラリのライセンス（LibRaw の依存など）" },
  { path: "third-party/ai-models.txt", title: "AI マスクのモデルのライセンス（U²-Net・DeepLab-v3）" },
  { path: "third-party/onnxruntime/LICENSE.txt", title: "ONNX Runtime のライセンス（MIT。AI マスクの推論）" },
  {
    path: "third-party/onnxruntime/ThirdPartyNotices.txt",
    title: "ONNX Runtime が使っているライブラリのライセンス",
  },
  { path: "third-party/vcruntime-terms.txt", title: "Visual C++ のランタイムの使用条件（Windows。インストールのときに同意したもの）" },
  { path: "LibRaw/README.txt", title: "LibRaw の案内（ソース・差し替えの方法）" },
  { path: "LibRaw/LICENSE.LGPL", title: "LibRaw のライセンス（LGPL-2.1）" },
  { path: "LibRaw/COPYRIGHT", title: "LibRaw の著作権表示" },
];

/** アプリ自体のライセンスの案内。 */
export const APP_LICENSE_NOTE =
  "GenzoParis は、GNU General Public License version 3 またはそれ以降の版（GPL-3.0-or-later）で配布しています。";
/** ソースコードの場所。 */
export const SOURCE_URL = "https://github.com/tokamocchi/GenzoParis";
/**
 * LibRaw を同梱した配布物で出す、LibRaw の著作権表示とライセンスの文書の案内（LGPL-2.1 の第 6 節。
 * crates/genzo-app/licenses/LibRaw の COPYRIGHT）。
 */
export const LIBRAW_CREDIT =
  "RAW の展開に LibRaw（Copyright (C) LibRaw LLC）を使っています。LibRaw は GNU Lesser General Public License version 2.1（LGPL-2.1）で配布されていて、全文は下の「LibRaw のライセンス（LGPL-2.1）」で読めます。";

/** LibRaw を同梱した配布物か（LibRaw のライセンスの文書があるか）。 */
export function hasLibrawDocuments(documents: LicenseDocument[]): boolean {
  return documents.some((d) => d.path === "LibRaw/LICENSE.LGPL");
}

/** 文書の見出し（決めていないものはパス）。 */
export function licenseTitle(path: string): string {
  return KNOWN_DOCUMENTS.find((d) => d.path === path)?.title ?? path;
}

/** 選ぶ欄の 1 行（見出しと大きさ）。 */
export function licenseOptionLabel(doc: LicenseDocument): string {
  return `${licenseTitle(doc.path)}（${formatBytes(doc.size)}）`;
}

/** 文書を、見出しを決めているものの順、その後にほかのものをパスの順に並べる。 */
export function orderLicenseDocuments(documents: LicenseDocument[]): LicenseDocument[] {
  const rank = (path: string) => {
    const i = KNOWN_DOCUMENTS.findIndex((d) => d.path === path);
    return i === -1 ? KNOWN_DOCUMENTS.length : i;
  };
  return [...documents].sort((a, b) => rank(a.path) - rank(b.path) || (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
}

/** 文書がないときの案内（開発用のビルドで資源のフォルダに文書を置いていないときなど）。 */
export function noLicenseDocumentsMessage(dir: string | null): string {
  return dir === null
    ? "ライセンスの文書のフォルダが見つかりません（配布物のインストール先・.app の中にあります）。"
    : "ライセンスの文書のフォルダに文書がありません。";
}
