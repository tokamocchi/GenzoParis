//! 書き出すファイルの XMP（LIB-15 の説明のメタデータと LIB-09 のキーワード。EXP-04）。
//!
//! カタログの中で編集したタイトル・キャプション・作成者と連絡先・著作権・撮影場所など
//! （genzo-model の [`IptcMetadata`]）と、写真に付けたキーワードを、XMP のパケット（UTF-8 の XML）にする。Exif の ASCII のタグは
//! 7bit の文字しか書けない（Exif 2.32）ので、日本語などはこの XMP で伝える（[`crate::exif_write`] は、
//! ASCII で表せる値だけを ImageDescription・Artist・Copyright に書く）。
//!
//! 使う名前空間とプロパティ（IPTC Photo Metadata Standard の IPTC Core と、XMP の仕様書の Dublin Core・
//! XMP Rights Management。対応は genzo-model の `MetadataField` の doc。原典との照合は人の確認が要る）:
//!
//! | 名前空間 | 接頭辞 | プロパティ |
//! |---|---|---|
//! | `http://purl.org/dc/elements/1.1/` | `dc` | `title`・`description`（言語の別の Alt）、`creator`（Seq）、`rights`（Alt）、`subject`（Bag。キーワード。IPTC Core の Keywords） |
//! | `http://ns.adobe.com/photoshop/1.0/` | `photoshop` | `Headline`・`AuthorsPosition`・`City`・`State`・`Country`・`Credit`・`Source`・`Instructions` |
//! | `http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/` | `Iptc4xmpCore` | `Location`・`CountryCode`・`CreatorContactInfo`（構造体。`CiAdrExtadr` など） |
//! | `http://ns.adobe.com/xap/1.0/rights/` | `xmpRights` | `Marked`（真偽値）・`UsageTerms`（Alt）・`WebStatement` |
//!
//! - 言語の別の値（`rdf:Alt`）は `xml:lang="x-default"` の 1 つだけを書く。
//! - キーワードは名前だけを `dc:subject`（順序のない `rdf:Bag`）に書く。階層（「場所 > 京都」）は書かず、
//!   親のキーワードも 1 つの名前として並べる（`Catalog::export_keywords`。Lightroom の
//!   `lr:hierarchicalSubject` は書かない。IPTC IIM（APP13）の Keywords も書かない。決定事項ログ No.81）。
//! - パケットは読み取り専用の印（`end="r"`）で、余白（padding）は付けない（書き出したファイルを後から
//!   その場で書き換える用途は想定しない）。
//! - 値は XML の特殊文字（`&`・`<`・`>`・`"`・`'`）を文字参照にする。XML 1.0 に書けない制御文字は
//!   カタログに保存する前に除いている（genzo-model の `normalize_metadata_value`）が、ここでも除く。

use genzo_model::{CopyrightStatus, IptcMetadata, MetadataField};

/// JPEG の APP1 に入れる XMP の識別子（`"http://ns.adobe.com/xap/1.0/\0"`。XMP の仕様書の第 3 部）。
pub const XMP_JPEG_SIGNATURE: &[u8; 29] = b"http://ns.adobe.com/xap/1.0/\0";

/// PNG の iTXt チャンクで XMP を入れるときのキーワード。
pub const XMP_PNG_KEYWORD: &str = "XML:com.adobe.xmp";

/// TIFF の XMP のタグ（XMLPacket。700 = 0x02BC）。
pub const TIFF_TAG_XMP: u16 = 700;

/// XML の文字データとして書ける文字か（XML 1.0 の Char から、ここで書かないものを除く）。
fn xml_char_ok(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || (!c.is_control() && !matches!(c, '\u{FFFE}' | '\u{FFFF}'))
}

/// XML の文字データ・属性の値として書けるように、特殊文字を文字参照にし、書けない文字を除く。
pub(crate) fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars().filter(|&c| xml_char_ok(c)) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // 改行コードを XML の処理系が \n に変えないよう、\r は文字参照にする。
            '\r' => out.push_str("&#xD;"),
            c => out.push(c),
        }
    }
    out
}

