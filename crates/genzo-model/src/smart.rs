//! スマートコレクションの条件（LIB-11。docs/04_architecture.md の 3.9 節）。
//!
//! 条件は版付きの JSON でカタログの `collection.smart_rule_json` に保存し、検索のたびに SQL に変換する
//! （genzo-catalog の `smart.rs`）。写真の評価・キーワードなどが変われば、次の検索（表示中の検索結果の
//! 作り直し。3.7 節）で結果が変わる（自動で更新される集まり）。
//!
//! JSON の形（`type` で条件の種類を表す。グループで入れ子にできる）:
//!
//! ```json
//! {
//!   "version": 1,
//!   "match": "all",
//!   "rules": [
//!     {"type": "rating", "op": "gte", "value": 3},
//!     {"type": "keyword", "op": "has", "keyword_id": 12},
//!     {"type": "group", "match": "any", "rules": [
//!       {"type": "camera", "op": "is", "value": "ILCE-7M4"},
//!       {"type": "kind", "value": "video"}
//!     ]}
//!   ]
//! }
//! ```
//!
//! - `match` が `all` なら条件のすべて、`any` ならいずれかに一致する。条件が空のグループは、`all` なら
//!   すべてに一致し、`any` ならどれにも一致しない。
//! - 撮影日時の範囲は UTC の時刻（画面は、カタログの既定のタイムゾーンの日付を UTC にして保存する。
//!   絞り込みの `captured_from`・`captured_until` と同じ）。`from` を含み、`until` を含まない。
//! - キーワードの `has` は、子のキーワードが付いた写真も一致する（LIB-09）。
//! - 削除したキーワード・フォルダ・コレクションを参照する条件は、どの写真にも一致しない（`not_has`・
//!   `not_in` はすべてに一致する）。カタログはこれらの ID を削除した後も使い回さない（genzo-catalog の
//!   スキーマの版 8。`AUTOINCREMENT`）ので、後から作ったものに付け替わらない。
//! - 版（[`SMART_RULE_VERSION`]）より新しい版の条件は読まない（[`SmartRuleError::NewerVersion`]）。
//!   条件の種類・演算子を足すときは版を上げずに足してよい（古い条件はそのまま読める）。意味を変えるときは
//!   版を上げ、古い版を読み替える処理を足す。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::catalog::{AssetKind, ColorLabel, Flag, Rating};
use crate::ids::{CollectionId, FolderId, KeywordId};

/// 条件の JSON の今の版。
pub const SMART_RULE_VERSION: u32 = 1;

/// グループの入れ子の深さの上限（最上位を 1 とする。**仮置き**: 8。画面で編集できる程度）。
pub const MAX_SMART_DEPTH: usize = 8;

/// 条件（グループを含む）の数の上限（**仮置き**: 200。SQL が大きくなりすぎないように）。
pub const MAX_SMART_CONDITIONS: usize = 200;

/// テキストの条件の値の文字数の上限（**仮置き**: 200）。
pub const MAX_SMART_TEXT_CHARS: usize = 200;

/// 「直近 N 日」の N の上限（**仮置き**: 36,500 日 ≒ 100 年）。
pub const MAX_SMART_DAYS: u32 = 36_500;

/// すべて / いずれか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// すべての条件に一致する。
    #[default]
    All,
    /// いずれかの条件に一致する。
    Any,
}

/// 数の比較（評価）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberOp {
    /// 等しい。
    Eq,
    /// 等しくない。
    Ne,
    /// 以上。
    Gte,
    /// 以下。
    Lte,
}

/// 等しい / 等しくない（フラグ・カラーラベル）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqualityOp {
    /// 等しい。
    Is,
    /// 等しくない。
    IsNot,
}

/// キーワードの条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeywordOp {
    /// そのキーワード（子のキーワードを含む）が付いている。
    Has,
    /// そのキーワード（子のキーワードを含む）が付いていない。
    NotHas,
    /// 何かのキーワードが付いている。
    Any,
    /// キーワードが 1 つも付いていない。
    None,
}

/// 撮影日時の条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateOp {
    /// 範囲（`from` を含み `until` を含まない。片方だけでもよい）。撮影日時の分からないものは除く。
    Range,
    /// 直近 `days` 日（検索した時刻から `days` × 24 時間前まで）。
    InLastDays,
    /// 撮影日時が分からない。
    Unknown,
}

