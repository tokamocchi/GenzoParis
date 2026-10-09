# 実装の進め方と進捗

設計資料（`docs/01`〜`docs/07`）をもとに、AI が実装を進めるための計画と進捗の記録です。
作業が途中で止まった場合は、この表の「状態」を見て続きから再開します。

- 対象範囲: ロードマップ（`05_poc_and_roadmap.md`）の **M0（開発基盤）の AI の担当分** と、**M2 ＋ M3（現像エンジンとカタログ、CLI）のうち、実機・サンプルデータなしで作って確かめられる部分**。
- 範囲外（人の作業、または実機が必要なもの）: サンプルの撮影、Windows / M1 Mac の実機での計測・目視評価、PoC の合否の判定、UI（PoC-1 / M4）の実機確認。
- 開発環境: Linux のコンテナ。LibRaw・lcms2 は Ubuntu のパッケージ、GPU は Mesa の llvmpipe（Vulkan のソフトウェア実装）で動作を確認する。実機（RTX 3080 / M1）での確認は人が行う。

## crate の構成（04 の 1.4 節）

| crate | 役割 | 主な依存 |
|---|---|---|
| `genzo-model` | ドメインの型、現像設定のスキーマ（2.5 節）、キャッシュキー（4.1 節） | serde, sha2 |
| `genzo-color` | 色空間・行列・伝達関数・Lab・ΔE2000・OKLab、lcms2 による ICC と 3D LUT（2.6 節・5 章） | lcms2 |
| `genzo-raw` | LibRaw の FFI（機能フラグ `libraw`）、RAW の展開結果の型 | cc（C++ のシム） |
| `genzo-media` | 画像の入出力（JPEG / TIFF / PNG と ICC の埋め込み）、ffprobe / ffmpeg、安全な書き出し（6.4 節） | image, tiff, png |
| `genzo-pipeline` | 処理ステージ（7.1 節）と CPU 基準実装、段階 A0〜C とキャッシュ（2.2 節）、タイル処理 | genzo-model, genzo-color |
| `genzo-gpu` | wgpu（WGSL）による GPU 版と、CPU 版との一致テスト（2.3 節） | wgpu |
| `genzo-catalog` | SQLite のスキーマ・マイグレーション・制約・検索・バックアップ（3 章）、サムネイル DB | rusqlite |
| `genzo-jobs` | 優先度付きのジョブスケジューラ、取り消し、メモリの予算（6.1 節） | — |
| `genzo-worker` | ワーカープロセス（展開・メタデータ）と、本体側の管理（タイムアウト・再起動・共有メモリ。1.2 節） | genzo-raw, genzo-media |
| `genzo-api` | コア API（コマンド・イベント。1.5 節）と各サービス | 上記すべて |
| `genzo-cli` | 登録・検索・現像・書き出し・計測のコマンド（ORG-05） | genzo-api |
| `genzo-testkit` | 回帰テストと計測の基盤（M0 タスク 11・12）、合成テスト画像・合成 DNG | genzo-color |

## 進捗

| No | 作業 | 状態 | メモ |
|---|---|---|---|
| 1 | workspace と crate の雛形、`genzo-model` | 未着手 | |
| 2 | CI（最小限）、cargo-deny、第三者の台帳（`docs/third_party.md`） | 未着手 | |
| 3 | `genzo-color` | 未着手 | |
| 4 | `genzo-raw`（LibRaw の FFI） | 未着手 | |
| 5 | `genzo-media` | 未着手 | |
| 6 | `genzo-catalog` | 未着手 | |
| 7 | `genzo-jobs` | 未着手 | |
| 8 | `genzo-testkit` | 未着手 | |
| 9 | `genzo-pipeline`（CPU 基準実装） | 未着手 | |
| 10 | `genzo-worker` | 未着手 | |
| 11 | `genzo-gpu` | 未着手 | |
| 12 | `genzo-api` | 未着手 | |
| 13 | `genzo-cli` | 未着手 | |
| 14 | 全体のレビューと修正 | 未着手 | |
| 15 | PoC の記録のひな形（`docs/poc/`）と README の更新 | 未着手 | |
