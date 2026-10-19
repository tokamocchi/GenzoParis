//! 書き出し（EXP-01・EXP-04、01 のストーリー 7。04 の 2.4 節・6.1 節・6.4 節）。
//!
//! variant ごとに:
//! 1. バッチ用ワーカーで元ファイルを展開する（RAW は `DecodeRaw`、それ以外は `DecodeImage`）。
//! 2. 本体で現像する（GPU 版を既定とし、GPU の側の失敗なら CPU 版。2.4 節）。寸法の指定は
//!    「フル解像度で処理してから縮小」（genzo-pipeline の書き出し）。
//! 3. genzo-media でエンコードする（出力の色空間の ICC を埋め込み、Exif を書く。GPS は、カタログで後から付けた
//!    位置（LIB-21。genzo-catalog の `Catalog::asset_location`）があればそれ、なければ元のファイルの値。
//!    `remove_gps` なら、付けた位置も含めて GPS を書かない。SEC-03）。カタログで編集した説明のメタデータ
//!    （LIB-15。タイトル・キャプション・作成者と連絡先・著作権・撮影場所など）は XMP に書き、ASCII で表せる
//!    キャプション・作成者・著作権表示は Exif の ImageDescription・Artist・Copyright にも書く（genzo-media の
//!    `xmp_write`・`exif_write`）。写真に付けたキーワード（LIB-09）は、親のキーワードを含めて XMP の
//!    `dc:subject` に書く（キーワードごとの「書き出しに含める」の印と、設定の `remove_keywords` に従う。
//!    genzo-catalog の `Catalog::export_keywords`）。
//! 4. 安全に書き出す（6.4 節）: **原本の保護**（書き出し先が、カタログに登録されたファイル、または今回の
//!    入力と同じファイルなら中止する。genzo-media の `ProtectedFiles` で、パスとファイルの同一性で照合する）、
//!    一時ファイルに書いてから名前を変更する、衝突の扱い（連番・上書き・スキップ）。
//!
//! カタログに登録されたファイルとの照合は、カタログ全体の数十万件のファイルを開かないため、次のファイルを
//! 保護の対象に加えて行う:
//! - 書き出し先のフォルダ（パスのままのものと、リンクを解決したもの）にカタログが登録しているファイル。
//! - 上書きの設定では、書き出すファイルと **同じ名前** の、カタログのすべてのフォルダのファイル
//!   （`same_named_catalog_files`）。名前を変更して公開する書き出しが原本の内容を失わせるのは、書き出し先の
//!   ディレクトリの項目が原本の項目そのもの（同じ名前）の場合だけなので、書き出し先のフォルダがカタログに
//!   別のパス（シンボリックリンク・大文字と小文字の違いなど）で登録されていても、同一性で見分けられる。
//!   連番・スキップの設定は既存のファイルを置き換えない（genzo-media の `persist_noclobber`）。
//!
//! 6.4 節との差: 別のフォルダの原本へのハードリンク（別の名前）を書き出し先に置いた場合は、原本として
//! 拒否しない（置き換わるのはリンクの項目だけで、原本の内容は変わらない）。
//!
//! P3 のジョブとして実行し、進捗（1 件ごと）をイベントで知らせ、取り消せる（タイルの区切りで確かめる）。
//! 結果は 1 件ずつ（成功・コピー・スキップ・失敗と理由）の一覧（[`ExportReport`]）。
//!
//! **書き出しの仕上げ**（EXP-03。決定事項ログ No.166・167）: 出力シャープ・透かしがあれば、パイプラインから
//! 16bit（ディザなし）で受け取り（縮小・ステージ 17b の後）、出力シャープ（genzo-pipeline の `output_sharpen`）→
//! 透かし（ワーカーで作った RGBA を genzo-media の `watermark::composite` で重ねる。同じ大きさの透かしは 1 回の
//! 書き出しの中で使い回す）の順にかけてから、8bit の形式なら 17b と同じ三角分布のディザで 8bit にする
//! （`requantize_u16_to_u8`）。GPU 版・CPU 版のどちらで現像しても、仕上げは同じ CPU の処理。仕上げがなければ、
//! 書き出す画素の値はこれまでと同じ（ビット単位で同じ。ファイルは、PNG に解像度の pHYs を書くようになったので同じでは
//! ない）。透かしを作れない（ファイルがない・フォントが読めない）写真は、透かしなしで書き出さずに失敗にする。
//!
//! **動画**（EXP-05。決定事項ログ No.168）: 再エンコードせずに、元のファイルをそのままコピーする（[`copy_video`]）。
//! コピー取り込みと同じ手順（`copy_import::copy_verified_with`。書き出し先と同じフォルダの一時ファイルに、読みながら
//! BLAKE3 を求めて書き、更新日時を元と同じにして確定（fsync）し、元のファイルが途中で変わっていないことと、一時
//! ファイルを読み直した BLAKE3 が元と同じことを確かめてから、上書きしない名前の変更で公開する）。元のファイルは
//! 読むだけ（DATA-01）。**上書きしない**（同じ名前があれば、上書きの設定でも連番を付ける。スキップの設定なら飛ばす）。
//! 名前は写真と同じ規則（元の名前・テンプレート）で、拡張子は元のファイルのまま。写真と動画を一緒に選んだら、写真は
//! 現像して書き出し、動画はコピーして、同じ書き出し先に置く（連番は指定した順に両方を数える）。次のときは動画を
//! 書き出さない（理由を付けて飛ばす）: 設定の `videos` が `skip`、GPS を削除する設定（元のファイルのままなので、動画の
//! 位置情報を消せない）、同じ動画のファイル（仮想コピーなど）を 2 回目に。キーワード・説明のメタデータ・撮影日時の
//! 補正は動画には書かない。登録の後に内容が変わっていれば警告を付けてコピーし、全体のハッシュを記録していなければ、
//! 登録の内容のままのときだけ記録する（ORG-03）。進捗は 1 本ごと（1 本の途中の進み具合は出さない。取り消しは 1 MiB
//! ごとに確かめ、一時ファイルを消す）。失敗の分類: 元のファイルの側（開けない・読めない・途中で変わった）は
//! [`ApiError::FileAccess`]、コピー先の側（空きの不足・書けない・確定できない）は写真と同じく入出力のエラー
//! （[`ApiError::Io`]。`storage`）にする（[`video_copy_error`]）。
//!
//! **ファイル名**（EXP-02。決定事項ログ No.163）: 既定は主となるファイルの名前から拡張子を除いたもの（仮想コピーは
//! `-名前` を足す。`output_name`）。ファイル名の規則（[`RenameRule`]。取り込みのリネームと同じテンプレート）を
//! 指定すると、撮影日時（現地の時刻。分からなければファイルの更新日時を既定のタイムゾーンで読んだもの）・カメラ・
//! 元のファイル名・連番（書き出す写真と動画の指定した順。書き出さない動画は数えない）で名前を作り、仮想コピーは `-名前` を足す
//! （`export_file_name`）。書き出す前にすべての名前を決める（上書きの設定での原本の照合にも使う）。同じ名前に
//! なったときの扱いは、規則がないときと同じ（同じ書き出しの中で書いたファイルは上書きしない）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, NaiveDateTime, Utc};
use genzo_catalog::FileFacts;
use genzo_media::watermark::{WatermarkGeometry, composite, place};
use genzo_media::{
    Destination, DynRgbImage, ExifData, ProtectedFiles, RgbImage8, RgbImage16, WriteOutcome,
    resolve_destination,
};
use genzo_model::{
    AssetKind, ConflictPolicy, ExportFormat, ExportSettings, FileId, VariantId, VideoExport,
    Watermark,
};
use genzo_pipeline::ExportPixels;
use genzo_pipeline::finish::output::{Dither, requantize_u16_to_u8};
use genzo_pipeline::output_sharpen::{OutputSharpenParams, sharpen_rgb16};
use genzo_worker::{Lane, WatermarkImage, WatermarkRequest};

