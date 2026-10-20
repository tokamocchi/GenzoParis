//! 地図のタイルと地名の検索の提供元（URL のひな形と、その検査）。
//!
//! 既定は OpenStreetMap Foundation のサーバー（タイルは `tile.openstreetmap.org`、検索は
//! `nominatim.openstreetmap.org`）。ポリシーが「ソフトウェアの更新なしに切り替えられること」を求める
//! （Nominatim は must、タイルは should）ので、genzo-app の設定（settings.json）で URL を変えられる。
//! 設定の URL は [`validate_tile_url_template`]・[`validate_search_url`] で確かめる（https だけ）。

use crate::GeoError;

/// OSM の標準のタイルの URL のひな形（ポリシーの原文が示す URL。`a.`・`b.`・`c.` のサブドメインは使わない）。
pub const OSM_TILE_URL: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";

/// OSM の標準のタイルの最大のズーム（仮置き。ポリシーの原文には数値がない）。
pub const OSM_MAX_ZOOM: u8 = 19;

/// OSM のデータの帰属の表示（openstreetmap.org の `/copyright` のページの元の文言）。地図の右下に常に出す
/// （画面の部品が出す。文言と出し方は人の確認待ち）。
pub const OSM_ATTRIBUTION: &str = "© OpenStreetMap contributors";

/// Nominatim の検索の URL（`search.php` は古い形なので使わない）。
pub const NOMINATIM_SEARCH_URL: &str = "https://nominatim.openstreetmap.org/search";

/// 設定で指定できる最大のズームの上限（タイルの番号 2^z が u32 に収まる範囲の、ゆとりのある上限。仮置き）。
pub const MAX_ZOOM_LIMIT: u8 = 22;

/// 地図のタイルの提供元。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileProvider {
    url_template: String,
    max_zoom: u8,
    attribution: String,
}

impl TileProvider {
    /// OSM の標準のタイル（[`OSM_TILE_URL`]・[`OSM_MAX_ZOOM`]・[`OSM_ATTRIBUTION`]）。
    pub fn openstreetmap() -> Self {
        Self {
            url_template: OSM_TILE_URL.to_string(),
            max_zoom: OSM_MAX_ZOOM,
            attribution: OSM_ATTRIBUTION.to_string(),
        }
    }

    /// 設定の URL のひな形から作る（[`validate_tile_url_template`] で確かめる）。`max_zoom` は
    /// [`MAX_ZOOM_LIMIT`] まで。`attribution` は前後の空白を除いて空でないこと。
    pub fn new(url_template: &str, max_zoom: u8, attribution: &str) -> Result<Self, GeoError> {
        validate_tile_url_template(url_template)?;
        if max_zoom > MAX_ZOOM_LIMIT {
            return Err(GeoError::InvalidInput(format!(
                "最大のズームは {MAX_ZOOM_LIMIT} までです: {max_zoom}"
            )));
        }
        let attribution = attribution.trim();
        if attribution.is_empty() {
            return Err(GeoError::InvalidInput(
                "地図の帰属の表示が空です".to_string(),
            ));
        }
        Ok(Self {
            url_template: url_template.to_string(),
            max_zoom,
            attribution: attribution.to_string(),
        })
    }

    /// テスト用: 検査をせずに作る（手元の http のサーバーの URL を使うため）。
    #[cfg(test)]
    pub(crate) fn for_loopback_test(url_template: &str, max_zoom: u8) -> Self {
        Self {
            url_template: url_template.to_string(),
            max_zoom,
            attribution: OSM_ATTRIBUTION.to_string(),
        }
    }

    /// URL のひな形（`{z}`・`{x}`・`{y}` を含む）。
    pub fn url_template(&self) -> &str {
        &self.url_template
    }

    /// 最大のズーム。
    pub fn max_zoom(&self) -> u8 {
        self.max_zoom
    }

    /// 帰属の表示。
    pub fn attribution(&self) -> &str {
        &self.attribution
    }

    /// タイルの URL（範囲は確かめない。呼び出し元が確かめる）。
    pub fn tile_url(&self, z: u8, x: u32, y: u32) -> String {
        self.url_template
            .replace("{z}", &z.to_string())
            .replace("{x}", &x.to_string())
            .replace("{y}", &y.to_string())
    }
}

/// 地名の検索（Nominatim の API と同じ形の `/search`）の提供元。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeocoderProvider {
    search_url: String,
}

impl GeocoderProvider {
    /// OSM Foundation の Nominatim（[`NOMINATIM_SEARCH_URL`]）。
    pub fn nominatim() -> Self {
        Self {
            search_url: NOMINATIM_SEARCH_URL.to_string(),
        }
    }

