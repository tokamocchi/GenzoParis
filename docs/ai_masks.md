# AI マスク（DEV-20）: 仕組み・モデルの置き方・確認の手順

被写体・空・人物の自動の選択（AI マスク。[01](01_feature_inventory.md) の DEV-20）の実装（2026-10-26）の説明と、人が
行うこと（モデルの選定と置き方、ONNX Runtime の同梱、実機での確認）の手順です。設計は [04](04_architecture.md) の 2.1 節
「ローカル補正」と 7.2 節「AI 機能を差し込む位置」、決定事項は [decision_log.md](decision_log.md) の No.200〜203、
人の判断が要る点は [implementation_status.md](implementation_status.md) の判断の表 No.202〜205 にあります。

**このリポジトリと配布物には、モデルのファイルも ONNX Runtime も含めていません。** どちらも人が選んで置くまで、
AI マスクのボタンは押せず（理由を説明に出す）、CLI は「使えません」と知らせます。

## 1. 仕組み

| 段階 | 場所 | 内容 |
|---|---|---|
| 推論の抽象 | `genzo-ml`（`MlBackend`・`MlSession`・`Tensor`） | 04 の 7.2 節のトレイト。モデルのファイルからセッションを作り、f32 のテンソルを入れて出す |
| ONNX Runtime | `genzo-ml` の `OnnxBackend`（機能 `onnx`。既定で有効） | pyke の `ort` 2.0.0-rc.15。共有ライブラリは**ビルド時に取らず**（既定の機能 `download-binaries` を外した）、実行時に指定したファイルから読み込む（`load-dynamic`）。実行プロバイダーは CPU だけ（CUDA・DirectML・Core ML は未対応。No.201） |
| モデルの台帳 | `genzo-ml` の `ModelManifest`（`*.genzo-model.json`） | モデルごとの前処理・後処理・種類との対応・出どころとライセンス（2 章）。アプリのコードはモデルの中身を知らない |
| 推論の入口 | `genzo-ml` の `Segmenter`（`ModelLibrary` が実装） | 画像 → 種類の重みのラスター（8 ビット）。モデルのフォルダの台帳を読み、種類ごとに最初に使える台帳を選び、セッションを使い回す |
| コア API | `genzo-api` の `Core::compute_ai_mask`（コマンド `compute_ai_mask`）・`ai_mask_status`・`collect_ai_masks` | 切り抜く前の全体（G の外接矩形）を今の設定で sRGB の 8 ビットに描き（長辺 1024 まで）、推論し、ラスターを PNG にしてカタログに保存し、設定に入れる形（`MaskShape::Ai`）を返す。設定への適用は画面・CLI（色域のスポイトと同じ） |
| 保存 | `genzo-catalog` の `ai_mask`（スキーマの版 14。[catalog_schema.md](catalog_schema.md)） | 内容の SHA-256 をキーにしたラスター。asset を消すと消える。参照のない行は `genzo ai collect` で回収 |
| 設定 | `genzo-model` の `MaskShape::Ai { kind, model, mask: DataRef, width, height, bounds }` | ラスターの参照（ID `ai-mask:<種類>` と SHA-256）と大きさ、覆う範囲の G 座標。ラスター自体は設定に入れない（履歴の 1 件ごとに設定の全体を写すため） |
| 描画 | `genzo-pipeline` の `ProfileResolver::ai_mask` → `ProfileResources::ai_masks` → `local::ai`（ブラシと同じ G 座標の格子 `BrushRaster` に写す） | 描画のたびに推論し直さない。GPU 版はブラシと同じ経路（`local.wgsl` の種類 2）で、シェーダーは変えていない。ラスターが見つからなければ重み 0 で描き、警告する（`SensorWarning::AiMaskMissing`） |
| 画面 | 現像のパネル「マスク」の「＋ AI 被写体 / 空 / 人物」（マスク・部品の両方）と、AI の部品の「作り直す」 | 使える種類だけ押せる。計算の間は「計算しています…」。失敗はトースト |
| CLI | `genzo develop mask add <ID> --ai subject|sky|person`・`add-part … --ai`・`genzo ai status`・`genzo ai collect` | `--models-dir`・`--onnxruntime`（全体のオプション）、環境変数 `GENZO_MODELS_DIR`・`GENZO_ONNXRUNTIME` |

座標: ラスターは作ったときの G 座標（回転・歪曲補正の後、切り抜く前）の全体を覆います。ブラシと同じく、切り抜きを変えても
位置は変わりませんが、回転や歪曲補正を変えると画像に対してずれます（部品の「作り直す」で計算し直す）。

## 2. モデルの置き方（人が行う）

### 2.1 フォルダ

モデルのフォルダは、次の順で決まります（アプリと CLI で同じ）。