use crate::copy_import::{CopyFailure, copy_verified_with, remove_stale_temp_files};
use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::jobs::{JobCtx, spawn_job};
use crate::naming::{NameInput, RenameRule, TemplatePart, join_extension, render_stem};
use crate::paths::{absolute_lexical, join_rel, sanitize_file_component, split_volume};
use crate::previews::{VariantFile, load_source, variant_file};
use crate::types::{
    ExportItem, ExportOutcome, ExportReport, JobKind, JobResult, NameExample, NamePreview,
};

/// 書き出し先のフォルダにある、カタログに登録されたファイルの絶対パス（パスのままと、リンクを解決した
/// もの）。
fn catalog_files_in(inner: &Inner, dir: &Path) -> Result<Vec<PathBuf>, ApiError> {
    let mut candidates = vec![dir.to_path_buf()];
    if let Ok(c) = std::fs::canonicalize(dir) {
        candidates.push(c);
    }
    let mut out = Vec::new();
    for d in candidates {
        let Ok(abs) = absolute_lexical(&d) else {
            continue;
        };
        // OS のボリューム ID のボリュームと、以前の形式（パスから作った ID）のボリューム（まだ移していない
        // フォルダ。FILE-03）の両方で探す。
        let mut vols = Vec::new();
        if let Ok(v) = crate::volumes::locate(inner.volume_probe.as_ref(), &abs) {
            vols.push(v);
        }
        if let Ok(v) = split_volume(&abs)
            && !vols.contains(&v)
        {
            vols.push(v);
        }
        for vol in vols {
            inner.with_catalog(|c| {
                let Some(volume) = c.volumes()?.into_iter().find(|v| v.uuid == vol.uuid) else {
                    return Ok(());
                };
                let Some(folder) = c.find_folder(volume.id, &vol.rel)? else {
                    return Ok(());
                };
                for f in c.files_in_folder(folder)? {
                    out.push(join_rel(&vol.mount, &vol.rel, &f.name));
                }
                Ok(())
            })?;
        }
    }
    Ok(out)
}

/// 書き出すファイルと同じ名前の、カタログに登録されたすべてのファイルの絶対パス（上書きの設定で使う）。
///
/// 名前を変更して公開する書き出しが原本を置き換えるのは、書き出し先の **ディレクトリの項目** が原本の
/// 項目そのものの場合だけ（ハードリンク・シンボリックリンクの別の名前を置き換えても、原本の内容は
/// 変わらない）。その項目の名前は、書き出すファイルの名前と（大文字・小文字と正規化の違いを除いて）
/// 同じになる。そこで、同じ名前のカタログのファイルを [`ProtectedFiles`] に入れ、パスに加えて
/// ファイルの同一性（ボリュームとファイル ID）で照合する。書き出し先のフォルダが、カタログに別のパス
/// （シンボリックリンクを経由したフォルダ、大文字・小文字の違い、`..` を含むパスなど）で登録されて
/// いても見分けられる（6.4 節）。`names` は書き出すファイルの名前（[`plan_names`]）。
fn same_named_catalog_files(inner: &Inner, names: &[&str]) -> Result<Vec<PathBuf>, ApiError> {
    inner.with_catalog_api(|c| {
        let mut out = Vec::new();
        for f in c.files_named(names)? {
            if let Some(p) = c.file_location(f.id)?.absolute_path() {
                out.push(p);
            }
        }
        Ok(out)
    })
}

/// 書き出しのファイル名（主となるファイルの名前から拡張子を除いたもの。仮想コピーは名前を付ける）。
fn output_name(file_name: &str, variant_name: Option<&str>, ext: &str) -> String {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("export");
    let mut name = sanitize_file_component(stem);
    if let Some(v) = variant_name {
        name.push('-');
        name.push_str(&sanitize_file_component(v));
    }
    join_extension(&name, ext)
}

/// 動画のコピーの拡張子（元のファイルの拡張子。大文字・小文字もそのまま。なければ空）。
fn original_extension(file_name: &str) -> String {
    Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(sanitize_file_component)
        .unwrap_or_default()
}

/// 動画を書き出すか（コピーする設定で、GPS を削除しない。モジュールの doc の「動画」）。
fn copies_videos(settings: &ExportSettings) -> bool {
    settings.videos == VideoExport::Copy && !settings.remove_gps
}