    /// 設定の URL から作る（[`validate_search_url`] で確かめる）。問い合わせ（`?key=…` など）を含んでよい。
    pub fn new(search_url: &str) -> Result<Self, GeoError> {
        validate_search_url(search_url)?;
        Ok(Self {
            search_url: search_url.to_string(),
        })
    }

    /// テスト用: 検査をせずに作る（手元の http のサーバーの URL を使うため）。
    #[cfg(test)]
    pub(crate) fn for_loopback_test(search_url: &str) -> Self {
        Self {
            search_url: search_url.to_string(),
        }
    }

    /// 検索の URL。
    pub fn search_url(&self) -> &str {
        &self.search_url
    }

    /// 検索の URL に問い合わせのパラメータ（エンコード済み）を足した URL。
    pub(crate) fn url_with_query(&self, query: &str) -> String {
        join_query(&self.search_url, query)
    }
}

/// `base` に問い合わせのパラメータ（エンコード済み）を足す。
fn join_query(base: &str, query: &str) -> String {
    let sep = if base.ends_with('?') || base.ends_with('&') {
        ""
    } else if base.contains('?') {
        "&"
    } else {
        "?"
    };
    format!("{base}{sep}{query}")
}

/// タイルの URL のひな形を確かめる。
///
/// - `https://` で始まり、ホスト名がある（利用者名・パスワードは不可）。空白・制御文字・ASCII でない文字・`#` は不可。
/// - `{z}`・`{x}`・`{y}` をちょうど 1 つずつ含む。ほかの置換子（サブドメインの `{s}` など）は不可。
/// - 置換子を数に置き換えた URL が、要求に使う http の URI として読める（パスの `<`・`>`・`` ` `` や、問い合わせの
///   `"`・`<`・`>` は不可。長すぎる URL も不可）。読めない URL を保存すると、要求のたびに「URL を読めません」に
///   なるため。
/// - パスの拡張子が PNG でない画像・ベクトルのタイルのもの（`.jpg`・`.jpeg`・`.webp`・`.gif`・`.avif`・`.pbf`・
///   `.mvt`）は不可（扱うのは PNG のタイルだけ。保存した後にすべてのタイルが 502 になるのを避ける目安。拡張子の
///   ない URL は、応答の `Content-Type` と署名で確かめる）。
pub fn validate_tile_url_template(template: &str) -> Result<(), GeoError> {
    check_https_url(template, "タイルの URL")?;
    let mut rest = template.to_string();
    for placeholder in ["{z}", "{x}", "{y}"] {
        if template.matches(placeholder).count() != 1 {
            return Err(GeoError::InvalidInput(
                "タイルの URL には {z}・{x}・{y} を 1 つずつ入れてください".to_string(),
            ));
        }
        rest = rest.replace(placeholder, "");
    }
    if rest.contains(['{', '}']) {
        return Err(GeoError::InvalidInput(
            "タイルの URL に使えない置換子があります（使えるのは {z}・{x}・{y} だけ。サブドメインの {s} は使えません）"
                .to_string(),
        ));
    }
    let path = template.split(['?', '#']).next().unwrap_or("");
    let extension = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase());
    if let Some(ext) = extension
        && NOT_PNG_EXTENSIONS.contains(&ext.as_str())
    {
        return Err(GeoError::InvalidInput(format!(
            "タイルの URL: PNG のタイルだけを使えます（.{ext} のタイルは使えません）"
        )));
    }
    // 実際に送る形（番号は桁のいちばん多いもの）で確かめる。
    let max = ((1u32 << MAX_ZOOM_LIMIT) - 1).to_string();
    let sample = template
        .replace("{z}", &MAX_ZOOM_LIMIT.to_string())
        .replace("{x}", &max)
        .replace("{y}", &max);
    check_request_uri(&sample, "タイルの URL")
}

/// PNG でないタイルの拡張子（[`validate_tile_url_template`]）。
const NOT_PNG_EXTENSIONS: [&str; 7] = ["jpg", "jpeg", "webp", "gif", "avif", "pbf", "mvt"];

/// 検索の URL を確かめる（`https://` で始まり、ホスト名がある。置換子 `{…}` は不可。検索のパラメータを足した
/// URL が http の URI として読める）。
pub fn validate_search_url(url: &str) -> Result<(), GeoError> {
    check_https_url(url, "検索の URL")?;
    if url.contains(['{', '}']) {
        return Err(GeoError::InvalidInput(
            "検索の URL に { } は使えません".to_string(),
        ));
    }
    // 実際に送る形（genzo-geo が後ろに足す問い合わせの例）で確かめる。
    check_request_uri(
        &join_query(url, "q=x&format=jsonv2&limit=10&accept-language=ja"),
        "検索の URL",
    )
}

