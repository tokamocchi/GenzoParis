GenzoParis のライセンスの文書
========================================

このフォルダには、GenzoParis と、GenzoParis が使っている第三者のソフトウェアのライセンスの文書があります。
アプリの「設定」の「このアプリについて」のタブからも読めます。

- GenzoParis/LICENSE.txt
  GenzoParis 自体のライセンス（GNU General Public License version 3 またはそれ以降の版。GPL-3.0-or-later）の
  全文です。GenzoParis のソースコードは https://github.com/tokamocchi/GenzoParis にあります（版ごとのソースは、
  その版のタグ。例: v0.1.1）。
- third-party/rust.txt
  本体（genzo-paris）とワーカー（genzo-worker）に組み込んだ Rust のライブラリ（crate）の一覧と、そのライセンスの
  本文です。
- third-party/npm.txt
  画面に組み込んだ JavaScript のライブラリ（npm のパッケージ）の一覧と、そのライセンスの本文です。
- third-party/ai-models.txt
  AI マスク（被写体・人物の自動の選択）に使う、同梱した学習済みモデル（U²-Net の小型版・DeepLab-v3）の
  入手元・変更の表示と、そのライセンス（Apache License 2.0・MIT License）の本文です。
- third-party/onnxruntime/（Windows・macOS の配布物）
  AI マスクの推論に使う ONNX Runtime（共有ライブラリとして同梱しています）のライセンス（LICENSE.txt。
  MIT License）と、ONNX Runtime が使っている第三者のソフトウェアのライセンス（ThirdPartyNotices.txt）です。
  GenzoParis は、ONNX Runtime のテレメトリー（Microsoft への利用の情報の送信）を止めて使います。
- third-party/native.txt（Windows・macOS の配布物）
  同梱した C / C++ の共有ライブラリ（LibRaw と、LibRaw などが使うライブラリ。Windows では Visual C++ の
  ランタイムを含む）の一覧と、そのライセンスの文書です。
- third-party/vcruntime-terms.txt（Windows の配布物）
  同梱した Microsoft Visual C++ のランタイムのファイルの使用条件です。インストールのときに、この条件への
  同意を求めています。GenzoParis 自体と、そのほかのソフトウェアには適用されません。
- LibRaw/（Windows・macOS の配布物）
  RAW の展開に使う LibRaw（共有ライブラリとして同梱しています）のライセンスの全文・著作権表示と、ソースの
  入手先・差し替えの方法の案内（LibRaw/README.txt）、同梱した LibRaw をビルドしたソースのアーカイブ
  （LibRaw/source）です。

----------------------------------------
Licenses of GenzoParis

This folder contains the license documents of GenzoParis and of the third-party software it uses. They can also be
viewed in the app, in the "About this app" tab of Settings.

- GenzoParis/LICENSE.txt: the license of GenzoParis itself (GNU General Public License version 3 or any later
  version, GPL-3.0-or-later). The source code is available at https://github.com/tokamocchi/GenzoParis (the source of
  each release is under its version tag, e.g. v0.1.1).
- third-party/rust.txt: the Rust libraries (crates) compiled into genzo-paris and genzo-worker, with their licenses.
- third-party/npm.txt: the JavaScript libraries (npm packages) bundled into the user interface, with their licenses.
- third-party/ai-models.txt: the pre-trained models bundled for AI masks (U²-Net small and DeepLab-v3), with
  their sources, modification notices and licenses (Apache License 2.0, MIT License).
- third-party/onnxruntime/ (Windows and macOS packages): the license of ONNX Runtime (LICENSE.txt, MIT License),
  bundled as a shared library to run the AI mask models, and the notices of the third-party software it uses
  (ThirdPartyNotices.txt). GenzoParis runs ONNX Runtime with its telemetry turned off.
- third-party/native.txt (Windows and macOS packages): the C / C++ shared libraries bundled with GenzoParis (LibRaw
  and the libraries it uses; on Windows, also the Visual C++ runtime), with their license documents.
- third-party/vcruntime-terms.txt (Windows package): the terms of use for the bundled Microsoft Visual C++ runtime
  files, agreed to in the installer. They do not apply to GenzoParis itself or to any other software.
- LibRaw/ (Windows and macOS packages): the license, copyright notice, source code location and replacement
  instructions for LibRaw, which is bundled as a shared library to decode raw images, and the source archives used
  for the bundled build (LibRaw/source).
