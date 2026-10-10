//! 出力: 標準出力に結果（表または JSON）、標準エラーに進捗・警告。
//!
//! - `--json` のとき、標準出力には **JSON の文書を 1 つだけ** 出す（成功は結果、失敗は
//!   `{"error": {...}}`）。進捗・警告は標準エラーに人が読む形で出す。
//! - 表は列をそろえて出す。日本語などの全角の文字は 2 桁として数える（[`display_width`]）。

use std::io::Write;

use serde::Serialize;

/// 出力先の設定。
#[derive(Debug, Clone, Copy)]
pub struct Output {
    /// 結果を JSON で出すか。
    pub json: bool,
    /// 進捗を出さないか。
    pub quiet: bool,
}

impl Output {
    /// 結果の JSON を標準出力に出す（整形して 1 つの文書）。
    pub fn print_json<T: Serialize + ?Sized>(&self, value: &T) {
        // JSON にできない値（UTF-8 で表せないパスなど）は、内部のエラーの JSON にする（説明の中の
        // 引用符などはエスケープする）。
        let text = serde_json::to_string_pretty(value).unwrap_or_else(|e| {
            serde_json::json!({
                "error": {
                    "kind": "internal",
                    "message": format!("結果を JSON にできません: {e}"),
                    "user_actionable": false,
                    "retryable": false,
                    "hint": null,
                }
            })
            .to_string()
        });
        let mut out = std::io::stdout().lock();
        // 出力先が閉じられた（パイプの先が終了した）場合は何もしない。
        let _ = writeln!(out, "{text}");
    }

    /// 人が読む結果の 1 行を標準出力に出す。
    pub fn line(&self, text: impl AsRef<str>) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{}", text.as_ref());
    }

    /// 進捗を標準エラーに出す（`--quiet` なら出さない）。
    pub fn progress(&self, text: impl AsRef<str>) {
        if !self.quiet {
            eprintln!("{}", text.as_ref());
        }
    }

    /// 警告を標準エラーに出す（`--quiet` でも出す）。
    pub fn warn(&self, text: impl AsRef<str>) {
        eprintln!("警告: {}", text.as_ref());
    }

    /// 表を標準出力に出す。
    pub fn table(&self, table: &Table) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(table.render().as_bytes());
    }
}

/// 値を JSON の値にする（`json!` の中に入れる結果の部分に使う）。
///
/// `serde_json::to_value`（`json!` が使う）は f32 を f64 に広げるため、現像設定の 0.3 が
/// `0.30000001192092896` になり、`develop get`（構造体を直接 JSON にする）と値の表記が変わる。いったん
/// 文字列にして読み直すと、f32 の最短の表記（`0.3`）になる。
pub fn json_value<T: Serialize + ?Sized>(value: &T) -> serde_json::Value {
    serde_json::to_string(value)
        .and_then(|s| serde_json::from_str(&s))
        .unwrap_or(serde_json::Value::Null)
}

/// 複数行の文字列を 1 行にする（表の欄・一覧の 1 行に入れるため。ワーカーのエラーの説明などには
/// 改行が入る）。各行の前後の空白を除き、空でない行を空白 1 つでつなぐ。
pub fn one_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// 文字列の表示幅（全角の文字を 2 桁とする簡易な判定。東アジアの文字・全角の記号の範囲）。
pub fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

fn char_width(c: char) -> usize {
    let cp = c as u32;
    let wide = matches!(cp,
        0x1100..=0x115F       // ハングルの字母
        | 0x2E80..=0x303E     // CJK の部首・記号と句読点
        | 0x3041..=0x33FF     // ひらがな・カタカナ・CJK の互換
        | 0x3400..=0x4DBF     // CJK 統合漢字拡張 A
        | 0x4E00..=0x9FFF     // CJK 統合漢字
        | 0xA000..=0xA4CF     // イ文字
        | 0xAC00..=0xD7A3     // ハングル
        | 0xF900..=0xFAFF     // CJK 互換漢字
        | 0xFE30..=0xFE4F     // CJK 互換形
        | 0xFF00..=0xFF60     // 全角の英数字・記号
        | 0xFFE0..=0xFFE6     // 全角の記号
        | 0x1F300..=0x1F64F   // 絵文字
        | 0x20000..=0x3FFFD); // CJK 統合漢字拡張 B 以降
    if c.is_control() {
        0
    } else if wide {
        2
    } else {
        1
    }
}