/// 文字列の条件（カメラ・レンズ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextOp {
    /// 完全一致（絞り込みの候補の値）。
    Is,
    /// 完全一致しない（値の分からないものも含む）。
    IsNot,
    /// 部分一致（ASCII の英字の大文字・小文字を区別しない）。
    Contains,
    /// 値が分からない（空）。
    Unknown,
}

/// テキスト検索の条件（ファイル名・キャプション・タイトルなど。3.6 節）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainsOp {
    /// 含む（空白で区切った語をすべて含む）。
    Contains,
    /// 含まない。
    NotContains,
}

/// 含まれる / 含まれない（フォルダ・コレクション）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberOp {
    /// 含まれる。
    In,
    /// 含まれない。
    NotIn,
}

/// 1 つの条件（またはグループ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SmartCondition {
    /// 条件のグループ（入れ子）。
    Group {
        /// すべて / いずれか。
        #[serde(rename = "match", default)]
        match_mode: MatchMode,
        /// 条件。
        #[serde(default)]
        rules: Vec<SmartCondition>,
    },
    /// 評価。
    Rating {
        /// 比較。
        op: NumberOp,
        /// 評価（0〜5）。
        value: Rating,
    },
    /// フラグ。
    Flag {
        /// 比較。
        op: EqualityOp,
        /// フラグ（-1 / 0 / 1）。
        value: Flag,
    },
    /// カラーラベル。
    ColorLabel {
        /// 比較。
        op: EqualityOp,
        /// ラベル（`null` はラベルなし）。
        #[serde(default)]
        value: Option<ColorLabel>,
    },
    /// キーワード。
    Keyword {
        /// 条件。
        op: KeywordOp,
        /// キーワード（`has`・`not_has` のときに必要）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keyword_id: Option<KeywordId>,
    },
    /// 撮影日時。
    CaptureTime {
        /// 条件。
        op: DateOp,
        /// 下限（含む。`range`）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<DateTime<Utc>>,
        /// 上限（含まない。`range`）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<DateTime<Utc>>,
        /// 日数（`in_last_days`）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        days: Option<u32>,
    },
    /// カメラ（`asset.camera`）。
    Camera {
        /// 比較。
        op: TextOp,
        /// 値（`unknown` では使わない）。
        #[serde(default)]
        value: String,
    },
    /// レンズ（`asset.lens`）。
    Lens {
        /// 比較。
        op: TextOp,
        /// 値（`unknown` では使わない）。
        #[serde(default)]
        value: String,
    },
    /// 種類（写真 / 動画）。
    Kind {
        /// 種類。
        value: AssetKind,
    },
    /// テキスト（ファイル名・キャプション・タイトルなど。3.6 節）。
    Text {
        /// 含む / 含まない。
        op: ContainsOp,
        /// 検索語（空白で区切った語をすべて含む）。
        value: String,
    },
    /// フォルダ。
    Folder {
        /// 含まれる / 含まれない。
        op: MemberOp,
        /// フォルダ。
        folder_id: FolderId,
        /// 配下のフォルダも含めるか。
        #[serde(default)]
        include_subfolders: bool,
    },
    /// コレクション（手動のコレクション・スマートコレクション・コレクションセット）。
    Collection {
        /// 含まれる / 含まれない。
        op: MemberOp,
        /// コレクション。
        collection_id: CollectionId,
    },
}

/// スマートコレクションの条件（最上位のグループ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SmartRule {
    /// JSON の版（[`SMART_RULE_VERSION`]）。
    pub version: u32,
    /// すべて / いずれか。
    #[serde(rename = "match", default)]
    pub match_mode: MatchMode,
    /// 条件。
    #[serde(default)]
    pub rules: Vec<SmartCondition>,
}

impl Default for SmartRule {
    fn default() -> Self {
        Self {
            version: SMART_RULE_VERSION,
            match_mode: MatchMode::All,
            rules: Vec::new(),
        }
    }
}

/// 条件の検証・読み取りのエラー。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SmartRuleError {
    /// JSON として読めない・形が違う。
    #[error("スマートコレクションの条件を読めません: {0}")]
    Invalid(String),
    /// アプリより新しい版の条件。
    #[error(
        "スマートコレクションの条件の版 {found} は、このアプリ（版 {supported} まで）では読めません。アプリを更新してください"
    )]
    NewerVersion {
        /// 条件の版。
        found: u32,
        /// このアプリが読める版。
        supported: u32,
    },
    /// 版が 0 など。
    #[error("スマートコレクションの条件の版が不正です: {0}")]
    BadVersion(u32),
    /// 入れ子が深すぎる。
    #[error("条件のグループの入れ子は {MAX_SMART_DEPTH} 段までです")]
    TooDeep,
    /// 条件が多すぎる。
    #[error("条件は {MAX_SMART_CONDITIONS} 個までです")]
    TooMany,
    /// 条件の値が足りない・範囲外。
    #[error("{0}")]
    BadCondition(String),
}

