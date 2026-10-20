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
- third-party/native.txt（Windows・macOS の配布物）
  同梱した C / C++ の共有ライブラリ（LibRaw と、LibRaw などが使うライブラリ。Windows では Visual C++ の
  ランタイムを含む）の一覧と、そのライセンスの文書です。
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
- third-party/native.txt (Windows and macOS packages): the C / C++ shared libraries bundled with GenzoParis (LibRaw
  and the libraries it uses; on Windows, also the Visual C++ runtime), with their license documents.
- LibRaw/ (Windows and macOS packages): the license, copyright notice, source code location and replacement
  instructions for LibRaw, which is bundled as a shared library to decode raw images, and the source archives used
  for the bundled build (LibRaw/source).