1. `CoreConfig::models_dir`（CLI の `--models-dir`）
2. 環境変数 `GENZO_MODELS_DIR`
3. OS のアプリのデータのフォルダの `models`（プリセットの `presets` と同じ場所。Windows は `%APPDATA%\<識別子>\models`、
   macOS は `~/Library/Application Support/<識別子>/models`。`genzo ai status` で表示）

ONNX Runtime の共有ライブラリ（Windows `onnxruntime.dll`、macOS `libonnxruntime.dylib`、Linux `libonnxruntime.so`）は、次の順で
探します（最初に見つかったもの）。

1. `CoreConfig::onnxruntime`（CLI の `--onnxruntime`）
2. 環境変数 `GENZO_ONNXRUNTIME`
3. モデルのフォルダ
4. 実行ファイルと同じフォルダ、その `lib`（配布物に同梱するとき）

ONNX Runtime はプロセスで 1 度しか読み込めません（`ort` の制約。別の場所を指定しても最初のものを使う）。版は `ort`
2.0.0-rc.15 が使う API の版（`ORT_API_VERSION` 31 → ONNX Runtime 1.31 以上。**AI の理解。要確認**）に対応したものが要ります。
この開発環境では、PyPI の `onnxruntime` 1.31.0 に含まれる `libonnxruntime.so.1.31.0` で結合テストを通しました（3 章）。

### 2.2 台帳（`<名前>.genzo-model.json`）

モデルのファイルと同じフォルダに、台帳の JSON を置きます（`genzo-ml` の `ModelManifest`）。

```json
{
  "id": "u2netp-2020",
  "name": "U²-Net (small)",
  "file": "u2netp.onnx",
  "sha256": "<ファイルの SHA-256。省略なら確かめない>",
  "license": "Apache-2.0",
  "source": "https://…（入手先）",
  "input": {
    "name": "input",
    "width": 320, "height": 320,
    "layout": "nchw",
    "range": "unit",
    "mean": [0.485, 0.456, 0.406],
    "std": [0.229, 0.224, 0.225],
    "bgr": false
  },
  "output": { "name": "output", "activation": "sigmoid", "layout": "nchw" },
  "kinds": { "subject": { "channel": 0 } },
  "threads": null
}
```

