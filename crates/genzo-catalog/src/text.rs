//! 文字列の正規化（docs/04_architecture.md の 3.5 節・3.6 節）。
//!
//! - **パスの比較キー**（`folder.rel_path_key`、`file.name_key`。3.5 節）: Unicode の NFC 正規化
//!   ＋ 小文字化。Windows（NTFS）と macOS（APFS）の既定のファイルシステムは大文字・小文字を
//!   区別せず、macOS は正規化の違い（NFC / NFD）も区別しないため、それに合わせる。
//!   表示には元の文字列を使う。
//! - **検索のキー**（3.6 節。レビュー R-11）: NFKC 正規化 ＋ 小文字化。全角・半角の英数字、
//!   英字の大文字・小文字を区別しない。濁点の有無は区別する（NFKC は濁点を結合した 1 文字に
//!   まとめるだけで、取り除かない）。検索する側とされる側の両方に同じ関数を使う。
//! - **検索式の組み立て**: ユーザーの入力をそのまま FTS5 の検索式にしない。空白で区切った
//!   各語を二重引用符で囲み（`"` は `""` にする）、3 文字以上の語は FTS5（trigram）、
//!   1〜2 文字の語は正規化した列への `LIKE`（`%`・`_`・エスケープ文字をエスケープ）で探す。

use unicode_normalization::{IsNormalized, UnicodeNormalization, is_nfc_quick, is_nfkc_quick};

/// パスの比較キー（NFC ＋ 小文字化。3.5 節）。
pub fn path_key(s: &str) -> String {
    let nfc: String = if is_nfc_quick(s.chars()) == IsNormalized::Yes {
        s.to_owned()
    } else {
        s.nfc().collect()
    };
    nfc.to_lowercase()
}

/// 検索のキー（NFKC ＋ 小文字化。3.6 節）。
pub fn search_key(s: &str) -> String {
    let nfkc: String = if is_nfkc_quick(s.chars()) == IsNormalized::Yes {
        s.to_owned()
    } else {
        s.nfkc().collect()
    };
    nfkc.to_lowercase()
}

/// FTS5 の trigram トークナイザーで検索できる最小の文字数（3.6 節）。
///
/// trigram は 3 文字の並びを索引にするため、2 文字以下の語は FTS5 では見つからない
/// （SQLite の FTS5 の説明書の trigram の節）。
pub const TRIGRAM_MIN_CHARS: usize = 3;

/// `LIKE` のエスケープ文字（`ESCAPE '\'`）。
pub(crate) const LIKE_ESCAPE: char = '\\';

/// `LIKE` のパターンで特別な意味を持つ文字（`%`・`_`・エスケープ文字）をエスケープする。
pub(crate) fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        if matches!(c, '%' | '_' | LIKE_ESCAPE) {
            out.push(LIKE_ESCAPE);
        }
        out.push(c);
    }
    out
}

/// 部分一致の `LIKE` のパターン（`%語%`）を作る。
pub(crate) fn like_contains_pattern(term: &str) -> String {
    format!("%{}%", escape_like(term))
}

/// 前方一致の `LIKE` のパターン（`語%`）を作る。
pub(crate) fn like_prefix_pattern(term: &str) -> String {
    format!("{}%", escape_like(term))
}

/// FTS5 の文字列（フレーズ）として二重引用符で囲む（`"` は `""` にする）。
///
/// 囲んだ中では `*`・`:`・`^`・`(`・`)`・`-`・`OR`・`AND`・`NEAR` などは演算子として
/// 解釈されない（FTS5 の説明書の「3.1 FTS5 Strings」）。
pub(crate) fn fts_phrase(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

/// テキスト検索の入力を、FTS5 の検索式と `LIKE` のパターンに分けたもの（3.6 節）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextQuery {
    /// 3 文字以上の語を AND でつないだ FTS5 の検索式（なければ `None`）。
    pub fts_match: Option<String>,
    /// 1〜2 文字の語の `LIKE` のパターン（`ESCAPE '\'` で使う。すべてを AND で満たす）。
    pub like_patterns: Vec<String>,
}

impl TextQuery {
    /// 検索語が 1 つもない（空白だけの入力など）か。
    pub fn is_empty(&self) -> bool {
        self.fts_match.is_none() && self.like_patterns.is_empty()
    }
}

/// 検索語の区切りとして扱う文字か（空白と制御文字）。
///
/// 制御文字（特に NUL）を語に含めると、SQLite が C の文字列として扱う箇所で語が NUL の位置で
/// 切れる。FTS5 の検索式では引用符が閉じられずに構文エラーになり（"unterminated string"）、
/// `LIKE` のパターンでは `%` だけが残って全件に一致してしまう（レビューで再現）。
/// 制御文字はファイル名・キャプションの検索語として意味を持たないので、区切りとして扱う。
fn is_term_separator(c: char) -> bool {
    c.is_whitespace() || c.is_control()
}