/// 単純な値のプロパティ（`<ns:Name>値</ns:Name>`）。
fn simple(out: &mut String, indent: &str, name: &str, value: &str) {
    out.push_str(&format!("{indent}<{name}>{}</{name}>\n", escape_xml(value)));
}

/// 言語の別の値（`rdf:Alt`。`x-default` の 1 つだけ）。
fn lang_alt(out: &mut String, indent: &str, name: &str, value: &str) {
    out.push_str(&format!(
        "{indent}<{name}>\n{indent} <rdf:Alt>\n{indent}  <rdf:li xml:lang=\"x-default\">{}</rdf:li>\n{indent} </rdf:Alt>\n{indent}</{name}>\n",
        escape_xml(value)
    ));
}

/// 順序付きの並び（`rdf:Seq`。値は 1 つ）。
fn seq(out: &mut String, indent: &str, name: &str, value: &str) {
    out.push_str(&format!(
        "{indent}<{name}>\n{indent} <rdf:Seq>\n{indent}  <rdf:li>{}</rdf:li>\n{indent} </rdf:Seq>\n{indent}</{name}>\n",
        escape_xml(value)
    ));
}

/// 作成者の連絡先（`Iptc4xmpCore:CreatorContactInfo` の項目と、その中のプロパティの名前）。
const CONTACT_FIELDS: [(MetadataField, &str); 8] = [
    (MetadataField::CreatorAddress, "Iptc4xmpCore:CiAdrExtadr"),
    (MetadataField::CreatorCity, "Iptc4xmpCore:CiAdrCity"),
    (MetadataField::CreatorRegion, "Iptc4xmpCore:CiAdrRegion"),
    (MetadataField::CreatorPostalCode, "Iptc4xmpCore:CiAdrPcode"),
    (MetadataField::CreatorCountry, "Iptc4xmpCore:CiAdrCtry"),
    (MetadataField::CreatorPhone, "Iptc4xmpCore:CiTelWork"),
    (MetadataField::CreatorEmail, "Iptc4xmpCore:CiEmailWork"),
    (MetadataField::CreatorUrl, "Iptc4xmpCore:CiUrlWork"),
];

/// 単純な値で書く項目と、そのプロパティの名前。
const SIMPLE_FIELDS: [(MetadataField, &str); 11] = [
    (MetadataField::Headline, "photoshop:Headline"),
    (MetadataField::CreatorJobTitle, "photoshop:AuthorsPosition"),
    (MetadataField::City, "photoshop:City"),
    (MetadataField::ProvinceState, "photoshop:State"),
    (MetadataField::Country, "photoshop:Country"),
    (MetadataField::Credit, "photoshop:Credit"),
    (MetadataField::Source, "photoshop:Source"),
    (MetadataField::Instructions, "photoshop:Instructions"),
    (MetadataField::Sublocation, "Iptc4xmpCore:Location"),
    (MetadataField::CountryCode, "Iptc4xmpCore:CountryCode"),
    (MetadataField::CopyrightUrl, "xmpRights:WebStatement"),
];

/// 順序のない並び（`rdf:Bag`）。
fn bag(out: &mut String, indent: &str, name: &str, values: &[String]) {
    out.push_str(&format!("{indent}<{name}>\n{indent} <rdf:Bag>\n"));
    for v in values {
        out.push_str(&format!("{indent}  <rdf:li>{}</rdf:li>\n", escape_xml(v)));
    }
    out.push_str(&format!("{indent} </rdf:Bag>\n{indent}</{name}>\n"));
}

