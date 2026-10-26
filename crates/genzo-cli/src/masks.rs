//! `develop mask` のサブコマンド（ローカル補正。DEV-18・DEV-19。04 の 2.1 節「ローカル補正」）。
//!
//! - マスクと部品は **1 から数えた番号** で指定する（アプリの「マスク 1」などの表示と同じ。設定の JSON の `masks`
//!   の位置は 0 から）。位置・半径・ブラシの大きさは G 座標（ジオメトリ補正後・切り抜く前の全体の外接矩形の長辺を
//!   1 とする。左上が 0,0）。
//! - `list` は写真を開かずに設定を読む。`add`・`add-part`・`remove-part`・`set`・`remove` は `develop set` と同じく
//!   写真を開かずに保存する（履歴に 1 件。名前は genzo-api の `describe_change`。例:「マスクを追加」「マスク「空」の
//!   露光量 -0.50」）。値の範囲・数の上限は genzo-model の検証で確かめる。
//! - `add --color-at`・`add-part --color-at`・`pick-color`・`overlay` は写真を開いて（展開して）、プレビューと同じ
//!   格子で色・重みを求める（genzo-api の `mask_color_at`・`mask_overlay`）。
//! - 処理バージョン 1 の写真ではマスクは使われないので、変えたときに警告する。
//! - JSON の出力: `list`・変更は `{"variant_id", "changed", "masks": [{"number", "display_name", ...マスクの JSON}]}`。
//!   `pick-color` は `{"variant_id", "x", "y", "lab"}`、`overlay` は `{"path", "overlay"}`。

use genzo_api::{DevelopSettings, VariantId};
use genzo_model::{
    AiMaskKind, BrushStroke, ColorSample, LocalAdjustment, LocalDelta, MaskComponent, MaskMode,
    MaskShape,
};
use serde_json::{Value, json};

use crate::Status;
use crate::args::{GlobalArgs, MaskCommand, MaskPartArgs, SwitchArg};
use crate::develop::{read_json_input, save, write_new_file};
use crate::error::{CliError, CliResult};
use crate::library::ensure_variants;
use crate::output::{Output, Table, json_value};
use crate::session::{OpenOptions, Session};

/// `develop mask` を実行する。
pub fn run(g: &GlobalArgs, out: Output, cmd: MaskCommand) -> CliResult<Status> {
    match cmd {
        MaskCommand::List { variant } => list(g, out, variant),
        MaskCommand::Add {
            variant,
            part,
            name,
            delta,
        } => change(g, out, variant, |s, settings| {
            let component = build_component(s, variant, &part)?;
            let mut m = LocalAdjustment::new(component);
            if let Some(name) = name {
                m.name = name;
            }
            delta.apply(&mut m.delta);
            settings.masks.push(m);
            Ok(())
        }),
        MaskCommand::AddPart {
            variant,
            mask,
            part,
        } => change(g, out, variant, |s, settings| {
            let i = index_of(settings, mask)?;
            let component = build_component(s, variant, &part)?;
            settings.masks[i].components.push(component);
            Ok(())
        }),
        MaskCommand::RemovePart {
            variant,
            mask,
            part,
        } => change(g, out, variant, |_, settings| {
            let i = index_of(settings, mask)?;
            let m = &mut settings.masks[i];
            if part > m.components.len() {
                return Err(CliError::input(format!(
                    "マスク {mask} に部品 {part} はありません（部品は {} 個）",
                    m.components.len()
                )));
            }
            m.components.remove(part - 1);
            Ok(())
        }),
        MaskCommand::Set {
            variant,
            mask,
            name,
            enabled,
            reset_adjustments,
            delta,
        } => change(g, out, variant, |_, settings| {
            let i = index_of(settings, mask)?;
            let m = &mut settings.masks[i];
            if let Some(name) = name {
                m.name = name;
            }
            if let Some(e) = enabled {
                m.enabled = e == SwitchArg::On;
            }
            if reset_adjustments {
                m.delta = LocalDelta::default();
            }
            delta.apply(&mut m.delta);
            Ok(())
        }),
        MaskCommand::Remove { variant, mask } => change(g, out, variant, |_, settings| {
            let i = index_of(settings, mask)?;
            settings.masks.remove(i);
            Ok(())
        }),
        MaskCommand::Overlay {
            variant,
            mask,
            out: path,
            uncropped,
        } => overlay(g, out, variant, mask, &path, uncropped),
        MaskCommand::PickColor { variant, x, y } => pick_color(g, out, variant, x, y),
    }
}