/// テキスト検索の入力を解析する（3.6 節）。
///
/// 入力を [`search_key`] で正規化してから空白（NFKC で全角の空白も半角になる）と制御文字で
/// 区切る。同じ語が重複していても結果は変わらないため、そのまま使う。
pub fn parse_text_query(input: &str) -> TextQuery {
    let normalized = search_key(input);
    let mut fts_terms = Vec::new();
    let mut like_patterns = Vec::new();
    for term in normalized
        .split(is_term_separator)
        .filter(|t| !t.is_empty())
    {
        if term.chars().count() >= TRIGRAM_MIN_CHARS {
            fts_terms.push(fts_phrase(term));
        } else {
            like_patterns.push(like_contains_pattern(term));
        }
    }
    TextQuery {
        fts_match: (!fts_terms.is_empty()).then(|| fts_terms.join(" AND ")),
        like_patterns,
    }
}

/// 検索対象の文字列（ファイル名とキャプション）を、索引に入れる 1 つの文字列にする。
///
/// 各部分を [`search_key`] で正規化し、改行でつなぐ（検索語は空白で区切るため、
/// 改行をまたいで一致することはない）。
pub(crate) fn searchable_text<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        // 部分の中の改行は空白にする（区切りの改行と区別するため）。
        let key = search_key(part);
        out.extend(key.chars().map(|c| if c == '\n' { ' ' } else { c }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_key_is_nfc_and_lowercase() {
        // macOS の HFS+ が返す NFD（か + 濁点）と NFC（が）を同じキーにする。
        let nfd = "\u{304b}\u{3099}.JPG";
        let nfc = "\u{304c}.jpg";
        assert_eq!(path_key(nfd), path_key(nfc));
        assert_eq!(path_key("DSC00001.ARW"), "dsc00001.arw");
        // 全角英字は NFC では変わらない（パスとしては別の名前）。
        assert_ne!(path_key("ＡＢＣ"), path_key("abc"));
    }

    #[test]
    fn search_key_is_nfkc_and_lowercase() {
        assert_eq!(search_key("ＡＢＣ"), "abc");
        assert_eq!(search_key("ABC"), "abc");
        assert_eq!(search_key("１２３"), "123");
        // 半角カナは全角に、濁点は結合した 1 文字になる。
        assert_eq!(search_key("ｶﾞ"), "ガ");
        // 濁点の有無は区別する。
        assert_ne!(search_key("か"), search_key("が"));
        // 全角の空白は半角の空白になる。
        assert_eq!(search_key("京都\u{3000}旅行"), "京都 旅行");
    }

    #[test]
    fn escape_like_escapes_wildcards_and_escape_char() {
        assert_eq!(escape_like("100%_a\\b"), "100\\%\\_a\\\\b");
        assert_eq!(like_contains_pattern("海"), "%海%");
        assert_eq!(like_prefix_pattern("a_"), "a\\_%");
    }

    #[test]
    fn fts_phrase_quotes_and_doubles_quotes() {
        assert_eq!(fts_phrase("京都"), "\"京都\"");
        assert_eq!(fts_phrase("a\"b"), "\"a\"\"b\"");
        assert_eq!(fts_phrase("\""), "\"\"\"\"");
    }

    #[test]
    fn parse_text_query_splits_by_length() {
        let q = parse_text_query("京都旅行  海 ＡＢＣ");
        assert_eq!(q.fts_match.as_deref(), Some("\"京都旅行\" AND \"abc\""));
        assert_eq!(q.like_patterns, vec!["%海%".to_owned()]);
        assert!(!q.is_empty());

        let q = parse_text_query("  \u{3000} ");
        assert!(q.is_empty());

        // 制御文字（NUL など）は区切りとして扱い、語に含めない。
        let q = parse_text_query("ab\0cde\u{1}f\tg");
        assert_eq!(q.fts_match.as_deref(), Some("\"cde\""));
        assert_eq!(q.like_patterns, vec!["%ab%", "%f%", "%g%"]);
        assert!(parse_text_query("\0\u{7f}\u{9f}").is_empty());

        // FTS5 の演算子は語として引用符で囲まれる（2 文字以下は LIKE）。
        let q = parse_text_query("NEAR OR \"x* ^a:b (c) -d");
        assert_eq!(
            q.fts_match.as_deref(),
            Some("\"near\" AND \"\"\"x*\" AND \"^a:b\" AND \"(c)\"")
        );
        assert_eq!(
            q.like_patterns,
            vec!["%or%", "%-d%"]
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn searchable_text_joins_normalized_parts() {
        assert_eq!(
            searchable_text(["DSC0001.ARW", "", "京都\n旅行 ＡＢＣ"]),
            "dsc0001.arw\n京都 旅行 abc"
        );
        assert_eq!(searchable_text(Vec::<&str>::new()), "");
    }
}