/// 書き出しのファイル名を作る写真の情報（[`export_file_name`]）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ExportNameInput<'a> {
    /// 主となるファイルの名前（拡張子を含む）。
    pub file_name: &'a str,
    /// 仮想コピーの名前（マスターは `None`）。
    pub copy_name: Option<&'a str>,
    /// 撮影日時（現地の時刻。分からなければファイルの更新日時）。
    pub when: NaiveDateTime,
    /// カメラの機種。
    pub camera: Option<&'a str>,
    /// 連番（開始番号を足した後の値）。
    pub sequence: u64,
}

/// 書き出しのファイル名（拡張子を含む。モジュールの doc の「ファイル名」）。`naming` が `None` なら
/// [`output_name`]（今までどおり）。規則があれば、テンプレートで名前を作り、仮想コピーは `-名前` を足す。
pub(crate) fn export_file_name(
    naming: Option<(&RenameRule, &[TemplatePart])>,
    input: &ExportNameInput<'_>,
    ext: &str,
) -> String {
    let Some((rule, parts)) = naming else {
        return output_name(input.file_name, input.copy_name, ext);
    };
    let original_stem = Path::new(input.file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("export");
    let mut stem = render_stem(
        parts,
        rule,
        &NameInput {
            original_stem,
            when: input.when,
            camera: input.camera,
            sequence: input.sequence,
        },
    );
    if let Some(v) = input.copy_name {
        stem.push('-');
        stem.push_str(&sanitize_file_component(v));
    }
    join_extension(&stem, ext)
}

/// 書き出すファイルの名前を、書き出す前にすべて決める（variant → 名前。書き出さない動画・見つからない variant は
/// 含めない）。連番は `variant_ids` の順（重なりは 1 回、書き出さない動画は数えない）。撮影日時が分からない写真・動画は、
/// 主となるファイルの更新日時を既定のタイムゾーンで読む（読めなければ今の時刻）。動画の拡張子は元のファイルのまま。
fn plan_names(
    inner: &Inner,
    variant_ids: &[VariantId],
    settings: &ExportSettings,
    naming: Option<&RenameRule>,
) -> Result<HashMap<VariantId, String>, ApiError> {
    struct Row {
        variant_id: VariantId,
        file_name: String,
        copy_name: Option<String>,
        when: Option<NaiveDateTime>,
        path: Option<PathBuf>,
        camera: Option<String>,
        video: bool,
    }
    let parts = naming.map(RenameRule::parse).transpose()?;
    let needs_time = parts.is_some();
    let videos = copies_videos(settings);
    let rows: Vec<Row> = inner.with_catalog_api(|c| {
        let by_id: HashMap<VariantId, genzo_catalog::VariantSummary> = c
            .variant_summaries(variant_ids)?
            .into_iter()
            .map(|s| (s.variant_id, s))
            .collect();
        let mut seen = HashSet::new();
        // 動画は元のファイルをコピーするので、同じファイル（仮想コピー）は 1 回だけにする（連番を、実際に
        // 書き出すものにだけ振る。PR #4 の Codex の指摘）。
        let mut video_files = HashSet::new();
        let mut rows = Vec::new();
        for v in variant_ids {
            let Some(s) = by_id.get(v) else { continue };
            if (s.kind == AssetKind::Video && !videos) || !seen.insert(*v) {
                continue;
            }
            if s.kind == AssetKind::Video && !video_files.insert(s.file_id) {
                continue;
            }
            let when = s.capture.local().map(|t| t.naive_local());
            let path = if needs_time && when.is_none() {
                c.file_location(s.file_id)?.absolute_path()
            } else {
                None
            };
            rows.push(Row {
                variant_id: *v,
                file_name: s.file_name.clone(),
                copy_name: (!s.is_master).then(|| s.variant_name.clone()).flatten(),
                when,
                path,
                camera: s.camera.clone(),
                video: s.kind == AssetKind::Video,
            });
        }
        Ok(rows)
    })?;
    let offset = inner.default_offset();
    let ext = settings.format.extension();
    let start = naming.map_or(0, |r| u64::from(r.sequence_start));
    let mut out = HashMap::with_capacity(rows.len());
    for (i, r) in rows.iter().enumerate() {
        let when = r.when.unwrap_or_else(|| {
            r.path
                .as_deref()
                .and_then(|p| std::fs::metadata(p).ok())
                .and_then(|m| m.modified().ok())
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(Utc::now)
                .with_timezone(&offset)
                .naive_local()
        });
        let video_ext;
        let ext = if r.video {
            video_ext = original_extension(&r.file_name);
            video_ext.as_str()
        } else {
            ext
        };
        let name = export_file_name(
            naming.zip(parts.as_deref()),
            &ExportNameInput {
                file_name: &r.file_name,
                copy_name: r.copy_name.as_deref(),
                when,
                camera: r.camera.as_deref(),
                sequence: start + i as u64,
            },
            ext,
        );
        out.insert(r.variant_id, name);
    }
    Ok(out)
}

/// 書き出しのファイル名の見本（ファイル名・撮影日時・カメラ・仮想コピーの名前）。
const EXPORT_SAMPLES: &[(&str, &str, Option<&str>, Option<&str>)] = &[
    (
        "DSC01234.ARW",
        "2026-10-13T14:30:52",
        Some("ILCE-7M4"),
        None,
    ),
    (
        "DSC01235.ARW",
        "2026-10-13T14:30:53",
        Some("ILCE-7M4"),
        None,
    ),
    (
        "DSC01235.ARW",
        "2026-10-13T14:30:53",
        Some("ILCE-7M4"),
        Some("白黒"),
    ),
];

/// 書き出しのファイル名の例（決まった見本の写真。EXP-02 の画面・CLI の確認用。カタログは使わない）。`naming` が
/// 正しくなければ `error` に理由（例は空）。
pub fn preview_export_names(naming: Option<&RenameRule>, ext: &str) -> NamePreview {
    let parts = match naming.map(RenameRule::parse).transpose() {
        Ok(p) => p,
        Err(e) => {
            return NamePreview {
                examples: Vec::new(),
                error: Some(e.to_string()),
            };
        }
    };
    let start = naming.map_or(0, |r| u64::from(r.sequence_start));
    let examples = EXPORT_SAMPLES
        .iter()
        .enumerate()
        .map(|(i, &(file_name, when, camera, copy_name))| {
            let when = NaiveDateTime::parse_from_str(when, "%Y-%m-%dT%H:%M:%S")
                .expect("見本の日時は正しい");
            let destination = export_file_name(
                naming.zip(parts.as_deref()),
                &ExportNameInput {
                    file_name,
                    copy_name,
                    when,
                    camera,
                    sequence: start + i as u64,
                },
                ext,
            );
            NameExample {
                source: match copy_name {
                    Some(c) => format!("{file_name}（仮想コピー「{c}」）"),
                    None => file_name.to_owned(),
                },
                destination,
                captured: when.format("%Y-%m-%d %H:%M:%S").to_string(),
            }
        })
        .collect();
    NamePreview {
        examples,
        error: None,
    }
}

/// 同じ書き出しの中で書いたファイルを覚えるときのキー（書き出し先のフォルダは 1 回の書き出しで共通なので、
/// ファイル名だけで照合する）。
///
/// Windows（NTFS）・macOS（APFS）の既定のファイルシステムは、大文字・小文字と正規化（NFC / NFD）の違いを
/// 区別しないので、カタログのパスの比較キーと同じ正規化（NFC ＋ 小文字化。3.5 節）で照合する。区別する
/// ファイルシステムで誤って一致しても、連番が付くだけで害はない。
fn written_key(name: &str) -> String {
    genzo_catalog::text::path_key(name)
}

/// 衝突の扱い: 上書きの設定でも、同じ書き出しの中で書いたファイルは上書きしない（連番にする）。
fn conflict_policy_for(
    settings: &ExportSettings,
    written: &HashSet<String>,
    desired_name: &str,
) -> ConflictPolicy {
    if settings.on_conflict == ConflictPolicy::Overwrite
        && written.contains(&written_key(desired_name))
    {
        ConflictPolicy::Sequence
    } else {
        settings.on_conflict
    }
}

/// 1 回の書き出しで使い回す透かしの数の上限（写真の向き・大きさの組み合わせの数。超えたら作り直す）。
const WATERMARK_CACHE_ENTRIES: usize = 8;

/// 1 回の書き出しの状態（書き出し先・原本の保護・決めた名前・書いたファイル・透かし・コピーした動画）。
struct ExportRun<'a> {
    ctx: &'a JobCtx<'a>,
    settings: &'a ExportSettings,
    dest: PathBuf,
    protected: ProtectedFiles,
    names: HashMap<VariantId, String>,
    /// 同じ書き出しの中で書いたファイルの名前（[`written_key`]）。
    written: HashSet<String>,
    /// 作った透かし（同じ依頼は使い回す）。
    watermarks: HashMap<WatermarkRequest, Arc<WatermarkImage>>,
    /// コピーした動画のファイル（同じファイルは 1 回だけ）。
    copied_files: HashSet<FileId>,
}