/// マスクの番号（1 から）を設定の位置にする（範囲の外は入力の誤り）。
fn index_of(settings: &DevelopSettings, number: usize) -> CliResult<usize> {
    let n = settings.masks.len();
    if number == 0 || number > n {
        return Err(CliError::input(if n == 0 {
            format!(
                "マスク {number} はありません（マスクがありません。genzo develop mask add で足せます）"
            )
        } else {
            format!("マスク {number} はありません（マスクは {n} 個）")
        }));
    }
    Ok(number - 1)
}

/// 部品を作る（形の指定から。色域は写真を開いて点の色を求める）。
fn build_component(s: &Session, v: VariantId, part: &MaskPartArgs) -> CliResult<MaskComponent> {
    let a = &part.shape;
    let shape = if let Some([x0, y0, x1, y1]) = a.linear {
        MaskShape::Linear { x0, y0, x1, y1 }
    } else if let Some([cx, cy, rx, ry, angle_deg, feather]) = a.radial {
        MaskShape::Radial {
            cx,
            cy,
            rx,
            ry,
            angle_deg,
            feather,
        }
    } else if let Some(src) = &a.brush {
        MaskShape::Brush {
            strokes: read_strokes(src)?,
        }
    } else if let Some([min, max, smoothness]) = a.luminance {
        MaskShape::LuminanceRange {
            min,
            max,
            smoothness,
        }
    } else if !a.color_at.is_empty() {
        MaskShape::ColorRange {
            samples: pick_samples(s, v, &a.color_at)?,
            amount: part.color_amount.unwrap_or(50.0),
        }
    } else if let Some(kind) = a.ai {
        compute_ai_shape(s, v, kind.kind())?
    } else {
        return Err(CliError::Usage(
            "形（--linear・--radial・--brush・--color-at・--luminance・--ai のどれか）を指定してください"
                .to_owned(),
        ));
    };
    if part.color_amount.is_some() && !matches!(shape, MaskShape::ColorRange { .. }) {
        return Err(CliError::Usage(
            "--color-amount は --color-at（色域の範囲）と一緒に指定してください".to_owned(),
        ));
    }
    let mode = part.mode.map(|m| m.mode()).unwrap_or(if shape.is_range() {
        MaskMode::Intersect
    } else {
        MaskMode::Add
    });
    Ok(MaskComponent {
        mode,
        invert: part.invert,
        shape,
    })
}

/// ブラシのストロークの JSON を読む（配列か 1 つのストローク。`{"strokes": [...]}` も読む）。
fn read_strokes(src: &str) -> CliResult<Vec<BrushStroke>> {
    let value = read_json_input(src)?;
    let list = match value {
        Value::Array(_) => value,
        Value::Object(ref o) if o.contains_key("strokes") => o["strokes"].clone(),
        Value::Object(_) => Value::Array(vec![value]),
        _ => {
            return Err(CliError::input(
                "ブラシの JSON は、ストロークの配列か 1 つのストロークのオブジェクトにしてください",
            ));
        }
    };
    let strokes: Vec<BrushStroke> = serde_json::from_value(list)
        .map_err(|e| CliError::input(format!("ブラシのストロークを読めません: {e}")))?;
    if strokes.is_empty() {
        return Err(CliError::input("ブラシのストロークがありません"));
    }
    Ok(strokes)
}

/// 写真を開いて、AI マスク（DEV-20）の種類 `kind` の選択範囲を推論して保存し、形を返す（閉じてから返す）。
fn compute_ai_shape(s: &Session, v: VariantId, kind: AiMaskKind) -> CliResult<MaskShape> {
    ensure_variants(s, &[v])?;
    s.core.open_develop(v)?;
    let r = s.core.compute_ai_mask(v, kind);
    s.core.close_develop()?;
    let r = r?;
    s.out.progress(format!(
        "AI マスク（{}）: モデル {}、{} × {}、描画 {} ms、推論 {} ms",
        kind.display_name(),
        r.model,
        r.width,
        r.height,
        r.render_ms,
        r.inference_ms
    ));
    Ok(r.shape)
}