/// 要求に使う http の URI として読めるか（ureq が使う http の crate の検査。文字の表を自前で持たない）。
fn check_request_uri(url: &str, what: &str) -> Result<(), GeoError> {
    match ureq::http::Uri::try_from(url) {
        Ok(_) => Ok(()),
        Err(_) => Err(GeoError::InvalidInput(format!(
            "{what}: 使えない文字があるか、長すぎます（\"・<・>・` などは %XX の形で書いてください）"
        ))),
    }
}

/// https の URL の形を確かめる（`url` crate を使わない最小の検査。読めない URL は要求のときに ureq が誤りにする）。
fn check_https_url(url: &str, what: &str) -> Result<(), GeoError> {
    let invalid = |reason: &str| Err(GeoError::InvalidInput(format!("{what}: {reason}")));
    if url.is_empty() {
        return invalid("空です");
    }
    if !url.chars().all(|c| c.is_ascii_graphic()) {
        return invalid(
            "使えない文字があります（空白・制御文字・全角の文字など。必要なら %XX の形で書いてください）",
        );
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return invalid("https:// で始まる URL を指定してください");
    };
    if url.contains('#') {
        return invalid("# は使えません");
    }
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.contains('@') {
        return invalid("利用者名・パスワードは入れられません");
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let Some(close) = v6.find(']') else {
            return invalid("ホスト名を読めません");
        };
        let host = &v6[..close];
        if host.is_empty()
            || !host
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        {
            return invalid("ホスト名を読めません");
        }
        (host, v6[close + 1..].strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return invalid("ホスト名がありません");
    }
    if !authority.starts_with('[')
        && !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        return invalid("ホスト名に使えない文字があります");
    }
    if let Some(port) = port {
        let ok = !port.is_empty()
            && port.chars().all(|c| c.is_ascii_digit())
            && port.parse::<u16>().is_ok_and(|p| p > 0);
        if !ok {
            return invalid("ポートの番号を読めません");
        }
    } else if authority.starts_with('[') && !authority.ends_with(']') {
        return invalid("ホスト名を読めません");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openstreetmap_defaults_follow_the_policy() {
        let p = TileProvider::openstreetmap();
        assert_eq!(
            p.url_template(),
            "https://tile.openstreetmap.org/{z}/{x}/{y}.png"
        );
        assert_eq!(p.max_zoom(), 19);
        assert_eq!(p.attribution(), "© OpenStreetMap contributors");
        assert_eq!(
            p.tile_url(12, 3638, 1612),
            "https://tile.openstreetmap.org/12/3638/1612.png"
        );
        validate_tile_url_template(OSM_TILE_URL).unwrap();
        let g = GeocoderProvider::nominatim();
        assert_eq!(g.search_url(), "https://nominatim.openstreetmap.org/search");
        validate_search_url(NOMINATIM_SEARCH_URL).unwrap();
    }

    #[test]
    fn tile_templates_are_checked() {
        for ok in [
            "https://tile.openstreetmap.org/{z}/{x}/{y}.png",
            "https://tiles.example.org:8443/osm/{z}/{x}/{y}.png?key=abc",
            "https://example.org/{y}/{x}/{z}",
            "https://[2001:db8::1]:443/{z}/{x}/{y}.png",
            "https://[2001:db8::1]/{z}/{x}/{y}.png",
            // http の URI として読める記号（問い合わせの ` と、パスの "）。
            "https://tiles.example.org/{z}/{x}/{y}.png?a=`b`",
            "https://tiles.example.org/\"a\"/{z}/{x}/{y}.png",
        ] {
            assert!(validate_tile_url_template(ok).is_ok(), "{ok}");
            assert!(TileProvider::new(ok, 19, "© OpenStreetMap contributors").is_ok());
        }
        for ng in [
            "",
            "http://tile.openstreetmap.org/{z}/{x}/{y}.png",
            "HTTPS://tile.openstreetmap.org/{z}/{x}/{y}.png",
            "file:///tmp/{z}/{x}/{y}.png",
            "https:///{z}/{x}/{y}.png",
            "https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png",
            "https://tile.openstreetmap.org/{z}/{x}.png",
            "https://tile.openstreetmap.org/{z}/{x}/{y}/{y}.png",
            "https://tile.openstreetmap.org/{z}/{x}/{y}{r}.png",
            "https://user:pass@tile.example.org/{z}/{x}/{y}.png",
            "https://tile.example.org:0/{z}/{x}/{y}.png",
            "https://tile.example.org:99999/{z}/{x}/{y}.png",
            "https://tile.example.org:/{z}/{x}/{y}.png",
            "https://tile example.org/{z}/{x}/{y}.png",
            " https://tile.example.org/{z}/{x}/{y}.png",
            "https://タイル.example.org/{z}/{x}/{y}.png",
            "https://tile.example.org/{z}/{x}/{y}.png#frag",
            "https://tile_example.org/{z}/{x}/{y}.png",
            "https://[2001:db8::1/{z}/{x}/{y}.png",
            "https://[]/{z}/{x}/{y}.png",
            // http の URI として読めない記号（要求のたびに「URL を読めません」になり、タイルがすべて 400 になる）。
            "https://tiles.example.org/{z}/{x}/{y}.png?style=\"dark\"",
            "https://tiles.example.org/{z}/{x}/{y}.png?a=<b>",
            "https://tiles.example.org/<a>/{z}/{x}/{y}.png",
            "https://tiles.example.org/`a`/{z}/{x}/{y}.png",
        ] {
            assert!(
                matches!(
                    validate_tile_url_template(ng),
                    Err(GeoError::InvalidInput(_))
                ),
                "{ng}"
            );
            assert!(TileProvider::new(ng, 19, "x").is_err(), "{ng}");
        }
        // 長すぎる URL（http の URI のパスと問い合わせの上限を超える）。
        let long = format!(
            "https://tiles.example.org/{}/{{z}}/{{x}}/{{y}}.png",
            "a".repeat(70_000)
        );
        assert!(matches!(
            validate_tile_url_template(&long),
            Err(GeoError::InvalidInput(_))
        ));
    }

    #[test]
    fn tile_templates_for_other_image_kinds_are_rejected() {
        // 扱うのは PNG のタイルだけ（応答の Content-Type と署名で確かめる）。拡張子でわかる別の種類は、保存の
        // ときに止める（保存した後にすべてのタイルが 502 になり、原因がわからないため）。
        for ng in [
            "https://tiles.example.org/{z}/{x}/{y}.jpg",
            "https://tiles.example.org/{z}/{x}/{y}.JPEG?key=abc",
            "https://tiles.example.org/{z}/{x}/{y}.webp",
            "https://tiles.example.org/{z}/{x}/{y}.pbf",
            "https://tiles.example.org/{z}/{x}/{y}.mvt",
        ] {
            let e = validate_tile_url_template(ng).unwrap_err();
            assert!(e.to_string().contains("PNG"), "{ng}: {e}");
        }
        for ok in [
            "https://tiles.example.org/{z}/{x}/{y}.png",
            "https://tiles.example.org/{z}/{x}/{y}.PNG",
            "https://tiles.example.org/{z}/{x}/{y}",
            "https://tiles.example.org/{z}/{x}/{y}@2x.png",
            "https://tiles.example.org/jpg/{z}/{x}/{y}.png?format=png",
        ] {
            assert!(validate_tile_url_template(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn provider_options_are_checked() {
        assert!(TileProvider::new(OSM_TILE_URL, 22, "x").is_ok());
        assert!(matches!(
            TileProvider::new(OSM_TILE_URL, 23, "x"),
            Err(GeoError::InvalidInput(_))
        ));
        assert!(matches!(
            TileProvider::new(OSM_TILE_URL, 19, "  "),
            Err(GeoError::InvalidInput(_))
        ));
        let p = TileProvider::new(OSM_TILE_URL, 19, "  © 提供元  ").unwrap();
        assert_eq!(p.attribution(), "© 提供元");
    }

    #[test]
    fn search_urls_are_checked() {
        for ok in [
            "https://nominatim.openstreetmap.org/search",
            "https://geocoder.example.org/v1/search?key=abc",
            "https://localhost:8080/search",
        ] {
            assert!(validate_search_url(ok).is_ok(), "{ok}");
            assert!(GeocoderProvider::new(ok).is_ok());
        }
        for ng in [
            "",
            "http://nominatim.openstreetmap.org/search",
            "https://",
            "https://nominatim.openstreetmap.org/search?q={q}",
            "https://nominatim.openstreetmap.org/search#x",
            "ftp://nominatim.openstreetmap.org/search",
            "https://nominatim.example.org/search?x=\"a\"",
            "https://nominatim.example.org/<search>",
        ] {
            assert!(
                matches!(validate_search_url(ng), Err(GeoError::InvalidInput(_))),
                "{ng}"
            );
        }
    }

    #[test]
    fn query_parameters_are_joined_to_the_search_url() {
        let p = GeocoderProvider::nominatim();
        assert_eq!(
            p.url_with_query("q=a&format=jsonv2"),
            "https://nominatim.openstreetmap.org/search?q=a&format=jsonv2"
        );
        let p = GeocoderProvider::new("https://example.org/search?key=k").unwrap();
        assert_eq!(
            p.url_with_query("q=a"),
            "https://example.org/search?key=k&q=a"
        );
        let p = GeocoderProvider::new("https://example.org/search?").unwrap();
        assert_eq!(p.url_with_query("q=a"), "https://example.org/search?q=a");
    }
}