/// 1 件を書き出す（写真は現像して書き出し、動画はコピーする）。
fn export_one(run: &mut ExportRun<'_>, variant_id: VariantId) -> Result<ExportOutcome, ApiError> {
    let file = run
        .ctx
        .inner
        .with_catalog_api(|c| variant_file(c, variant_id))?;
    if file.kind == AssetKind::Video {
        return copy_video(run, &file);
    }
    export_photo(run, variant_id)
}

/// 書き出しの仕上げ（EXP-03）の結果。
struct Finished {
    pixels: DynRgbImage,
    warnings: Vec<String>,
}

/// 透かしを作る（同じ依頼は使い回す）。
fn watermark_for(
    run: &mut ExportRun<'_>,
    request: WatermarkRequest,
) -> Result<Arc<WatermarkImage>, ApiError> {
    if let Some(w) = run.watermarks.get(&request) {
        return Ok(w.clone());
    }
    let pool = run.ctx.inner.pool()?;
    let image = Arc::new(pool.render_watermark(Lane::Batch, &request, run.ctx.token())?);
    if run.watermarks.len() >= WATERMARK_CACHE_ENTRIES {
        run.watermarks.clear();
    }
    run.watermarks.insert(request, image.clone());
    Ok(image)
}

/// 透かしの警告（フォントにない文字・縮めた）。
pub(crate) fn watermark_warnings(image: &WatermarkImage) -> Vec<String> {
    let mut out = Vec::new();
    if !image.missing.is_empty() {
        let chars: String = image.missing.iter().collect();
        out.push(format!(
            "透かしのフォントにない文字があります（フォントの「ない文字」の形で描きました）: {chars}"
        ));
    }
    if image.shrunk {
        out.push("透かしの文字が写真に収まらないため、小さくしました".to_owned());
    }
    out
}

/// 透かしの依頼（`width` × `height` の画像に付けるとき）と、置く位置の計算に使う大きさ。
pub(crate) fn watermark_request(
    watermark: &Watermark,
    width: u32,
    height: u32,
    space: genzo_model::OutputColorSpace,
) -> (WatermarkRequest, WatermarkGeometry) {
    let g = WatermarkGeometry::new(watermark, width, height);
    (
        WatermarkRequest {
            content: watermark.content.clone(),
            size_px: g.size_px.min(WatermarkRequest::MAX_EDGE),
            max_width: g.max_width.min(WatermarkRequest::MAX_EDGE),
            max_height: g.max_height.min(WatermarkRequest::MAX_EDGE),
            align: watermark.anchor.horizontal(),
            color_space: space,
        },
        g,
    )
}

/// 透かしを重ねる（`data` は出力の色空間に符号化した 16bit の RGB）。
pub(crate) fn apply_watermark(
    data: &mut [u16],
    width: u32,
    height: u32,
    watermark: &Watermark,
    geometry: &WatermarkGeometry,
    image: &WatermarkImage,
) -> Result<(), ApiError> {
    let mark = &image.bitmap;
    let at = place(
        watermark.anchor,
        (width, height),
        (mark.width(), mark.height()),
        geometry.margin_px,
    );
    composite(
        data,
        width,
        height,
        mark,
        at,
        f32::from(watermark.opacity) / 100.0,
    )?;
    Ok(())
}