/// 列をそろえた表。
#[derive(Debug, Clone, Default)]
pub struct Table {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    /// 見出しを指定して作る。
    pub fn new<I, S>(header: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            header: header.into_iter().map(Into::into).collect(),
            rows: Vec::new(),
        }
    }

    /// 行を足す（列の数が足りなければ空の欄で補う。欄の改行は [`one_line`] で 1 行にする）。
    pub fn row<I, S>(&mut self, cells: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut r: Vec<String> = cells.into_iter().map(|c| one_line(&c.into())).collect();
        r.resize(self.header.len().max(r.len()), String::new());
        self.rows.push(r);
    }

    /// 行の数。
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// 行がないか。
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// 文字列にする（列の間は 2 桁の空白、最後の列は詰めない）。
    pub fn render(&self) -> String {
        let cols = self
            .rows
            .iter()
            .map(Vec::len)
            .chain([self.header.len()])
            .max()
            .unwrap_or(0);
        let mut widths = vec![0usize; cols];
        for r in std::iter::once(&self.header).chain(&self.rows) {
            for (i, c) in r.iter().enumerate() {
                widths[i] = widths[i].max(display_width(c));
            }
        }
        let mut out = String::new();
        for r in std::iter::once(&self.header).chain(&self.rows) {
            let mut line = String::new();
            for (i, c) in r.iter().enumerate() {
                line.push_str(c);
                if i + 1 < r.len() {
                    let pad = widths[i] - display_width(c) + 2;
                    line.extend(std::iter::repeat_n(' ', pad));
                }
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

/// 色空間の表示名（JSON と同じ名前）。
pub fn color_space_text(c: genzo_model::OutputColorSpace) -> &'static str {
    match c {
        genzo_model::OutputColorSpace::Srgb => "srgb",
        genzo_model::OutputColorSpace::DisplayP3 => "display_p3",
        genzo_model::OutputColorSpace::AdobeRgb => "adobe_rgb",
    }
}

/// UTC からのオフセット（分）の表示（例: 540 → `UTC+09:00`）。
pub fn utc_offset_text(minutes: i32) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    let m = minutes.unsigned_abs();
    format!("UTC{sign}{:02}:{:02}", m / 60, m % 60)
}

/// 項目と値の 2 列の一覧（`show` など）。
pub fn key_values(items: &[(&str, String)]) -> String {
    let w = items
        .iter()
        .map(|(k, _)| display_width(k))
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for (k, v) in items {
        out.push_str(k);
        out.extend(std::iter::repeat_n(' ', w - display_width(k) + 2));
        out.push_str(&one_line(v));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_count_full_width_characters_as_two() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("撮影日時"), 8);
        assert_eq!(display_width("ＡＢ"), 4);
        assert_eq!(
            display_width("★3"),
            2,
            "★ は曖昧な幅の文字（1 桁として数える）"
        );
        assert_eq!(display_width("コピー 1"), 8);
    }

    #[test]
    fn offsets_and_color_spaces() {
        assert_eq!(utc_offset_text(540), "UTC+09:00");
        assert_eq!(utc_offset_text(-210), "UTC-03:30");
        assert_eq!(utc_offset_text(0), "UTC+00:00");
        // JSON（serde）と同じ名前。
        for c in [
            genzo_model::OutputColorSpace::Srgb,
            genzo_model::OutputColorSpace::DisplayP3,
            genzo_model::OutputColorSpace::AdobeRgb,
        ] {
            assert_eq!(
                serde_json::to_value(c).unwrap(),
                serde_json::Value::String(color_space_text(c).to_owned())
            );
        }
    }

    #[test]
    fn json_values_keep_the_short_form_of_f32() {
        #[derive(Serialize)]
        struct S {
            ev: f32,
        }
        let v = serde_json::json!({ "s": json_value(&S { ev: 0.3 }) });
        assert_eq!(v.to_string(), r#"{"s":{"ev":0.3}}"#);
        assert_eq!(
            serde_json::to_string(&S { ev: 0.3 }).unwrap(),
            r#"{"ev":0.3}"#,
            "構造体を直接 JSON にした場合と同じ表記"
        );
    }

    #[test]
    fn multi_line_text_becomes_one_line() {
        assert_eq!(
            one_line("デコードできない: 不足\n（Decode）"),
            "デコードできない: 不足 （Decode）"
        );
        assert_eq!(one_line("a\r\n\r\n  b  \n"), "a b");
        assert_eq!(one_line("一行"), "一行");
        let mut t = Table::new(["ID", "理由"]);
        t.row(["1", "x\ny"]);
        assert_eq!(t.render(), "ID  理由\n1   x y\n");
        assert_eq!(key_values(&[("k", "a\nb".into())]), "k  a b\n");
    }

    #[test]
    fn tables_align_columns() {
        let mut t = Table::new(["ID", "名前", "評価"]);
        t.row(["1", "夕焼け.jpg", "3"]);
        t.row(["12", "a.jpg"]);
        assert_eq!(t.len(), 2);
        assert_eq!(
            t.render(),
            "ID  名前        評価\n1   夕焼け.jpg  3\n12  a.jpg\n"
        );
        assert_eq!(
            key_values(&[("ID", "1".into()), ("撮影日時", "x".into())]),
            "ID        1\n撮影日時  x\n"
        );
    }
}