/// 説明のメタデータとキーワードを XMP のパケットにする（UTF-8。値が 1 つもなければ `None`）。
///
/// `keywords` は書き出すキーワードの名前（空の名前・前後の空白だけの名前は書かない）。
pub fn xmp_packet(meta: &IptcMetadata, keywords: &[String]) -> Option<String> {
    let keywords: Vec<String> = keywords
        .iter()
        .map(|k| k.trim())
        .filter(|k| !k.is_empty())
        .map(str::to_owned)
        .collect();
    if meta.is_empty() && keywords.is_empty() {
        return None;
    }
    let ind = "   ";
    let mut body = String::new();
    if let Some(v) = meta.get(MetadataField::Title) {
        lang_alt(&mut body, ind, "dc:title", v);
    }
    if let Some(v) = meta.get(MetadataField::Caption) {
        lang_alt(&mut body, ind, "dc:description", v);
    }
    if let Some(v) = meta.get(MetadataField::Creator) {
        seq(&mut body, ind, "dc:creator", v);
    }
    if let Some(v) = meta.get(MetadataField::Copyright) {
        lang_alt(&mut body, ind, "dc:rights", v);
    }
    if !keywords.is_empty() {
        bag(&mut body, ind, "dc:subject", &keywords);
    }
    if let Some(v) = meta.get(MetadataField::UsageTerms) {
        lang_alt(&mut body, ind, "xmpRights:UsageTerms", v);
    }
    match meta.copyright_status() {
        Some(CopyrightStatus::Copyrighted) => simple(&mut body, ind, "xmpRights:Marked", "True"),
        Some(CopyrightStatus::PublicDomain) => simple(&mut body, ind, "xmpRights:Marked", "False"),
        None => {}
    }
    for (field, name) in SIMPLE_FIELDS {
        if let Some(v) = meta.get(field) {
            simple(&mut body, ind, name, v);
        }
    }
    let contact: Vec<(&str, &str)> = CONTACT_FIELDS
        .iter()
        .filter_map(|&(f, name)| meta.get(f).map(|v| (name, v)))
        .collect();
    if !contact.is_empty() {
        body.push_str(&format!(
            "{ind}<Iptc4xmpCore:CreatorContactInfo rdf:parseType=\"Resource\">\n"
        ));
        for (name, v) in contact {
            simple(&mut body, &format!("{ind} "), name, v);
        }
        body.push_str(&format!("{ind}</Iptc4xmpCore:CreatorContactInfo>\n"));
    }
    if body.is_empty() {
        // 著作権の状態が解釈できない値だけ、など。
        return None;
    }
    Some(format!(
        "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
         <rdf:Description rdf:about=\"\"\n    \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n    \
         xmlns:photoshop=\"http://ns.adobe.com/photoshop/1.0/\"\n    \
         xmlns:Iptc4xmpCore=\"http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/\"\n    \
         xmlns:xmpRights=\"http://ns.adobe.com/xap/1.0/rights/\">\n\
         {body}  </rdf:Description>\n \
         </rdf:RDF>\n\
         </x:xmpmeta>\n\
         <?xpacket end=\"r\"?>"
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// テスト用の簡易な XML の確かめ: 開きタグと閉じタグの対応（`<?`・`<!` は飛ばす）と、属性の引用符。
    pub(crate) fn assert_well_formed(xml: &str) {
        let mut stack: Vec<String> = Vec::new();
        let mut rest = xml;
        while let Some(i) = rest.find('<') {
            let after = &rest[i + 1..];
            let end = after.find('>').expect("タグが閉じている");
            let tag = &after[..end];
            rest = &after[end + 1..];
            if tag.starts_with('?') || tag.starts_with('!') {
                continue;
            }
            assert_eq!(tag.matches('"').count() % 2, 0, "属性の引用符: {tag}");
            if let Some(name) = tag.strip_prefix('/') {
                assert_eq!(stack.pop().as_deref(), Some(name.trim()), "閉じタグの対応");
            } else if !tag.ends_with('/') {
                let name = tag.split_whitespace().next().unwrap().to_owned();
                stack.push(name);
            }
        }
        assert!(stack.is_empty(), "閉じていないタグ: {stack:?}");
    }

    fn full() -> IptcMetadata {
        let mut m = IptcMetadata::default();
        for f in MetadataField::ALL {
            m.set(f, Some(format!("値 {f}")));
        }
        m.copyright_status = Some("copyrighted".to_owned());
        m
    }

    #[test]
    fn empty_metadata_has_no_packet() {
        assert_eq!(xmp_packet(&IptcMetadata::default(), &[]), None);
        // 解釈できない著作権の状態だけなら、書くものがない。
        let m = IptcMetadata {
            copyright_status: Some("bogus".to_owned()),
            ..IptcMetadata::default()
        };
        assert_eq!(xmp_packet(&m, &[]), None);
    }

    #[test]
    fn every_field_is_written_with_its_property() {
        let xmp = xmp_packet(&full(), &[]).unwrap();
        assert_well_formed(&xmp);
        assert!(xmp.starts_with("<?xpacket begin=\"\u{FEFF}\""));
        assert!(xmp.ends_with("<?xpacket end=\"r\"?>"));
        for (f, prop) in [
            (MetadataField::Title, "dc:title"),
            (MetadataField::Caption, "dc:description"),
            (MetadataField::Creator, "dc:creator"),
            (MetadataField::Copyright, "dc:rights"),
            (MetadataField::UsageTerms, "xmpRights:UsageTerms"),
        ] {
            assert!(xmp.contains(&format!("<{prop}>")), "{prop} がない: {xmp}");
            assert!(xmp.contains(&format!("値 {f}")), "{f}");
        }
        for (f, prop) in SIMPLE_FIELDS.iter().chain(CONTACT_FIELDS.iter()) {
            assert!(
                xmp.contains(&format!("<{prop}>値 {f}</{prop}>")),
                "{prop}: {xmp}"
            );
        }
        assert!(xmp.contains("<xmpRights:Marked>True</xmpRights:Marked>"));
        assert!(xmp.contains("rdf:parseType=\"Resource\""));
        // 言語の別の値は x-default。
        assert!(xmp.contains("<rdf:li xml:lang=\"x-default\">値 title</rdf:li>"));
        // すべての項目が書かれている（著作権の状態は Marked として）。
        for f in MetadataField::ALL {
            if f != MetadataField::CopyrightStatus {
                assert!(xmp.contains(&format!("値 {f}<")), "{f}");
            }
        }
    }

    #[test]
    fn special_characters_are_escaped() {
        let m = IptcMetadata {
            title: Some("A & B <C> \"D\" 'E'".to_owned()),
            caption: Some("一行目\r\n二行目\u{0}\u{1b}".to_owned()),
            copyright_status: Some("public_domain".to_owned()),
            ..IptcMetadata::default()
        };
        let xmp = xmp_packet(&m, &[]).unwrap();
        assert_well_formed(&xmp);
        assert!(xmp.contains("A &amp; B &lt;C&gt; &quot;D&quot; &apos;E&apos;"));
        assert!(xmp.contains("一行目&#xD;\n二行目<"), "{xmp}");
        assert!(!xmp.contains('\u{0}') && !xmp.contains('\u{1b}'));
        assert!(xmp.contains("<xmpRights:Marked>False</xmpRights:Marked>"));
        assert!(
            !xmp.contains("CreatorContactInfo"),
            "連絡先がなければ構造体も書かない"
        );
    }

    #[test]
    fn keywords_are_written_as_a_bag() {
        let keywords = vec![
            "京都".to_owned(),
            "A & B".to_owned(),
            "  ".to_owned(),
            "場所".to_owned(),
        ];
        // 説明のメタデータがなくても、キーワードだけで書く。
        let xmp = xmp_packet(&IptcMetadata::default(), &keywords).unwrap();
        assert_well_formed(&xmp);
        assert!(xmp.contains("<dc:subject>\n    <rdf:Bag>\n"), "{xmp}");
        assert!(xmp.contains("<rdf:li>京都</rdf:li>"));
        assert!(xmp.contains("<rdf:li>A &amp; B</rdf:li>"));
        assert!(xmp.contains("<rdf:li>場所</rdf:li>"));
        assert_eq!(xmp.matches("<rdf:li>").count(), 3, "空の名前は書かない");
        // 空のキーワードだけなら書かない。
        assert_eq!(xmp_packet(&IptcMetadata::default(), &["".to_owned()]), None);
        // 説明のメタデータと一緒に書く。
        let xmp = xmp_packet(&full(), &keywords).unwrap();
        assert_well_formed(&xmp);
        assert!(xmp.contains("<dc:subject>") && xmp.contains("<dc:title>"));
    }

    #[test]
    fn well_formedness_check_catches_errors() {
        let bad = std::panic::catch_unwind(|| assert_well_formed("<a><b></a>"));
        assert!(bad.is_err());
        assert_well_formed("<?x?><a b=\"1\"><c/></a>");
    }
}