/// 書き出しの仕上げ（EXP-03。モジュールの doc）: 16bit の画素に出力シャープ → 透かしをかけ、形式のビット数にする。
fn finish_pixels(
    run: &mut ExportRun<'_>,
    pixels: ExportPixels,
    width: u32,
    height: u32,
) -> Result<Finished, ApiError> {
    let settings = run.settings;
    let mut data = match pixels {
        ExportPixels::Rgb16(v) => v,
        // 仕上げのときはパイプラインから 16bit で受け取る。8bit が来たら広げる（念のため）。
        ExportPixels::Rgb8(v) => v.into_iter().map(|x| u16::from(x) * 257).collect(),
    };
    let mut warnings = Vec::new();
    if let Some(sharpening) = &settings.output_sharpening {
        let params = OutputSharpenParams::new(sharpening, settings.resolution_ppi);
        sharpen_rgb16(&mut data, width, height, settings.color_space, &params)?;
    }
    if run.ctx.is_cancelled() {
        return Err(ApiError::Cancelled);
    }
    if let Some(watermark) = &settings.watermark {
        let (request, geometry) = watermark_request(watermark, width, height, settings.color_space);
        let image = watermark_for(run, request)?;
        apply_watermark(&mut data, width, height, watermark, &geometry, &image)?;
        warnings.extend(watermark_warnings(&image));
    }
    let pixels = if settings.format.bits_per_channel() == 8 {
        DynRgbImage::Rgb8(RgbImage8::from_raw(
            width,
            height,
            requantize_u16_to_u8(&data, width, (0, 0), Dither::DEFAULT_8BIT),
        )?)
    } else {
        DynRgbImage::Rgb16(RgbImage16::from_raw(width, height, data)?)
    };
    Ok(Finished { pixels, warnings })
}

/// 写真を 1 枚書き出す。
fn export_photo(run: &mut ExportRun<'_>, variant_id: VariantId) -> Result<ExportOutcome, ApiError> {
    let ctx = run.ctx;
    let inner = ctx.inner;
    let settings = run.settings;
    let (file, develop, capture, descriptive, keywords, assigned_gps) =
        inner.with_catalog_api(|c| {
            let f = variant_file(c, variant_id)?;
            let d = c.develop_settings(variant_id)?;
            let t = c.capture_time(f.asset_id)?;
            let m = c.asset_metadata(f.asset_id)?;
            // 書き出すキーワード（付いているものと親のうち、書き出しに含めるもの。LIB-09）。設定で書かない
            // ときも読む（genzo-media が設定に従って除く）。
            let k = c.export_keywords(variant_id)?;
            // 後から付けた位置（LIB-21）。GPS を削除する設定のときも読む（genzo-media が設定に従って除く）。
            let g = c.asset_location(f.asset_id)?;
            Ok((f, d, t, m, k, g))
        })?;
    let loaded = load_source(inner, &file, Lane::Batch, ctx.token())?;
    // 保存された設定の外部データ（render_deps）を、実際に使う値に置き換えて描き、使った値を結果に記録する
    // （`deps` の doc）。保存された値と違えば警告する。
    let effective = crate::deps::with_render_deps(&inner.engine, &develop, &loaded.render_deps);
    let render_deps = effective.render_deps.clone();
    if crate::deps::recorded_deps_differ(&develop.render_deps, &render_deps) {
        inner.events.warn(
            crate::events::WarningCode::RenderDepsChanged,
            crate::deps::deps_changed_message(&develop.render_deps, &render_deps),
            Some(variant_id),
            None,
        );
    }
    let develop = effective;
    let token = ctx.token().clone();
    let control = move || token.is_cancelled();
    // 仕上げ（出力シャープ・透かし）があれば、16bit（ディザなし）で受け取ってから仕上げる。
    let finishing = settings.has_finishing();
    let render_settings = if finishing && settings.format.bits_per_channel() == 8 {
        ExportSettings {
            format: ExportFormat::Tiff16,
            ..settings.clone()
        }
    } else {
        settings.clone()
    };
    let (image, backend) = inner.render_export(
        &loaded.source,
        &develop,
        &render_settings,
        &control,
        variant_id,
    )?;
    let (w, h) = (image.width, image.height);
    let mut warnings = crate::render::warning_texts(&image.warnings, &image.unimplemented);
    let pixels = if finishing {
        let f = finish_pixels(run, image.pixels, w, h)?;
        warnings.extend(f.warnings);
        f.pixels
    } else {
        match image.pixels {
            ExportPixels::Rgb8(v) => DynRgbImage::Rgb8(RgbImage8::from_raw(w, h, v)?),
            ExportPixels::Rgb16(v) => DynRgbImage::Rgb16(RgbImage16::from_raw(w, h, v)?),
        }
    };
    // Exif（撮影情報と、利用者が補正した撮影日時）と、カタログで編集した説明のメタデータ（LIB-15。XMP と、
    // ASCII で表せる値は Exif にも）、キーワード（LIB-09。XMP の dc:subject）。GPS は後から付けた位置（LIB-21）が
    // あればそれで置き換え、なければ元のファイルの値のまま。GPS・キーワードは export_image が設定（remove_gps・
    // remove_keywords）に従って除く（付けた位置を入れた後に除くので、付けた位置も書かない）。
    let mut exif = ExifData::from_photo_metadata(&loaded.metadata)
        .with_capture_time(&capture)
        .with_descriptive(descriptive)
        .with_keywords(keywords);
    if let Some(g) = assigned_gps {
        exif = exif.with_gps(g);
    }
    // 名前は書き出す前に決めてある（[`plan_names`]）。決めた後に増えた写真などは、元のファイルの名前にする。
    let desired_name = match run.names.get(&variant_id) {
        Some(n) => n.clone(),
        None => output_name(
            &file.name,
            (!file.is_master)
                .then_some(file.variant_name.as_deref())
                .flatten(),
            settings.format.extension(),
        ),
    };
    let desired = run.dest.join(&desired_name);
    // 同じ書き出しの中で書いたファイルは上書きしない（同じ名前の写真が別のフォルダにある場合など）。
    let mut effective = settings.clone();
    effective.on_conflict = conflict_policy_for(settings, &run.written, &desired_name);
    match genzo_media::export_image(&pixels, &effective, Some(&exif), &desired, &run.protected)? {
        WriteOutcome::Written { path, replaced } => {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                run.written.insert(written_key(name));
            }
            crate::render::emit_render_warnings(
                inner,
                variant_id,
                &format!("書き出し（{}）", path.display()),
                &warnings,
            );
            Ok(ExportOutcome::Written {
                path,
                replaced,
                backend,
                warnings,
                render_deps: (render_deps != genzo_model::RenderDeps::default())
                    .then(|| Box::new(render_deps)),
            })
        }
        WriteOutcome::Skipped { existing } => Ok(ExportOutcome::Skipped {
            reason: "同じ名前のファイルがあるため、書き出しませんでした".to_owned(),
            existing: Some(existing),
        }),
    }
}