| 項目 | 意味 |
|---|---|
| `id` | 現像設定に記録する ID（1〜100 文字。空白・`/`・`\` なし）。モデルを差し替えたら ID も変える |
| `file` | モデルのファイル（台帳と同じフォルダからの相対パス。`..` 不可） |
| `sha256` | ファイルの SHA-256（小文字 64 文字）。書いてあれば読み込みの前に確かめる（違えば使わない） |
| `input.width`・`height` | モデルの固定の入力の大きさ（画像はこの大きさに縮める。縦横比は保たない） |
| `input.layout` | `nchw`（PyTorch 由来。既定）か `nhwc`（TensorFlow 由来） |
| `input.range` | 画素の値の範囲（`unit`: 0〜1、`byte`: 0〜255、`signed`: −1〜1）。その後に `mean` を引き `std` で割る（1 つか 3 つ。省略なら 0・1） |
| `input.bgr` | チャンネルの順を BGR にする |
| `output.activation` | `none`（すでに確率）・`sigmoid`・`softmax`（チャンネルの軸） |
| `output.layout` | 出力の並び（`[1, C, H, W]` か `[1, H, W, C]`。`[1, H, W]`・`[H, W]` はチャンネル 1） |
| `kinds` | 種類（`subject`・`sky`・`person`）ごとの読み方。`{"channel": c}`（1 チャンネルの値）か `{"classes": [...]}`（クラスの確率の和。多クラスのモデル） |
| `threads` | 1 回の推論に使うスレッドの数（省略なら ONNX Runtime の既定） |

台帳が読めない・ファイルがない・SHA-256 が違う台帳は使わず、`genzo ai status` と画面のボタンの説明に理由を出します。
同じ種類を扱える台帳が複数あれば、ファイル名の順で最初のものを使います。

### 2.3 モデルの候補（**人の判断。AI の理解で、ライセンスと入手先は未確認**）

この実装はモデルの中身に依存しないので、台帳で前処理・後処理を書ける ONNX のモデルなら使えます。候補として調べたものを
挙げますが、**ライセンス（商用・配布の可否）、入手先の信頼性、ONNX への変換の方法と精度、推論の時間は人が確かめて**
ください（判断の表 No.202）。

| 種類 | 候補 | AI の理解（要確認） |
|---|---|---|
| 被写体 | U²-Net（`u2net`・小さい `u2netp`） | 顕著な被写体の検出。Apache-2.0 とされる。入力 320 × 320、ImageNet の平均・標準偏差、出力はシグモイド済みの 1 チャンネル（変換の仕方による） |
| 人物 | U²-Net の人物用（`u2net_human_seg`） | 同じリポジトリの、人物の切り抜き用の重み |
| 空・人物 | ADE20K（150 クラス）で学習したセマンティックセグメンテーションのモデル（SegFormer・DeepLab 系など） | 多クラスの出力（`softmax` と `classes`）。ADE20K のクラスの番号は 0 始まりで空が 2、人物が 12 とされる。**学習済みの重みのライセンスは非商用のものが多い**（SegFormer の重みなど）ので、特に確認が要る |

モデルのファイルは Git に入れません（大きく、ライセンスの表示が要るため）。配布物に同梱する場合は、[third_party.md](third_party.md)
の 2 章にモデルと ONNX Runtime の行を足し、配布物のライセンスの文書（1.4 節。`native.txt` と同じ扱い）に載せます。

### 2.4 配布物への同梱（**人の判断**）

今の配布のビルド（`release.yml`・`scripts/lib/build.mjs`）は ONNX Runtime とモデルを同梱しません。同梱するなら、

- ONNX Runtime（MIT）の共有ライブラリを実行ファイルと同じフォルダ（または `lib`）に置き、`native.txt` にライセンスの本文を
  足す（LibRaw の同梱と同じ方式。[packaging.md](packaging.md)）。
- モデルは OS のアプリのデータのフォルダの `models` に入れるインストーラーの処理と、ライセンスの表示が要る。

同梱しない間は、利用者が 2.1 節の場所に置きます（判断の表 No.203）。

## 3. 確認の手順

### 3.1 開発環境（ONNX Runtime とモデルなし）

```sh
cargo test -p genzo-model -p genzo-pipeline -p genzo-catalog -p genzo-ml -p genzo-api --test ai_masks
cargo test -p genzo-cli --test cli ai_masks           # 使えないときの状態とエラー
cd ui && npm test -- mask                              # 画面の流れ（モックの推論）と描画
```

コア API・画面のテストは、`genzo-ml` のテスト用の推論（`FixedSegmenter`。右半分を選ぶ）を `CoreHooks::segmenter` で差し込み、
計算 → 保存 → 設定 → 描画（右半分だけ変わる）→ 履歴・スナップショット・仮想コピーの参照 → 回収 → 消したラスターを指す
設定の警告、を確かめます。

### 3.2 ONNX Runtime の結合テスト

環境変数 `GENZO_ORT_DYLIB` で共有ライブラリを指定したときだけ実行します（なければスキップして知らせる）。モデルは
`crates/genzo-ml/tests/fixtures/mean_threshold.onnx`（`make_mean_threshold.py` で作った、チャンネルの平均のしきい値を
返すだけの小さなもの。学習したモデルではない）。

```sh
GENZO_ORT_DYLIB=/path/to/libonnxruntime.so cargo test -p genzo-ml --test onnx
GENZO_ORT_DYLIB=/path/to/libonnxruntime.so cargo test -p genzo-cli --test cli ai_masks_are_computed_with_onnx_runtime
```

2026-10-26 にこの開発環境（Linux）で、PyPI の `onnxruntime` 1.31.0 の `libonnxruntime.so.1.31.0` を指定して両方通りました
（CLI は `develop mask add --ai sky` → `list`・`overlay`・書き出しまで）。Windows・macOS では未確認です。

### 3.3 実機（人が行う）

1. 2 章のとおり ONNX Runtime とモデル（台帳付き）を置き、`genzo ai status` で「使えます」と種類が出ることを確かめる。
2. アプリで写真を開き、「マスク」の「＋ AI 空」などを押す。数秒で右の一覧にマスクが足され、O キーの重ねて表示で選択範囲が
   見えること。
3. 露光量などを動かして、範囲の中だけが変わること。切り抜きを変えても位置が変わらないこと。回転を変えると範囲がずれ、
   「作り直す」で直ること。
4. 推論の時間（長辺 1024 の入力。CPU）を計測して [implementation_status.md](implementation_status.md) に記録する（PERF-01 の
   対話の応答の目安に対して長ければ、ジョブ化（進捗の表示）を検討する。判断の表 No.204）。
5. `genzo ai collect` で参照のないラスターが消え、消したラスターを指す設定（別のカタログから貼り付けたなど）が警告付きで
   描けること。

## 4. 制限（2026-10-26）

- 推論は CPU だけ。GPU の実行プロバイダー（CUDA・DirectML・Core ML）は機能フラグを足していない（決定事項ログ No.201）。
- 推論は同期（コマンドの応答まで待つ。画面は計算中の表示だけ）。長いモデルではジョブ化が要る（判断の表 No.204）。
- ラスターの長辺は 1024 まで（`DEFAULT_INPUT_LONG_EDGE`）。モデルの出力（320〜512 が多い）を双線形で広げるので、細部の
  境界はモデルの解像度で決まる。
- 設定を別の写真に貼り付けると、元の写真のラスターをそのまま指す（別の写真の選択範囲にはならない。部品の「作り直す」で
  計算し直す）。プリセットの既定ではマスクを写さない。
- カタログのバックアップにはラスターが含まれる。thumbs.db（作り直せるキャッシュ）には置かない。