/// 写真を開いて、プレビューの点の色を見本にする（閉じてから返す）。
fn pick_samples(s: &Session, v: VariantId, points: &[[f64; 2]]) -> CliResult<Vec<ColorSample>> {
    if points.len() > genzo_model::MAX_COLOR_SAMPLES {
        return Err(CliError::Usage(format!(
            "--color-at は {} つまでです",
            genzo_model::MAX_COLOR_SAMPLES
        )));
    }
    ensure_variants(s, &[v])?;
    s.core.open_develop(v)?;
    let picked = points
        .iter()
        .map(|&[x, y]| s.core.mask_color_at(v, x, y, Some(false)))
        .collect::<Result<Vec<_>, _>>();
    s.core.close_develop()?;
    Ok(picked?
        .into_iter()
        .map(|p| ColorSample {
            x: p.x,
            y: p.y,
            lab: p.lab,
        })
        .collect())
}

/// マスクの JSON（番号と表示の名前を足す）。
fn masks_json(settings: &DevelopSettings) -> Vec<Value> {
    settings
        .masks
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut v = json_value(m);
            if let Value::Object(o) = &mut v {
                o.insert("number".to_owned(), json!(i + 1));
                o.insert(
                    "display_name".to_owned(),
                    json!(genzo_api::mask_display_name(m, i)),
                );
            }
            v
        })
        .collect()
}

/// 部品の要約（「線形 + 輝度（交差）」など）。
fn components_text(m: &LocalAdjustment) -> String {
    if m.components.is_empty() {
        return "（部品なし）".to_owned();
    }
    m.components
        .iter()
        .map(|c| {
            let kind = match &c.shape {
                MaskShape::Linear { .. } => "線形".to_owned(),
                MaskShape::Radial { .. } => "円形".to_owned(),
                MaskShape::Brush { strokes } => format!("ブラシ {} 本", strokes.len()),
                MaskShape::ColorRange { samples, .. } => format!("色域 {} 色", samples.len()),
                MaskShape::LuminanceRange { min, max, .. } => format!("輝度 {min}〜{max}"),
                MaskShape::Ai { kind, model, .. } => {
                    format!("AI {}（{model}）", kind.display_name())
                }
            };
            let mode = match c.mode {
                MaskMode::Add => "",
                MaskMode::Subtract => "（除く）",
                MaskMode::Intersect => "（交差）",
            };
            format!("{kind}{mode}{}", if c.invert { "（反転）" } else { "" })
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

/// 調整の要約（0 でない項目だけ）。
fn delta_text(d: &LocalDelta) -> String {
    let parts: Vec<String> = LocalDelta::NAMES
        .iter()
        .zip(d.values())
        .filter(|(_, v)| *v != 0.0)
        .map(|(name, v)| {
            if *name == "exposure_ev" {
                format!("{name} {v:+.2}")
            } else {
                format!("{name} {v:+.0}")
            }
        })
        .collect();
    if parts.is_empty() {
        "（調整なし）".to_owned()
    } else {
        parts.join(", ")
    }
}

fn print_masks(out: Output, v: VariantId, settings: &DevelopSettings, changed: Option<bool>) {
    if out.json {
        let mut obj = json!({ "variant_id": v, "masks": masks_json(settings) });
        if let Some(c) = changed {
            obj["changed"] = json!(c);
        }
        out.print_json(&obj);
        return;
    }
    if settings.masks.is_empty() {
        out.line("マスクはありません");
        return;
    }
    let mut t = Table::new(["番号", "名前", "有効", "部品", "調整"]);
    for (i, m) in settings.masks.iter().enumerate() {
        t.row([
            (i + 1).to_string(),
            genzo_api::mask_display_name(m, i),
            if m.enabled { "はい" } else { "いいえ" }.to_owned(),
            components_text(m),
            delta_text(&m.delta),
        ]);
    }
    out.table(&t);
}

fn list(g: &GlobalArgs, out: Output, v: VariantId) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let settings = s.core.develop_settings(v)?;
    s.close()?;
    print_masks(out, v, &settings, None);
    Ok(Status::Success)
}

/// 設定のマスクを変えて保存する（検証してから。変わらなければ履歴に記録しない）。
fn change(
    g: &GlobalArgs,
    out: Output,
    v: VariantId,
    edit: impl FnOnce(&Session, &mut DevelopSettings) -> CliResult<()>,
) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let current = s.core.develop_settings(v)?;
    let mut new = current.clone();
    edit(&s, &mut new)?;
    new.validate().map_err(genzo_model::DevelopError::from)?;
    let changed = save(&s, v, &new, None)?;
    let saved = s.core.develop_settings(v)?;
    s.close()?;
    if !changed && !out.json {
        out.progress("設定は変わっていません（履歴に記録しませんでした）");
    }
    if changed
        && saved.process_version < genzo_pipeline::ProcessVersion::V2.get()
        && !saved.masks.is_empty()
    {
        out.warn(format!(
            "この写真の処理バージョン（{}）では、マスクは使われません（genzo develop upgrade {v} で更新できます）",
            saved.process_version
        ));
    }
    print_masks(out, v, &saved, Some(changed));
    Ok(Status::Success)
}

/// `develop mask overlay`: 写真を開いて、マスクの重みをグレーの PNG に書く。
fn overlay(
    g: &GlobalArgs,
    out: Output,
    v: VariantId,
    number: usize,
    path: &std::path::Path,
    uncropped: bool,
) -> CliResult<Status> {
    if path.exists() {
        return Err(CliError::Usage(format!(
            "{} は既にあります（上書きしません）",
            path.display()
        )));
    }
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &[v])?;
    let settings = s.core.develop_settings(v)?;
    let i = index_of(&settings, number)?;
    s.core.open_develop(v)?;
    let o = s.core.mask_overlay(v, i, Some(uncropped));
    s.core.close_develop()?;
    s.close()?;
    let o = o?;
    let rgb: Vec<u8> = o.weights.iter().flat_map(|&w| [w, w, w]).collect();
    let image = genzo_media::RgbImage8::from_raw(o.info.width, o.info.height, rgb)
        .map_err(|e| CliError::other(e.to_string()))?;
    let png = genzo_media::png_io::encode_png(&image, None, None)
        .map_err(|e| CliError::other(e.to_string()))?;
    write_new_file(path, &png)?;
    let covered = o.weights.iter().filter(|&&w| w > 0).count();
    if out.json {
        out.print_json(&json!({
            "path": path,
            "overlay": o.info,
            "covered_pixels": covered,
        }));
    } else {
        out.line(format!(
            "マスク {number}（{}）の重みを書き出しました: {}（{}×{}。重みが 0 でない画素 {covered}）",
            genzo_api::mask_display_name(&settings.masks[i], i),
            path.display(),
            o.info.width,
            o.info.height,
        ));
    }
    Ok(Status::Success)
}