impl SmartRule {
    /// 条件のない（すべてに一致する）条件。
    pub fn new(match_mode: MatchMode, rules: Vec<SmartCondition>) -> Self {
        Self {
            version: SMART_RULE_VERSION,
            match_mode,
            rules,
        }
    }

    /// JSON から読んで検証する。版がアプリより新しければ [`SmartRuleError::NewerVersion`]。
    pub fn from_json(json: &str) -> Result<Self, SmartRuleError> {
        // 形を読む前に版を確かめる（新しい版の未知の条件を「形が違う」と報告しないため）。
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|e| SmartRuleError::Invalid(e.to_string()))?;
        if let Some(v) = value.get("version").and_then(serde_json::Value::as_u64) {
            let found = u32::try_from(v).unwrap_or(u32::MAX);
            if found > SMART_RULE_VERSION {
                return Err(SmartRuleError::NewerVersion {
                    found,
                    supported: SMART_RULE_VERSION,
                });
            }
        }
        let rule: Self =
            serde_json::from_value(value).map_err(|e| SmartRuleError::Invalid(e.to_string()))?;
        rule.validate()?;
        Ok(rule)
    }

    /// JSON にする（カタログに保存する形）。
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("条件は JSON にできる")
    }

    /// 検証する（版・入れ子の深さ・数・各条件の値）。
    pub fn validate(&self) -> Result<(), SmartRuleError> {
        if self.version == 0 {
            return Err(SmartRuleError::BadVersion(self.version));
        }
        if self.version > SMART_RULE_VERSION {
            return Err(SmartRuleError::NewerVersion {
                found: self.version,
                supported: SMART_RULE_VERSION,
            });
        }
        let mut count = 0usize;
        validate_rules(&self.rules, 1, &mut count)
    }

    /// 参照しているコレクション（重複なし、出てきた順）。
    pub fn referenced_collections(&self) -> Vec<CollectionId> {
        let mut out = Vec::new();
        walk(&self.rules, &mut |c| {
            if let SmartCondition::Collection { collection_id, .. } = c
                && !out.contains(collection_id)
            {
                out.push(*collection_id);
            }
        });
        out
    }

    /// 参照しているキーワード（重複なし、出てきた順）。
    pub fn referenced_keywords(&self) -> Vec<KeywordId> {
        let mut out = Vec::new();
        walk(&self.rules, &mut |c| {
            if let SmartCondition::Keyword {
                keyword_id: Some(k),
                ..
            } = c
                && !out.contains(k)
            {
                out.push(*k);
            }
        });
        out
    }
}

/// 条件の木をたどる（グループの中も）。
fn walk(rules: &[SmartCondition], f: &mut impl FnMut(&SmartCondition)) {
    for c in rules {
        f(c);
        if let SmartCondition::Group { rules, .. } = c {
            walk(rules, f);
        }
    }
}

fn bad(msg: impl Into<String>) -> SmartRuleError {
    SmartRuleError::BadCondition(msg.into())
}

fn check_text(value: &str, what: &str) -> Result<(), SmartRuleError> {
    let t = value.trim();
    if t.is_empty() {
        return Err(bad(format!("{what}の値が空です")));
    }
    if t.chars().count() > MAX_SMART_TEXT_CHARS {
        return Err(bad(format!(
            "{what}の値は {MAX_SMART_TEXT_CHARS} 文字までです"
        )));
    }
    if t.contains('\0') {
        return Err(bad(format!("{what}の値に NUL 文字が含まれています")));
    }
    Ok(())
}