/// 動画を飛ばす理由（書き出す設定なら `None`。モジュールの doc の「動画」）。
pub(crate) fn video_skip_reason(settings: &ExportSettings) -> Option<&'static str> {
    if settings.videos == VideoExport::Skip {
        Some("動画を書き出さない設定です")
    } else if settings.remove_gps {
        Some(
            "位置情報（GPS）を削除する設定では、動画は書き出しません（動画は元のファイルのままコピーするため、位置情報を消せません）",
        )
    } else {
        None
    }
}

/// 同じ名前の動画をコピーし直す回数の上限（確定の前に同じ名前ができたとき）。
const VIDEO_NAME_RETRIES: u32 = 8;

/// 動画のコピーの失敗（取り消し・名前の重なりを除く）のエラー。元のファイルの側の失敗は、元のファイルにアクセスでき
/// ない（[`ApiError::FileAccess`]）、コピー先の側の失敗（空きの不足・書けない・確定できない）は、写真の書き出しと
/// 同じく入出力のエラー（[`ApiError::Io`]。分類は `storage`）にする。
fn video_copy_error(failure: CopyFailure, source: &Path, target: &Path) -> ApiError {
    let io_error = |kind: std::io::ErrorKind, m: String| ApiError::Io {
        path: target.to_path_buf(),
        source: std::io::Error::new(kind, m),
    };
    match failure {
        CopyFailure::NoSpace(m) => io_error(std::io::ErrorKind::StorageFull, m),
        CopyFailure::WriteFailed(m) => io_error(std::io::ErrorKind::Other, m),
        CopyFailure::Failed(m) => ApiError::FileAccess {
            path: source.to_path_buf(),
            message: m,
        },
        CopyFailure::Cancelled => ApiError::Cancelled,
        CopyFailure::Exists => ApiError::FileAccess {
            path: target.to_path_buf(),
            message: "書き出し先に同じ名前のファイルが次々にできるため、コピーできません"
                .to_owned(),
        },
    }
}

/// 動画を 1 本、再エンコードせずにコピーする（EXP-05。モジュールの doc の「動画」）。
fn copy_video(run: &mut ExportRun<'_>, file: &VariantFile) -> Result<ExportOutcome, ApiError> {
    let settings = run.settings;
    if let Some(reason) = video_skip_reason(settings) {
        return Ok(ExportOutcome::Skipped {
            reason: reason.to_owned(),
            existing: None,
        });
    }
    if run.copied_files.contains(&file.file_id) {
        return Ok(ExportOutcome::Skipped {
            reason: "同じ動画のファイルは 1 回だけコピーします（仮想コピーなど）".to_owned(),
            existing: None,
        });
    }
    let inner = run.ctx.inner;
    let source = file.path.clone().ok_or_else(|| ApiError::FileAccess {
        path: PathBuf::from(&file.name),
        message: "元のファイルのドライブがつながっていません".to_owned(),
    })?;
    let record = inner.with_catalog(|c| c.file(file.file_id))?;
    let facts = FileFacts::read(&source).map_err(|e| ApiError::FileAccess {
        path: source.clone(),
        message: format!("元のファイルを読めません: {e}"),
    })?;
    let mtime = std::fs::metadata(&source)
        .and_then(|m| m.modified())
        .map_err(|e| ApiError::FileAccess {
            path: source.clone(),
            message: format!("元のファイルの更新日時を読めません: {e}"),
        })?;
    let mut warnings = Vec::new();
    let unchanged = facts == record.facts;
    if !unchanged {
        warnings.push(
            "登録の後に元のファイルが変わっています（今のファイルをコピーしました。ファイルの確認でカタログを更新できます）"
                .to_owned(),
        );
    }
    let desired_name = match run.names.get(&file.variant_id) {
        Some(n) => n.clone(),
        None => output_name(
            &file.name,
            (!file.is_master)
                .then_some(file.variant_name.as_deref())
                .flatten(),
            &original_extension(&file.name),
        ),
    };
    let desired = run.dest.join(&desired_name);
    // 動画は上書きしない（スキップの設定なら飛ばし、それ以外は連番を付ける）。
    let policy = if settings.on_conflict == ConflictPolicy::Skip {
        ConflictPolicy::Skip
    } else {
        ConflictPolicy::Sequence
    };
    let mut attempt = 0;
    let (target, info) = loop {
        let target = match resolve_destination(&desired, policy)? {
            Destination::Skip { existing } => {
                return Ok(ExportOutcome::Skipped {
                    reason: "同じ名前のファイルがあるため、書き出しませんでした".to_owned(),
                    existing: Some(existing),
                });
            }
            Destination::Write { path, .. } => path,
        };
        run.protected.check(&target)?;
        // テスト専用のフック（一時ファイルに書いた後。取り消し・照合の失敗を確かめる）はコピー取り込みと同じ。
        let hook = inner.on_copy_import.as_ref();
        match copy_verified_with(hook, &source, &facts, mtime, &target, run.ctx.token()) {
            Ok(info) => break (target, info),
            Err(CopyFailure::Exists) if attempt < VIDEO_NAME_RETRIES => attempt += 1,
            Err(f) => return Err(video_copy_error(f, &source, &target)),
        }
    };
    run.copied_files.insert(file.file_id);
    if let Some(name) = target.file_name().and_then(|n| n.to_str()) {
        run.written.insert(written_key(name));
    }
    if settings.on_conflict == ConflictPolicy::Overwrite && target != desired {
        warnings.push(
            "動画は上書きしません（同じ名前のファイルがあるため、連番を付けました）".to_owned(),
        );
    }
    match &record.full_hash {
        // 登録の内容のままなのに全体のハッシュが違う（大きさ・更新日時・先頭と末尾が同じで、途中が変わった）。
        Some(h) if unchanged && *h != info.full_hash => warnings.push(
            "元のファイルの内容がカタログに記録した内容と違います（今のファイルをコピーしました）"
                .to_owned(),
        ),
        Some(_) => {}
        None if unchanged => {
            // 全体のハッシュを記録する（ORG-03。記録できなくてもコピーは済んでいる）。
            if let Err(e) = inner.with_catalog(|c| {
                c.set_full_hash_if_current(file.file_id, record.revision, &facts, &info.full_hash)
            }) {
                tracing::warn!(file = %source.display(), error = %e, "動画の全体のハッシュを記録できません");
            }
        }
        None => {}
    }
    for w in &warnings {
        inner.events.warn(
            crate::events::WarningCode::VideoCopy,
            format!("動画のコピー（{}）: {w}", target.display()),
            Some(file.variant_id),
            None,
        );
    }
    Ok(ExportOutcome::Copied {
        path: target,
        bytes: info.bytes,
        full_hash: info.full_hash,
        warnings,
    })
}