/// `develop mask pick-color`: 写真を開いて、プレビューの点の G 座標と色を表示する。
fn pick_color(g: &GlobalArgs, out: Output, v: VariantId, x: f64, y: f64) -> CliResult<Status> {
    if !((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)) {
        return Err(CliError::Usage(
            "--x・--y は 0〜1 の割合で指定してください".to_owned(),
        ));
    }
    let s = Session::open(g, out, &OpenOptions::default())?;
    let samples = pick_samples(&s, v, &[[x, y]]);
    s.close()?;
    let p = samples?.remove(0);
    if out.json {
        out.print_json(&json!({ "variant_id": v, "x": p.x, "y": p.y, "lab": p.lab }));
    } else {
        out.line(format!(
            "G 座標 ({:.4}, {:.4})・OKLab ({:.4}, {:.4}, {:.4})",
            p.x, p.y, p.lab[0], p.lab[1], p.lab[2]
        ));
    }
    Ok(Status::Success)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_numbers_start_at_one() {
        let mut s = DevelopSettings::default();
        assert!(index_of(&s, 1).is_err());
        s.masks.push(LocalAdjustment::new(MaskComponent::add(
            MaskShape::LuminanceRange {
                min: 0.0,
                max: 100.0,
                smoothness: 50.0,
            },
        )));
        assert_eq!(index_of(&s, 1).unwrap(), 0);
        assert!(index_of(&s, 0).is_err());
        assert!(index_of(&s, 2).is_err());
    }

    #[test]
    fn summaries_show_the_parts_and_the_nonzero_adjustments() {
        let mut m = LocalAdjustment::new(MaskComponent::add(MaskShape::Linear {
            x0: 0.5,
            y0: 0.0,
            x1: 0.5,
            y1: 0.3,
        }));
        m.components.push(MaskComponent {
            mode: MaskMode::Intersect,
            invert: true,
            shape: MaskShape::LuminanceRange {
                min: 50.0,
                max: 100.0,
                smoothness: 20.0,
            },
        });
        assert_eq!(components_text(&m), "線形 + 輝度 50〜100（交差）（反転）");
        assert_eq!(delta_text(&m.delta), "（調整なし）");
        m.delta.exposure_ev = -0.5;
        m.delta.dehaze = 20.0;
        assert_eq!(delta_text(&m.delta), "exposure_ev -0.50, dehaze +20");
    }
}