fn validate_rules(
    rules: &[SmartCondition],
    depth: usize,
    count: &mut usize,
) -> Result<(), SmartRuleError> {
    if depth > MAX_SMART_DEPTH {
        return Err(SmartRuleError::TooDeep);
    }
    for c in rules {
        *count += 1;
        if *count > MAX_SMART_CONDITIONS {
            return Err(SmartRuleError::TooMany);
        }
        match c {
            SmartCondition::Group { rules, .. } => validate_rules(rules, depth + 1, count)?,
            SmartCondition::Keyword { op, keyword_id } => match op {
                KeywordOp::Has | KeywordOp::NotHas if keyword_id.is_none() => {
                    return Err(bad("キーワードの条件にキーワードが指定されていません"));
                }
                _ => {}
            },
            SmartCondition::CaptureTime {
                op,
                from,
                until,
                days,
            } => match op {
                DateOp::Range => {
                    if from.is_none() && until.is_none() {
                        return Err(bad("撮影日の範囲の開始か終了を指定してください"));
                    }
                    if let (Some(f), Some(u)) = (from, until)
                        && f >= u
                    {
                        return Err(bad("撮影日の範囲の終了は開始より後にしてください"));
                    }
                }
                DateOp::InLastDays => match days {
                    Some(d) if (1..=MAX_SMART_DAYS).contains(d) => {}
                    _ => {
                        return Err(bad(format!(
                            "「直近の日数」は 1〜{MAX_SMART_DAYS} 日で指定してください"
                        )));
                    }
                },
                DateOp::Unknown => {}
            },
            SmartCondition::Camera { op, value } | SmartCondition::Lens { op, value } => {
                let what = if matches!(c, SmartCondition::Camera { .. }) {
                    "カメラ"
                } else {
                    "レンズ"
                };
                if *op != TextOp::Unknown {
                    check_text(value, what)?;
                }
            }
            SmartCondition::Text { value, .. } => check_text(value, "テキスト")?,
            SmartCondition::Rating { .. }
            | SmartCondition::Flag { .. }
            | SmartCondition::ColorLabel { .. }
            | SmartCondition::Kind { .. }
            | SmartCondition::Folder { .. }
            | SmartCondition::Collection { .. } => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn sample() -> SmartRule {
        SmartRule::new(
            MatchMode::All,
            vec![
                SmartCondition::Rating {
                    op: NumberOp::Gte,
                    value: Rating::new(3).unwrap(),
                },
                SmartCondition::Keyword {
                    op: KeywordOp::Has,
                    keyword_id: Some(KeywordId::new(12)),
                },
                SmartCondition::Group {
                    match_mode: MatchMode::Any,
                    rules: vec![
                        SmartCondition::Camera {
                            op: TextOp::Is,
                            value: "ILCE-7M4".to_owned(),
                        },
                        SmartCondition::Kind {
                            value: AssetKind::Video,
                        },
                        SmartCondition::Collection {
                            op: MemberOp::NotIn,
                            collection_id: CollectionId::new(4),
                        },
                    ],
                },
            ],
        )
    }

    #[test]
    fn json_form_is_plain_and_round_trips() {
        let rule = sample();
        let json = rule.to_json();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap(),
            serde_json::json!({
                "version": 1,
                "match": "all",
                "rules": [
                    {"type": "rating", "op": "gte", "value": 3},
                    {"type": "keyword", "op": "has", "keyword_id": 12},
                    {"type": "group", "match": "any", "rules": [
                        {"type": "camera", "op": "is", "value": "ILCE-7M4"},
                        {"type": "kind", "value": "video"},
                        {"type": "collection", "op": "not_in", "collection_id": 4}
                    ]}
                ]
            })
        );
        assert_eq!(SmartRule::from_json(&json).unwrap(), rule);
        assert_eq!(rule.referenced_collections(), vec![CollectionId::new(4)]);
        assert_eq!(rule.referenced_keywords(), vec![KeywordId::new(12)]);
    }

    #[test]
    fn every_condition_kind_parses() {
        let json = r#"{"version":1,"match":"any","rules":[
            {"type":"flag","op":"is_not","value":-1},
            {"type":"color_label","op":"is","value":null},
            {"type":"color_label","op":"is","value":"red"},
            {"type":"keyword","op":"none"},
            {"type":"capture_time","op":"range","from":"2024-01-01T00:00:00Z","until":"2025-01-01T00:00:00Z"},
            {"type":"capture_time","op":"in_last_days","days":30},
            {"type":"capture_time","op":"unknown"},
            {"type":"lens","op":"unknown"},
            {"type":"lens","op":"contains","value":"FE 24"},
            {"type":"text","op":"not_contains","value":"京都"},
            {"type":"folder","op":"in","folder_id":3,"include_subfolders":true},
            {"type":"group","rules":[]}
        ]}"#;
        let rule = SmartRule::from_json(json).unwrap();
        assert_eq!(rule.match_mode, MatchMode::Any);
        assert_eq!(rule.rules.len(), 12);
        assert_eq!(
            rule.rules[4],
            SmartCondition::CaptureTime {
                op: DateOp::Range,
                from: Some(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()),
                until: Some(Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()),
                days: None,
            }
        );
        // 省略したグループの match は all。
        assert_eq!(
            rule.rules[11],
            SmartCondition::Group {
                match_mode: MatchMode::All,
                rules: Vec::new()
            }
        );
        // 往復しても同じ。
        assert_eq!(SmartRule::from_json(&rule.to_json()).unwrap(), rule);
    }

    #[test]
    fn newer_versions_and_bad_shapes_are_rejected() {
        assert_eq!(
            SmartRule::from_json(r#"{"version":2,"match":"all","rules":[{"type":"future"}]}"#),
            Err(SmartRuleError::NewerVersion {
                found: 2,
                supported: 1
            })
        );
        assert!(matches!(
            SmartRule::from_json(r#"{"version":0,"match":"all","rules":[]}"#),
            Err(SmartRuleError::BadVersion(0))
        ));
        assert!(matches!(
            SmartRule::from_json(r#"{"version":1,"rules":[{"type":"future"}]}"#),
            Err(SmartRuleError::Invalid(_))
        ));
        assert!(matches!(
            SmartRule::from_json("not json"),
            Err(SmartRuleError::Invalid(_))
        ));
        // 評価の範囲外は形の段階で弾く。
        assert!(matches!(
            SmartRule::from_json(
                r#"{"version":1,"rules":[{"type":"rating","op":"eq","value":6}]}"#
            ),
            Err(SmartRuleError::Invalid(_))
        ));
    }

    #[test]
    fn condition_values_are_validated() {
        let one = |c: SmartCondition| SmartRule::new(MatchMode::All, vec![c]).validate();
        assert!(
            one(SmartCondition::Keyword {
                op: KeywordOp::Has,
                keyword_id: None
            })
            .is_err()
        );
        assert!(
            one(SmartCondition::Keyword {
                op: KeywordOp::Any,
                keyword_id: None
            })
            .is_ok()
        );
        let t = |y| Utc.with_ymd_and_hms(y, 1, 1, 0, 0, 0).unwrap();
        assert!(
            one(SmartCondition::CaptureTime {
                op: DateOp::Range,
                from: None,
                until: None,
                days: None
            })
            .is_err()
        );
        assert!(
            one(SmartCondition::CaptureTime {
                op: DateOp::Range,
                from: Some(t(2025)),
                until: Some(t(2024)),
                days: None
            })
            .is_err()
        );
        assert!(
            one(SmartCondition::CaptureTime {
                op: DateOp::Range,
                from: Some(t(2024)),
                until: None,
                days: None
            })
            .is_ok()
        );
        for days in [None, Some(0), Some(MAX_SMART_DAYS + 1)] {
            assert!(
                one(SmartCondition::CaptureTime {
                    op: DateOp::InLastDays,
                    from: None,
                    until: None,
                    days
                })
                .is_err(),
                "{days:?}"
            );
        }
        assert!(
            one(SmartCondition::Camera {
                op: TextOp::Is,
                value: "  ".to_owned()
            })
            .is_err()
        );
        assert!(
            one(SmartCondition::Camera {
                op: TextOp::Unknown,
                value: String::new()
            })
            .is_ok()
        );
        assert!(
            one(SmartCondition::Text {
                op: ContainsOp::Contains,
                value: "a\0".to_owned()
            })
            .is_err()
        );
        assert!(
            one(SmartCondition::Text {
                op: ContainsOp::Contains,
                value: "あ".repeat(MAX_SMART_TEXT_CHARS + 1)
            })
            .is_err()
        );
    }

    #[test]
    fn depth_and_count_are_limited() {
        let mut c = SmartCondition::Kind {
            value: AssetKind::Photo,
        };
        for _ in 0..MAX_SMART_DEPTH {
            c = SmartCondition::Group {
                match_mode: MatchMode::All,
                rules: vec![c],
            };
        }
        // 最上位（1 段目）の中に MAX_SMART_DEPTH 段のグループ → 深さ MAX_SMART_DEPTH + 1。
        assert_eq!(
            SmartRule::new(MatchMode::All, vec![c.clone()]).validate(),
            Err(SmartRuleError::TooDeep)
        );
        if let SmartCondition::Group { rules, .. } = c {
            assert!(SmartRule::new(MatchMode::All, rules).validate().is_ok());
        }
        let many = vec![
            SmartCondition::Kind {
                value: AssetKind::Photo
            };
            MAX_SMART_CONDITIONS + 1
        ];
        assert_eq!(
            SmartRule::new(MatchMode::Any, many).validate(),
            Err(SmartRuleError::TooMany)
        );
    }
}