/// 書き出しの本体。
fn run_export(
    ctx: &JobCtx<'_>,
    variant_ids: &[VariantId],
    settings: &ExportSettings,
    dest_dir: &Path,
    naming: Option<&RenameRule>,
) -> Result<ExportReport, ApiError> {
    let inner = ctx.inner;
    let dest = absolute_lexical(dest_dir)?;
    std::fs::create_dir_all(&dest).map_err(|e| ApiError::io(&dest, e))?;
    // 保存待ちの現像設定を先に保存する（書き出しはカタログの設定を使う）。
    crate::develop::flush_for_export(inner)?;
    // 原本の保護（6.4 節）: 今回の入力（asset のすべてのファイル）と、書き出し先のフォルダにあるカタログの
    // ファイル。
    let mut protected = ProtectedFiles::new();
    let inputs = inner.with_catalog_api(|c| {
        let mut paths = Vec::new();
        for a in c.assets_of_variants(variant_ids)? {
            for f in c.files_of_asset(a)? {
                if let Some(p) = c.file_location(f.id)?.absolute_path() {
                    paths.push(p);
                }
            }
        }
        Ok(paths)
    })?;
    protected.extend(inputs);
    protected.extend(catalog_files_in(inner, &dest)?);
    // 書き出すファイルの名前を先に決める（ファイル名の規則の連番・上書きの設定の照合）。
    let names = plan_names(inner, variant_ids, settings, naming)?;
    // 上書きの設定では、書き出すファイルと同じ名前の、カタログのすべてのファイルも保護の対象にする
    // （書き出し先のフォルダが、カタログに別のパスで登録されたフォルダでも、同一性で照合するため）。
    if settings.on_conflict == ConflictPolicy::Overwrite {
        let refs: Vec<&str> = names.values().map(String::as_str).collect();
        protected.extend(same_named_catalog_files(inner, &refs)?);
    }
    // 前の動画のコピーが残した古い一時ファイル（強制終了したときなど）を片付ける（コピー取り込みと同じ規則）。
    if copies_videos(settings) {
        remove_stale_temp_files(&dest);
    }

    let mut report = ExportReport {
        dest_dir: dest.clone(),
        ..Default::default()
    };
    let mut run = ExportRun {
        ctx,
        settings,
        dest,
        protected,
        names,
        written: HashSet::new(),
        watermarks: HashMap::new(),
        copied_files: HashSet::new(),
    };
    let total = variant_ids.len() as u64;
    ctx.progress(0, total);
    for (i, &v) in variant_ids.iter().enumerate() {
        if ctx.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let outcome = match export_one(&mut run, v) {
            Ok(o) => o,
            Err(e) if e.is_cancelled() => {
                report.cancelled = true;
                break;
            }
            Err(e) => ExportOutcome::Failed { error: e.info() },
        };
        match &outcome {
            ExportOutcome::Written { .. } => report.written += 1,
            ExportOutcome::Copied { .. } => report.copied += 1,
            ExportOutcome::Skipped { .. } => report.skipped += 1,
            ExportOutcome::Failed { .. } => report.failed += 1,
        }
        report.items.push(ExportItem {
            variant_id: v,
            outcome,
        });
        ctx.progress(i as u64 + 1, total);
    }
    Ok(report)
}

impl Core {
    /// 書き出す（EXP-01・EXP-03・EXP-04・EXP-05。動画は元のファイルのままコピーする）。P3 のジョブとして実行し、ジョブの
    /// ID を返す。ファイル名は元のファイルの名前
    /// （[`Core::export_with`] で規則を指定できる）。
    ///
    /// 結果（[`ExportReport`]）は [`Core::wait_job`]・[`Core::job`] で取れる。
    pub fn export(
        &self,
        variant_ids: &[VariantId],
        settings: &ExportSettings,
        dest_dir: &Path,
    ) -> Result<u64, ApiError> {
        self.export_with(variant_ids, settings, dest_dir, None)
    }

