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
| 1 | workspace と crate の雛形、`genzo-model` | 完了 | |
| 2 | CI（最小限）、cargo-deny、第三者の台帳（`docs/third_party.md`） | 完了 | Windows の CI は LibRaw なし（入手方法が未定） |
| 3 | `genzo-color` | 完了（独立レビュー済み） | 純粋なガンマの変換先で、33³ の LUT の暗部（L* < 5）の ΔE2000 が 1 を超える。扱いは PoC-1 で決める |
| 4 | `genzo-raw`（LibRaw の FFI） | 完了（独立レビュー済み） | `libraw_r` を推奨（`libraw` では LibRaw の使用を 1 つずつに制限）。実機の ARW での確認は PoC-2 |
| 5 | `genzo-media` | 完了（独立レビュー済み） | Windows / macOS 固有の動作（大文字・小文字、exFAT での名前の変更）は実機で未確認 |
| 6 | `genzo-catalog` | 完了（独立レビュー済み） | スキーマの版 2。PERF-07（50 万件で 0.2 秒）は撮影日時順などで未達の見込み。PoC-6 で決める |
| 7 | `genzo-jobs` | 完了（独立レビュー済み） | 優先度の高い大きなメモリの要求が待たされうる件は、PoC で PERF-13 を計測して判断する |
| 8 | `genzo-testkit` | 完了（独立レビュー済み） | ColorChecker の値は X-Rite の原本と未照合（人の確認が必要） |
| 9 | `genzo-pipeline`（CPU 基準実装） | 完了（v1 の一部を除く。独立レビュー済み） | ステージ 2・3・5（RCD）・8・9〜17、ガイド、エンジン（A0 / A1 / B のキャッシュ・タイル・書き出し）。未実装はステージ 4・6・7・12・14、歪曲補正、HSL など。**人の判断が必要**: IQ-07b が合成画像で未達（細線・強い補正。PoC-5）／回転・切り抜きでプレビューと書き出しの位置が最大「プレビューの 0.5 画素＋フル解像度の 0.5 画素」ずれる件（設計 2.7 節）／既定でシーンの 1.0（RAW の飽和・JPEG の 255）が sRGB 8bit の 225 になる（ステージ 15 の膝。PoC-4）／RAW 以外の入力にステージ 15 をかけるか、トーンカーブの黒の持ち上げの式（2.6 節への追記）／RCD の説明の出典（`third_party.md` 3.5 節）。仮置きの値と速度は合成データでの参考値 |
| 10 | `genzo-worker` | 完了（独立レビュー済み） | Windows / macOS は clippy のみ（実機での動作、メモリの上限、共有メモリのファイルの残り方は PoC-2）。孫プロセス（ffmpeg）をまとめて終了させる仕組み（プロセスグループ・ジョブオブジェクト）と、依頼の書き込みのタイムアウトは未実装。タイムアウト・共有メモリの大きさは仮置き（PoC-2） |
| 11 | `genzo-gpu` | 一部完了（独立レビュー済み） | 段階 C（ステージ 9〜17）と A1 の簡易処理は GPU 版あり。RCD・ガイド・ルーペの範囲の GPU 版は未実装（書き出しのセンサー処理は CPU）。確認は llvmpipe だけで、M1（Metal の高速な数学で NaN の判定が消えないか）・RTX 3080（DX12）は PoC-3。ソフトウェアの GPU（WARP・llvmpipe）を既定で使う（`allow_software`）かは人の判断。CI では比較テストを飛ばす |
| 12 | `genzo-api` | 未着手 | |
| 13 | `genzo-cli` | 未着手 | |
| 14 | 全体のレビューと修正 | 進行中 | 2026-10-10: No.3〜8 の統合を確認（fmt・clippy・test を LibRaw の有無の両方で、cargo-deny、Windows / macOS 向けの cargo clippy --target）。同日、No.9〜11 の統合も同じ手順と rustdoc（警告なし）で確認 |
| 15 | PoC の記録のひな形（`docs/poc/`）と README の更新 | 未着手 | |