    /// [`Core::export`] と同じで、ファイル名の規則（EXP-02。モジュールの doc の「ファイル名」）を指定する
    /// （`None` なら元のファイルの名前）。規則が正しくなければ、ジョブを投入せずに [`ApiError::InvalidArgument`]。
    pub fn export_with(
        &self,
        variant_ids: &[VariantId],
        settings: &ExportSettings,
        dest_dir: &Path,
        file_naming: Option<&RenameRule>,
    ) -> Result<u64, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        settings
            .validate()
            .map_err(|e| ApiError::InvalidArgument(e.to_string()))?;
        if let Some(rule) = file_naming {
            rule.parse()?;
        }
        let ids = variant_ids.to_vec();
        let settings = settings.clone();
        let dest = dest_dir.to_path_buf();
        let naming = file_naming.cloned();
        Ok(spawn_job(
            inner,
            JobKind::Export,
            "書き出し",
            move |ctx| {
                run_export(ctx, &ids, &settings, &dest, naming.as_ref()).map(JobResult::Export)
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同じ書き出しの中で書いたファイルの照合は、大文字・小文字と正規化（NFC / NFD）の違いを区別しない
    /// （Windows / macOS の既定のファイルシステムでは同じファイルになり、先の出力が置き換えられるため。
    /// 指摘 F17）。
    #[test]
    fn files_written_in_the_same_export_are_matched_like_the_file_system() {
        let settings = ExportSettings {
            on_conflict: ConflictPolicy::Overwrite,
            ..ExportSettings::default()
        };
        let mut written = HashSet::new();
        written.insert(written_key("IMG_0001.jpg"));
        written.insert(written_key("\u{30AB}\u{3099}.jpg")); // NFD の「ガ」
        assert_eq!(
            conflict_policy_for(&settings, &written, "img_0001.jpg"),
            ConflictPolicy::Sequence
        );
        assert_eq!(
            conflict_policy_for(&settings, &written, "\u{30AC}.JPG"), // NFC の「ガ」
            ConflictPolicy::Sequence
        );
        assert_eq!(
            conflict_policy_for(&settings, &written, "IMG_0002.jpg"),
            ConflictPolicy::Overwrite
        );
        let skip = ExportSettings {
            on_conflict: ConflictPolicy::Skip,
            ..settings
        };
        assert_eq!(
            conflict_policy_for(&skip, &written, "img_0001.jpg"),
            ConflictPolicy::Skip
        );
    }

    /// ファイル名の規則（EXP-02）: テンプレートで名前を作り、仮想コピーは名前を足す。規則がなければ今までどおり。
    #[test]
    fn export_file_names_follow_the_rule() {
        let when =
            NaiveDateTime::parse_from_str("2026-10-13 14:30:52", "%Y-%m-%d %H:%M:%S").unwrap();
        let input = |copy: Option<&'static str>, seq: u64| ExportNameInput {
            file_name: "DSC01234.ARW",
            copy_name: copy,
            when,
            camera: Some("ILCE-7M4"),
            sequence: seq,
        };
        assert_eq!(
            export_file_name(None, &input(None, 1), "jpg"),
            "DSC01234.jpg"
        );
        assert_eq!(
            export_file_name(None, &input(Some("白黒"), 1), "jpg"),
            "DSC01234-白黒.jpg"
        );
        let rule = RenameRule {
            template: "{date}_{camera}_{seq}".to_owned(),
            sequence_digits: 3,
            ..RenameRule::default()
        };
        let parts = rule.parse().unwrap();
        let named = |copy, seq| export_file_name(Some((&rule, &parts)), &input(copy, seq), "tif");
        assert_eq!(named(None, 7), "20261013_ILCE-7M4_007.tif");
        assert_eq!(named(Some("x/y"), 8), "20261013_ILCE-7M4_008-x_y.tif");
        // 例: 見本の写真に連番を付ける。
        let preview = preview_export_names(Some(&rule), "jpg");
        assert_eq!(preview.error, None);
        let names: Vec<&str> = preview
            .examples
            .iter()
            .map(|e| e.destination.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "20261013_ILCE-7M4_001.jpg",
                "20261013_ILCE-7M4_002.jpg",
                "20261013_ILCE-7M4_003-白黒.jpg"
            ]
        );
        assert_eq!(
            preview_export_names(None, "png").examples[2].destination,
            "DSC01235-白黒.png"
        );
        let bad = RenameRule {
            template: "{unknown}".to_owned(),
            ..RenameRule::default()
        };
        let p = preview_export_names(Some(&bad), "jpg");
        assert!(p.examples.is_empty());
        assert!(p.error.unwrap().contains("unknown"));
    }

    /// 動画のコピーの失敗の分類: コピー先の空きの不足・書き込みの失敗は、写真の書き出しと同じ `storage`（もう一度
    /// 試せる）で、元のファイルにアクセスできないとは知らせない（検証のラウンド 1 の指摘。直す前は空きの不足も
    /// `file_access`（元の動画のパス）だった）。
    #[test]
    fn video_copy_failures_are_classified_by_where_they_happen() {
        use crate::error::ErrorKind;
        let (src, dst) = (Path::new("/photos/V.mp4"), Path::new("/tiny/V.mp4"));
        let full = video_copy_error(
            CopyFailure::NoSpace(
                "コピー先に書けません（/tiny/V.mp4）: No space left on device (os error 28)"
                    .to_owned(),
            ),
            src,
            dst,
        );
        let info = full.info();
        assert_eq!(info.kind, ErrorKind::Storage);
        assert!(info.retryable && info.user_actionable);
        assert!(info.message.contains("/tiny/V.mp4"), "{}", info.message);
        assert!(!info.message.contains("/photos/V.mp4"), "{}", info.message);
        assert!(
            matches!(&full, ApiError::Io { source, .. } if source.kind() == std::io::ErrorKind::StorageFull)
        );
        let write = video_copy_error(
            CopyFailure::WriteFailed(
                "一時ファイルを作れません（/tiny）: Permission denied".to_owned(),
            ),
            src,
            dst,
        );
        assert_eq!(write.kind(), ErrorKind::Storage);
        // 元のファイルの側の失敗は、これまでどおり元のファイルのパスで知らせる。
        let read = video_copy_error(
            CopyFailure::Failed("元のファイルを読めません（/photos/V.mp4）: I/O error".to_owned()),
            src,
            dst,
        );
        assert!(matches!(&read, ApiError::FileAccess { path, .. } if path == src));
        assert_eq!(read.kind(), ErrorKind::FileAccess);
        assert!(video_copy_error(CopyFailure::Cancelled, src, dst).is_cancelled());
    }

    #[test]
    fn output_names() {
        assert_eq!(output_name("DSC00001.ARW", None, "jpg"), "DSC00001.jpg");
        assert_eq!(
            output_name("DSC00001.ARW", Some("白黒"), "tif"),
            "DSC00001-白黒.tif"
        );
        assert_eq!(
            output_name("a.b.c.png", Some("x/y"), "png"),
            "a.b.c-x_y.png"
        );
    }
}
