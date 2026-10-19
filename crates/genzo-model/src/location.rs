//! 座標の文字列の解釈と書式（位置情報の後付け。LIB-21）。
//!
//! 利用者が貼り付けた座標の文字列を [`GpsCoord`]（WGS 84、10 進数の度）にする（[`parse_coordinates`]）。
//! 画面（`ui/src/library/geotagLogic.ts` の `parseCoordinates`）も同じ形を同じ規則で受け付ける。
//! 規則を変えるときは両方を直し、テストのケースもそろえる。
//!
//! 受け付ける形（例は東京駅の付近）:
//!
//! 1. 10 進の度: `35.681236, 139.767125`、`-33.8568 151.2153`。区切りはカンマ・セミコロン・読点「、」
//!    （前後の空白は任意）か、空白だけ。符号（`+`・`-`）と度の記号（`35.681236°`）は任意。小数の桁は制限しない。
//! 2. 方角の記号: `35.681236N 139.767125E`、`N35.681236 E139.767125`、`35.681236° N, 139.767125° E`
//!    （小文字も）。S・W は負。経度の記号が先なら入れ替える（`139.767125E 35.681236N`）。
//! 3. 日本語: `北緯 35.681236°、東経 139.767125°`（画面の `formatGps` の出力）。南緯・西経は負。
//! 4. 度分秒: `35°40'52.4"N 139°46'01.7"E`、`35°40′52.4″N 139°46′1.7″E`、度分 `35°40.873'N`。
//!    記号のない度分秒（`35 40 52.4 N 139 46 1.7 E`）は方角の記号があるときだけ。度は整数、分は 0 以上
//!    60 未満（秒があれば整数）、秒は 0 以上 60 未満。
//! 5. Google マップの URL（`http://`・`https://` で始まるもの）: クエリの `q=緯度,経度`、`ll=緯度,経度`、
//!    パスの `@緯度,経度`（`https://www.google.com/maps/@35.681236,139.767125,17z`）の順に探す。
//!    `q`・`ll` の値は `%XX`（`%2C` はカンマ）と `+`（空白）を戻してから読む。`@` の値は表示している
//!    地図の中心と考えられ、ピンの位置とずれることがある（Google の資料では確かめていない）。
//!
//! 読む前に、全角の数字・「，」「．」「－」「＋」・全角の空白を半角にし、前後の空白を除く。
//!
//! 細かい規則:
//!
//! - 方角の記号のない 2 つの数は、緯度・経度の順（Google マップで地点を右クリックしてコピーできる形）。
//!   区切りの記号があるときに片方だけに記号があれば、もう片方は残りの軸。
//! - 区切りの記号（カンマ・セミコロン・読点）がないとき:
//!   - 方角の記号を使うなら、両方の数に、同じ側（両方とも数の前か、両方とも後）に付ける（`35.68N 139.76E`。
//!     `35.68 139.76E` は読まない）。記号の位置で 2 つに分けるので、間の空白はなくてよい（`35.68N139.76E`）。
//!   - 方角の記号がなければ、2 つの数（度分秒の記号付きを含む）の間に空白が要る（`35.68-139.76` は読まない）。
//! - 符号と方角の記号は、同じ数には一緒に使えない（`-35N`）。緯度どうし・経度どうし（`35N 139N`）は矛盾。
//! - 緯度は -90〜90、経度は -180〜180（両端を含む）。NaN・無限は不可。
//! - 誤りの順: 形（[`CoordParseError::Unreadable`] など）→ 符号と方角の記号・方角の矛盾 → 度分秒の値 →
//!   範囲（緯度が先）。

use crate::metadata::GpsCoord;

/// 座標の文字列を解釈できないときのエラー（[`parse_coordinates`]）。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CoordParseError {
    /// 空（空白だけ）。
    #[error("座標が空です")]
    Empty,
    /// 座標の形として読めない。
    #[error("座標として読めません（例: 35.681236, 139.767125）")]
    Unreadable,
    /// URL に座標が見つからない。
    #[error(
        "URL から座標を読めません（Google マップの URL の @緯度,経度・q=緯度,経度・ll=緯度,経度 を読みます）"
    )]
    NoCoordinatesInUrl,
    /// 度分秒の値が正しくない（度・分が整数でない、分・秒が 0 以上 60 未満でない）。
    #[error(
        "度分秒の値が正しくありません（度は整数、分・秒は 0 以上 60 未満。秒があれば分も整数）"
    )]
    InvalidDms,
    /// 符号と方角の記号を一緒に使った。
    #[error("符号（+・-）と方角の記号（N・S・E・W・北緯など）は一緒に使えません")]
    SignWithDirection,
    /// 方角の記号が矛盾している（緯度どうし・経度どうし）。
    #[error("方角の記号が矛盾しています（緯度（N・S）と経度（E・W）を 1 つずつ付けてください）")]
    ConflictingDirections,
    /// 緯度が範囲外（-90〜90）。値は読んだ値。
    #[error("緯度が範囲外です（-90〜90）: {0}")]
    LatitudeOutOfRange(f64),
    /// 経度が範囲外（-180〜180）。値は読んだ値。
    #[error("経度が範囲外です（-180〜180）: {0}")]
    LongitudeOutOfRange(f64),
}

/// 座標の文字列を解釈する（受け付ける形はモジュールの説明）。
///
/// 返す座標は [`GpsCoord::is_valid`] を満たす。`-0` は `0` にする。
pub fn parse_coordinates(text: &str) -> Result<GpsCoord, CoordParseError> {
    let normalized = normalize(text);
    let s = normalized.trim();
    if s.is_empty() {
        return Err(CoordParseError::Empty);
    }
    let (lat, lon) = if is_url(s) {
        parse_url(s)?
    } else {
        parse_text(s)?
    };
    if !(lat.is_finite() && (-90.0..=90.0).contains(&lat)) {
        return Err(CoordParseError::LatitudeOutOfRange(lat));
    }
    if !(lon.is_finite() && (-180.0..=180.0).contains(&lon)) {
        return Err(CoordParseError::LongitudeOutOfRange(lon));
    }
    // -0.0 + 0.0 は 0.0。
    Ok(GpsCoord {
        lat: lat + 0.0,
        lon: lon + 0.0,
    })
}

/// 座標を `"35.681236, 139.767125"` の形（緯度, 経度。10 進の度、小数 6 桁。約 0.1 m）にする。
///
/// [`parse_coordinates`] で読み直せる。画面の `gpsDecimal`（`ui/src/library/metadataLogic.ts`）と同じ文字列に
/// なる（丸めも JavaScript の `toFixed(6)` と同じ）。範囲の検査はしない。
pub fn format_coordinates(g: GpsCoord) -> String {
    format!("{}, {}", fixed6(g.lat), fixed6(g.lon))
}

/// 小数 6 桁の 10 進の文字列（JavaScript の `Number.prototype.toFixed(6)` と同じ丸め）。
///
/// Rust の書式はちょうど中間の値を偶数の側へ、`toFixed` は絶対値の大きい側へ丸める。ちょうど中間になるのは
/// `v × 128` が奇数の整数のとき（`v × 10^6` の小数部が 0.5 ⇔ `v` = 奇数 / 2^7。10^6 = 2^6 × 5^6 のため）だけ
/// なので、そのときは絶対値を 1 ulp 大きくしてから書く（|v| が 10^6 未満なら 1 ulp は 10^-6 よりずっと小さい）。
/// `-0.0` は `toFixed` と同じく `0.000000` と書く。
fn fixed6(v: f64) -> String {
    let v = v + 0.0;
    let scaled = v * 128.0;
    let tie = v.abs() < 1.0e6 && scaled.fract() == 0.0 && scaled.rem_euclid(2.0) == 1.0;
    let v = if tie {
        f64::from_bits(v.to_bits() + 1)
    } else {
        v
    };
    format!("{v:.6}")
}

/// 全角の数字・「，」「．」「－」「＋」・全角の空白を半角にする。
fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '０'..='９' => char::from(b'0' + (u32::from(c) - u32::from('０')) as u8),
            '，' => ',',
            '．' => '.',
            '－' => '-',
            '＋' => '+',
            '\u{3000}' => ' ',
            c => c,
        })
        .collect()
}

/// `http://`・`https://` で始まるか（大文字・小文字は問わない）。
fn is_url(s: &str) -> bool {
    ["http://", "https://"].iter().any(|p| {
        s.as_bytes()
            .get(..p.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(p.as_bytes()))
    })
}

/// URL の `q=`・`ll=`・`@` から（緯度, 経度）を読む。範囲は検査しない。
fn parse_url(url: &str) -> Result<(f64, f64), CoordParseError> {
    let url = url.split_once('#').map_or(url, |(u, _)| u);
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    for key in ["q", "ll"] {
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            if k != key {
                continue;
            }
            let found = percent_decode(v).and_then(|v| match decimal_pair_prefix(v.trim()) {
                Some((c, rest)) if rest.trim().is_empty() => Some(c),
                _ => None,
            });
            if let Some(c) = found {
                return Ok(c);
            }
        }
    }
    // パスの `@緯度,経度`（後ろに `,17z` などが続く）。
    for (i, _) in path.match_indices('@') {
        if let Some((c, rest)) = decimal_pair_prefix(&path[i + 1..])
            && (rest.is_empty() || rest.starts_with([',', '/']))
        {
            return Ok(c);
        }
    }
    Err(CoordParseError::NoCoordinatesInUrl)
}

/// URL のクエリの値の `%XX` と `+`（空白）を戻す。`%` の後ろが 16 進の 2 桁でない・UTF-8 でなければ `None`。
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hex = b.get(i + 1..i + 3)?;
                if !hex.iter().all(u8::is_ascii_hexdigit) {
                    return None;
                }
                out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
                i += 3;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// 先頭の `緯度,経度`（10 進の度。符号は任意、カンマの前後の空白は任意）を読み、値と残りを返す。
fn decimal_pair_prefix(s: &str) -> Option<((f64, f64), &str)> {
    let a_end = number_end(s, 0)?;
    let rest = s[a_end..].trim_start().strip_prefix(',')?.trim_start();
    let b_end = number_end(rest, 0)?;
    let a = s[..a_end].parse().ok()?;
    let b = rest[..b_end].parse().ok()?;
    Some(((a, b), &rest[b_end..]))
}

/// `start` から始まる数（`[+-]?[0-9]+(\.[0-9]+)?`）の終わりの位置。数でなければ `None`。
///
/// `35.`・`.5` のような、小数点の片側に数字のない形は読まない。
fn number_end(s: &str, start: usize) -> Option<usize> {
    let b = s.as_bytes();
    let digits = |from: usize| {
        b.get(from..)
            .map_or(0, |t| t.iter().take_while(|c| c.is_ascii_digit()).count())
    };
    let mut i = start;
    if matches!(b.get(i), Some(b'+' | b'-')) {
        i += 1;
    }
    let n = digits(i);
    if n == 0 {
        return None;
    }
    i += n;
    if b.get(i) == Some(&b'.') {
        let m = digits(i + 1);
        if m == 0 {
            return None;
        }
        i += 1 + m;
    }
    Some(i)
}

/// 緯度か経度か。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Lat,
    Lon,
}

/// 方角の記号（N・S・E・W・北緯・南緯・東経・西経）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Direction {
    axis: Axis,
    /// S・W・南緯・西経（負の側）。
    negative: bool,
}

/// 数（符号を含む文字列）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Number<'a> {
    text: &'a str,
}

impl Number<'_> {
    fn signed(self) -> bool {
        self.text.starts_with(['+', '-'])
    }

    fn negative(self) -> bool {
        self.text.starts_with('-')
    }

    fn is_integer(self) -> bool {
        !self.text.contains('.')
    }

    /// 符号を除いた値。桁が多すぎれば無限（範囲の検査で落とす）。
    fn magnitude(self) -> f64 {
        self.text
            .trim_start_matches(['+', '-'])
            .parse()
            .unwrap_or(f64::INFINITY)
    }
}

/// 字句の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind<'a> {
    Num(Number<'a>),
    /// `°`。
    Deg,
    /// `'`・`′`。
    Min,
    /// `"`・`″`。
    Sec,
    Dir(Direction),
    /// `,`・`;`・`、`。
    Sep,
}

/// 字句。
#[derive(Debug, Clone, Copy)]
struct Token<'a> {
    kind: Kind<'a>,
    /// 直前に空白があったか。
    space_before: bool,
}

/// 角度（符号を含む最初の数と、分・秒）。
#[derive(Debug, Clone, Copy)]
enum Angle<'a> {
    Degrees(Number<'a>),
    DegMin(Number<'a>, Number<'a>),
    DegMinSec(Number<'a>, Number<'a>, Number<'a>),
}

/// 緯度か経度の片方（方角の記号と角度）。
#[derive(Debug, Clone, Copy)]
struct Part<'a> {
    dir: Option<Direction>,
    angle: Angle<'a>,
}

impl Part<'_> {
    fn first_number(&self) -> Number<'_> {
        match self.angle {
            Angle::Degrees(d) | Angle::DegMin(d, _) | Angle::DegMinSec(d, _, _) => d,
        }
    }

    /// 符号付きの値（度）。
    fn value(&self) -> Result<f64, CoordParseError> {
        let in_minute = |n: Number| (0.0..60.0).contains(&n.magnitude());
        let magnitude = match self.angle {
            Angle::Degrees(d) => d.magnitude(),
            Angle::DegMin(d, m) => {
                if !(d.is_integer() && in_minute(m)) {
                    return Err(CoordParseError::InvalidDms);
                }
                d.magnitude() + m.magnitude() / 60.0
            }
            Angle::DegMinSec(d, m, s) => {
                if !(d.is_integer() && m.is_integer() && in_minute(m) && in_minute(s)) {
                    return Err(CoordParseError::InvalidDms);
                }
                d.magnitude() + m.magnitude() / 60.0 + s.magnitude() / 3600.0
            }
        };
        let negative = self.first_number().negative() || self.dir.is_some_and(|d| d.negative);
        Ok(if negative { -magnitude } else { magnitude })
    }
}

/// 字句に分ける。知らない文字があれば [`CoordParseError::Unreadable`]。
fn tokenize(s: &str) -> Result<Vec<Token<'_>>, CoordParseError> {
    let dir = |axis, negative| Kind::Dir(Direction { axis, negative });
    let mut tokens = Vec::new();
    let mut space_before = false;
    let mut pos = 0;
    while let Some(c) = s[pos..].chars().next() {
        let start = pos;
        pos += c.len_utf8();
        let kind = match c {
            c if c.is_whitespace() => {
                space_before = true;
                continue;
            }
            ',' | ';' | '、' => Kind::Sep,
            '°' => Kind::Deg,
            '\'' | '′' => Kind::Min,
            '"' | '″' => Kind::Sec,
            'N' | 'n' => dir(Axis::Lat, false),
            'S' | 's' => dir(Axis::Lat, true),
            'E' | 'e' => dir(Axis::Lon, false),
            'W' | 'w' => dir(Axis::Lon, true),
            '北' | '南' | '東' | '西' => {
                let (axis, second) = if matches!(c, '北' | '南') {
                    (Axis::Lat, '緯')
                } else {
                    (Axis::Lon, '経')
                };
                if !s[pos..].starts_with(second) {
                    return Err(CoordParseError::Unreadable);
                }
                pos += second.len_utf8();
                dir(axis, matches!(c, '南' | '西'))
            }
            '+' | '-' | '0'..='9' => {
                let end = number_end(s, start).ok_or(CoordParseError::Unreadable)?;
                pos = end;
                Kind::Num(Number {
                    text: &s[start..end],
                })
            }
            _ => return Err(CoordParseError::Unreadable),
        };
        tokens.push(Token { kind, space_before });
        space_before = false;
    }
    Ok(tokens)
}

/// 角度の字句の並び（全体）を読む。記号のない度分・度分秒は `has_dir` のときだけ。
/// 2 つ目以降の数に符号があれば読まない。
fn angle<'a>(tokens: &[Token<'a>], has_dir: bool) -> Option<Angle<'a>> {
    use Kind::{Deg, Min, Num, Sec};
    let kinds: Vec<Kind<'a>> = tokens.iter().map(|t| t.kind).collect();
    let angle = match kinds[..] {
        [Num(d)] | [Num(d), Deg] => Angle::Degrees(d),
        [Num(d), Deg, Num(m), Min] => Angle::DegMin(d, m),
        [Num(d), Deg, Num(m), Min, Num(s), Sec] => Angle::DegMinSec(d, m, s),
        [Num(d), Num(m)] if has_dir => Angle::DegMin(d, m),
        [Num(d), Num(m), Num(s)] if has_dir => Angle::DegMinSec(d, m, s),
        _ => return None,
    };
    let later_signed = match angle {
        Angle::Degrees(_) => false,
        Angle::DegMin(_, m) => m.signed(),
        Angle::DegMinSec(_, m, s) => m.signed() || s.signed(),
    };
    (!later_signed).then_some(angle)
}

/// 緯度か経度の片方の字句の並び（全体）を読む。方角の記号は数の前か後に 1 つまで。
fn part<'a>(tokens: &[Token<'a>]) -> Result<Part<'a>, CoordParseError> {
    let (dir, rest) = match tokens {
        [
            Token {
                kind: Kind::Dir(d), ..
            },
            rest @ ..,
        ]
        | [
            rest @ ..,
            Token {
                kind: Kind::Dir(d), ..
            },
        ] => (Some(*d), rest),
        _ => (None, tokens),
    };
    let angle = angle(rest, dir.is_some()).ok_or(CoordParseError::Unreadable)?;
    Ok(Part { dir, angle })
}

/// 記号の形の角度（`35`・`35°`・`35°40'`・`35°40'52"`）を先頭から最も長く取ったときの字句の数。
fn symbolic_len(tokens: &[Token]) -> usize {
    let is = |i: usize, f: fn(Kind) -> bool| tokens.get(i).is_some_and(|t| f(t.kind));
    let num = |k: Kind| matches!(k, Kind::Num(_));
    if !is(0, num) {
        0
    } else if !is(1, |k| k == Kind::Deg) {
        1
    } else if !(is(2, num) && is(3, |k| k == Kind::Min)) {
        2
    } else if !(is(4, num) && is(5, |k| k == Kind::Sec)) {
        4
    } else {
        6
    }
}

/// 字句の並びを緯度・経度の 2 つ（並びは入力の順）に分けて読む。
fn split<'a>(tokens: &[Token<'a>]) -> Result<[Part<'a>; 2], CoordParseError> {
    let position = |f: fn(&Kind) -> bool| {
        tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| f(&t.kind))
            .map(|(i, _)| i)
            .collect::<Vec<_>>()
    };
    let seps = position(|k| *k == Kind::Sep);
    let at = match seps[..] {
        // 区切りの記号の前と後。
        [i] => return Ok([part(&tokens[..i])?, part(&tokens[i + 1..])?]),
        [] => match position(|k| matches!(k, Kind::Dir(_)))[..] {
            // 方角の記号が両方とも数の前（`N35 E139`）: 2 つ目の記号の前で分ける。
            [0, j] => j,
            // 両方とも数の後（`35N 139E`）: 1 つ目の記号の後で分ける。
            [i, j] if j + 1 == tokens.len() => i + 1,
            // 方角の記号なし: 1 つ目の角度を記号の形で最も長く取る。2 つ目との間に空白が要る。
            [] => {
                let n = symbolic_len(tokens);
                if !tokens.get(n).is_some_and(|t| t.space_before) {
                    return Err(CoordParseError::Unreadable);
                }
                n
            }
            _ => return Err(CoordParseError::Unreadable),
        },
        _ => return Err(CoordParseError::Unreadable),
    };
    Ok([part(&tokens[..at])?, part(&tokens[at..])?])
}

/// URL でない文字列から（緯度, 経度）を読む。範囲は検査しない。
fn parse_text(s: &str) -> Result<(f64, f64), CoordParseError> {
    let tokens = tokenize(s)?;
    let [first, second] = split(&tokens)?;
    if [first, second]
        .iter()
        .any(|p| p.dir.is_some() && p.first_number().signed())
    {
        return Err(CoordParseError::SignWithDirection);
    }
    let axis = |p: &Part| p.dir.map(|d| d.axis);
    let swap = match (axis(&first), axis(&second)) {
        (Some(a), Some(b)) if a == b => return Err(CoordParseError::ConflictingDirections),
        (Some(Axis::Lon), _) | (_, Some(Axis::Lat)) => true,
        _ => false,
    };
    let (a, b) = (first.value()?, second.value()?);
    Ok(if swap { (b, a) } else { (a, b) })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 読んだ座標が期待の値（1e-9 度以内）か。
    #[track_caller]
    fn assert_coord(text: &str, lat: f64, lon: f64) {
        let g = parse_coordinates(text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
        assert!(
            (g.lat - lat).abs() < 1e-9 && (g.lon - lon).abs() < 1e-9,
            "{text:?}: {g:?}（期待 {lat}, {lon}）"
        );
    }

    #[track_caller]
    fn assert_err(text: &str, expected: CoordParseError) {
        assert_eq!(parse_coordinates(text), Err(expected), "{text:?}");
    }

    /// 度分秒を 10 進の度にする（期待の値の計算用）。
    fn dms(d: f64, m: f64, s: f64) -> f64 {
        d + m / 60.0 + s / 3600.0
    }

    #[test]
    fn decimal_degrees_with_each_separator() {
        for text in [
            "35.681236, 139.767125",
            "35.681236,139.767125",
            "35.681236 139.767125",
            "35.681236;139.767125",
            "35.681236; 139.767125",
            "35.681236 ; 139.767125",
            "35.681236 , 139.767125",
            "35.681236、139.767125",
            "35.681236\t139.767125",
            "35.681236\n139.767125",
            "35.681236   139.767125",
            "  35.681236, 139.767125  \n",
            "35.681236\u{a0}139.767125",
            "35.681236°, 139.767125°",
            "35.681236° 139.767125°",
            "+35.681236, +139.767125",
        ] {
            assert_coord(text, 35.681236, 139.767125);
        }
    }

    #[test]
    fn decimal_degrees_signs_and_digits() {
        assert_coord("-33.8568, 151.2153", -33.8568, 151.2153);
        assert_coord("-33.8568 151.2153", -33.8568, 151.2153);
        assert_coord("40.7128, -74.0060", 40.7128, -74.006);
        assert_coord("35.68 -139.76", 35.68, -139.76);
        assert_coord("35.68,-139.76", 35.68, -139.76);
        assert_coord("35, 139", 35.0, 139.0);
        assert_coord("0, 0", 0.0, 0.0);
        assert_coord("035.5, 0139.5", 35.5, 139.5);
        // 小数の桁は制限しない（f64 の精度で、最も近い値に読む）。
        let g = parse_coordinates("35.681236123456789012, 139.767125987654321098").unwrap();
        assert_eq!(g.lat, "35.681236123456789012".parse::<f64>().unwrap());
        assert_eq!(g.lon, "139.767125987654321098".parse::<f64>().unwrap());
        // -0 は 0 にする。
        let g = parse_coordinates("-0, -0.0").unwrap();
        assert!(g.lat == 0.0 && g.lat.is_sign_positive(), "{g:?}");
        assert!(g.lon == 0.0 && g.lon.is_sign_positive(), "{g:?}");
    }

    #[test]
    fn direction_letters() {
        for text in [
            "35.681236N 139.767125E",
            "35.681236N139.767125E",
            "35.681236 N 139.767125 E",
            "35.681236N, 139.767125E",
            "N35.681236 E139.767125",
            "N35.681236E139.767125",
            "N 35.681236 E 139.767125",
            "N35.681236, E139.767125",
            "35.681236° N, 139.767125° E",
            "35.681236°N 139.767125°E",
            "35.681236n 139.767125e",
            "n35.681236 e139.767125",
            // 片方だけに記号（区切りの記号があるとき）。
            "35.681236N, 139.767125",
            "35.681236, 139.767125E",
            "N 35.681236, 139.767125",
            // 区切りの記号があれば、数の前と後が混ざってもよい。
            "35.681236N, E139.767125",
        ] {
            assert_coord(text, 35.681236, 139.767125);
        }
        // 符号と方角の記号は、別々の数になら使える。
        assert_coord("-35, 139E", -35.0, 139.0);
        // S・W は負。
        assert_coord("33.8568S 151.2153E", -33.8568, 151.2153);
        assert_coord("S33.8568 E151.2153", -33.8568, 151.2153);
        assert_coord("40.7128N 74.0060W", 40.7128, -74.006);
        assert_coord("33.8568s, 70.5w", -33.8568, -70.5);
        assert_coord("S 0, W 0", 0.0, 0.0);
    }

    #[test]
    fn longitude_first_is_swapped() {
        for text in [
            "139.767125E 35.681236N",
            "E139.767125 N35.681236",
            "139.767125E, 35.681236N",
            "139.767125° E, 35.681236° N",
            "139.767125E, 35.681236",
            "139.767125, 35.681236N",
            "東経 139.767125°、北緯 35.681236°",
        ] {
            assert_coord(text, 35.681236, 139.767125);
        }
        assert_coord("74.0060W 40.7128N", 40.7128, -74.006);
        assert_coord("151.2153E, 33.8568S", -33.8568, 151.2153);
    }

    #[test]
    fn japanese_hemisphere_words() {
        // 画面の formatGps の出力の形。
        assert_coord("北緯 35.681236°、東経 139.767125°", 35.681236, 139.767125);
        assert_coord("南緯 33.856800°、西経 151.215300°", -33.8568, -151.2153);
        assert_coord("南緯 33.856800°、東経 151.215300°", -33.8568, 151.2153);
        assert_coord("北緯35.681236、東経139.767125", 35.681236, 139.767125);
        assert_coord("北緯35.681236 東経139.767125", 35.681236, 139.767125);
        assert_coord("北緯 35.681236° 東経 139.767125°", 35.681236, 139.767125);
        assert_coord("北緯 35.681236, 東経 139.767125", 35.681236, 139.767125);
        assert_coord("35.681236北緯 139.767125東経", 35.681236, 139.767125);
        // 度分秒と組み合わせる。
        assert_coord(
            "北緯 35°40'52.4\"、東経 139°46'1.7\"",
            dms(35.0, 40.0, 52.4),
            dms(139.0, 46.0, 1.7),
        );
    }

    #[test]
    fn degrees_minutes_seconds() {
        let (lat, lon) = (dms(35.0, 40.0, 52.4), dms(139.0, 46.0, 1.7));
        for text in [
            "35°40'52.4\"N 139°46'01.7\"E",
            "35°40'52.4\"N, 139°46'01.7\"E",
            "35°40′52.4″N 139°46′1.7″E",
            "35° 40' 52.4\" N, 139° 46' 1.7\" E",
            "35° 40' 52.4\" N 139° 46' 1.7\" E",
            "N 35°40'52.4\" E 139°46'1.7\"",
            "N35°40'52.4\"E139°46'1.7\"",
            "35°40'52.4\"n 139°46'01.7\"e",
            "139°46'01.7\"E 35°40'52.4\"N",
            // 記号のない度分秒（方角の記号があるときだけ）。
            "35 40 52.4 N 139 46 1.7 E",
            "35 40 52.4N 139 46 1.7E",
            "N 35 40 52.4 E 139 46 1.7",
            "35 40 52.4 N, 139 46 1.7 E",
            "35 40 52.4 N, 139°46'1.7\"",
            // 方角の記号がなくても、度分秒の記号があればよい。
            "35°40'52.4\" 139°46'1.7\"",
            "35°40'52.4\", 139°46'1.7\"",
        ] {
            assert_coord(text, lat, lon);
        }
        assert_coord(
            "33°51'24.5\"S 151°12'55.1\"E",
            -dms(33.0, 51.0, 24.5),
            dms(151.0, 12.0, 55.1),
        );
        // 符号付きの度分秒（方角の記号なし）は全体に符号が付く。
        assert_coord(
            "-33°51'24.5\", 151°12'55.1\"",
            -dms(33.0, 51.0, 24.5),
            dms(151.0, 12.0, 55.1),
        );
        assert_coord(
            "-33°51'24.5\" -70°40'1\"",
            -dms(33.0, 51.0, 24.5),
            -dms(70.0, 40.0, 1.0),
        );
        assert_coord("35°0'0\"N 139°0'0\"E", 35.0, 139.0);
    }

    #[test]
    fn degrees_minutes() {
        let (lat, lon) = (35.0 + 40.873 / 60.0, 139.0 + 46.028 / 60.0);
        for text in [
            "35°40.873'N 139°46.028'E",
            "35°40.873′N, 139°46.028′E",
            "N35°40.873' E139°46.028'",
            "35 40.873 N 139 46.028 E",
            "35°40.873', 139°46.028'",
        ] {
            assert_coord(text, lat, lon);
        }
        assert_coord(
            "35°40'N 139°46'E",
            dms(35.0, 40.0, 0.0),
            dms(139.0, 46.0, 0.0),
        );
    }

    #[test]
    fn minutes_and_seconds_bounds() {
        // 分・秒は 0 以上 60 未満。
        assert_coord("35°59'59.999\"N 139°0'0\"E", dms(35.0, 59.0, 59.999), 139.0);
        assert_coord("35°59.999999'N 139°0'E", 35.0 + 59.999999 / 60.0, 139.0);
        assert_err("35°60'N 139°0'E", CoordParseError::InvalidDms);
        assert_err("35°0'60\"N 139°0'E", CoordParseError::InvalidDms);
        assert_err("35°40'60.0\"N 139°46'1.7\"E", CoordParseError::InvalidDms);
        assert_err("35 60 N 139 E", CoordParseError::InvalidDms);
        assert_err("35 40 60 N 139 E", CoordParseError::InvalidDms);
        assert_err("35°40'52.4\"N 139°60'1.7\"E", CoordParseError::InvalidDms);
        // 度分・度分秒の度は整数、秒があれば分も整数。
        assert_err("35.5°30'N 139°E", CoordParseError::InvalidDms);
        assert_err("35°40.5'30\"N 139°E", CoordParseError::InvalidDms);
        assert_err("35.5 30 N 139 E", CoordParseError::InvalidDms);
    }

    #[test]
    fn full_width_characters_are_normalized() {
        assert_coord(
            "３５．６８１２３６，１３９．７６７１２５",
            35.681236,
            139.767125,
        );
        assert_coord(
            "３５．６８１２３６，　１３９．７６７１２５",
            35.681236,
            139.767125,
        );
        assert_coord(
            "３５．６８１２３６　１３９．７６７１２５",
            35.681236,
            139.767125,
        );
        assert_coord("－３３．８５６８，　＋１５１．２１５３", -33.8568, 151.2153);
        assert_coord("　35.681236, 139.767125　", 35.681236, 139.767125);
        assert_coord(
            "北緯　３５．６８１２３６°、東経　１３９．７６７１２５°",
            35.681236,
            139.767125,
        );
        assert_coord(
            "３５°４０'５２．４\"N　１３９°４６'１．７\"E",
            dms(35.0, 40.0, 52.4),
            dms(139.0, 46.0, 1.7),
        );
    }

    #[test]
    fn google_maps_urls() {
        for text in [
            "https://www.google.com/maps/@35.681236,139.767125,17z",
            "https://www.google.com/maps/@35.681236,139.767125,17z?entry=ttu",
            "https://www.google.com/maps/@35.681236,139.767125",
            "https://www.google.co.jp/maps/@35.681236,139.767125,15.5z/data=!3m1!1e3",
            "https://www.google.com/maps/place/Tokyo+Station/@35.681236,139.767125,17z/data=!4m6",
            "https://maps.google.com/?q=35.681236,139.767125",
            "https://maps.google.com/maps?q=35.681236%2C139.767125&z=17",
            "https://maps.google.com/maps?q=35.681236,+139.767125",
            "https://maps.google.com/maps?q=35.681236%2C%20139.767125",
            "https://www.google.com/maps?ll=35.681236,139.767125&z=17",
            "https://www.google.com/maps?z=17&ll=35.681236,139.767125#top",
            "http://maps.google.com/?q=35.681236,139.767125",
            "HTTPS://MAPS.GOOGLE.COM/?q=35.681236,139.767125",
            "  https://www.google.com/maps/@35.681236,139.767125,17z  ",
        ] {
            assert_coord(text, 35.681236, 139.767125);
        }
        assert_coord(
            "https://www.google.com/maps/@-33.8568,151.2153,15z",
            -33.8568,
            151.2153,
        );
        assert_coord("https://maps.google.com/?q=-33.8568,-70.5", -33.8568, -70.5);
        // q → ll → @ の順。座標でない q は飛ばす。
        assert_coord("https://maps.google.com/maps?ll=1,2&q=3,4", 3.0, 4.0);
        assert_coord("https://www.google.com/maps/@5,6,17z?ll=1,2", 1.0, 2.0);
        assert_coord(
            "https://maps.google.com/maps?q=Tokyo+Station&ll=35.681236,139.767125",
            35.681236,
            139.767125,
        );
        assert_coord(
            "https://www.google.com/maps/place/Tokyo/@35.681236,139.767125,17z?q=%E6%9D%B1%E4%BA%AC",
            35.681236,
            139.767125,
        );
    }

    #[test]
    fn urls_without_coordinates_are_rejected() {
        for text in [
            "https://maps.app.goo.gl/abcdefg",
            "https://maps.google.com/?q=Tokyo+Station",
            "https://maps.google.com/?q=",
            "https://maps.google.com/?xq=35.68,139.76",
            "https://maps.google.com/?q=35.68",
            "https://maps.google.com/?q=35.68,139.76,10",
            "https://maps.google.com/?q=35.68%2",
            "https://maps.google.com/?q=35.68%ZZ139.76",
            "https://www.openstreetmap.org/#map=17/35.68124/139.76713",
            "https://www.google.com/maps/@35.68",
            "https://www.google.com/maps/@35.68,139.76abc",
            "https://user@example.com/",
            "https://example.com/?ll=35.68N,139.76E",
        ] {
            assert_err(text, CoordParseError::NoCoordinatesInUrl);
        }
        // URL の中の座標も範囲を検査する。
        assert_err(
            "https://www.google.com/maps/@91,0,17z",
            CoordParseError::LatitudeOutOfRange(91.0),
        );
        assert_err(
            "https://maps.google.com/?q=0,180.5",
            CoordParseError::LongitudeOutOfRange(180.5),
        );
        // http(s) で始まらなければ URL として読まない。
        assert_err(
            "www.google.com/maps/@35.681236,139.767125,17z",
            CoordParseError::Unreadable,
        );
    }

    #[test]
    fn empty_text_is_rejected() {
        for text in ["", " ", "\t\n", "　", "　 　"] {
            assert_err(text, CoordParseError::Empty);
        }
    }

    #[test]
    fn unreadable_text_is_rejected() {
        for text in [
            "abc",
            "東京駅",
            "35.681236",
            "N35.681236",
            "35.68, 139.76, 10",
            "35.68 139.76 10",
            "35.68,,139.76",
            "35.68, ",
            ", 139.76",
            "35.68;139.76;",
            "35., 139.",
            ".5, 1",
            "35.68.1, 139.76",
            "35,68 139,76",
            "- 35, 139",
            "--35, 139",
            "+-35, 139",
            "35.68-139.76",
            "35.68+139.76",
            "35°40'52.4\"139°46'1.7\"",
            // 記号のない度分秒は方角の記号があるときだけ。
            "35 40 139 46",
            "35 40 52.4, 139 46 1.7",
            "35 40 52.4 139 46 1.7",
            // 区切りの記号がないときは、方角の記号を両方に同じ側に付ける。
            "35.68N 139.76",
            "35.68 139.76E",
            "N35.68 139.76",
            "N35.68 139.76E",
            "35.68N E139.76",
            "35.68N 139.76E 10",
            // 記号の形が崩れている。
            "35°52\"N 139°1\"E",
            "35°40'52.4N 139°46'1.7E",
            "35'N 139'E",
            "35°°, 139",
            "35°40'52.4\"\"N 139E",
            "35°-40'N 139E",
            "35 -40 N 139 E",
            "N35N, 139",
            "N, 139",
            "35 N S, 139",
            // 知らない文字・語。
            "North 35, East 139",
            "北 35, 東 139",
            "緯 35, 経 139",
            "1e2, 3",
            "NaN, 0",
            "inf, 0",
            "Infinity, -Infinity",
            "(35.68, 139.76)",
            "35.68\u{2212}139.76",
            "35.68º, 139.76º",
            "Ｎ35.68 Ｅ139.76",
        ] {
            assert_err(text, CoordParseError::Unreadable);
        }
    }

    #[test]
    fn range_boundaries() {
        for (text, lat, lon) in [
            ("90, 180", 90.0, 180.0),
            ("-90, -180", -90.0, -180.0),
            ("90N 180E", 90.0, 180.0),
            ("90S 180W", -90.0, -180.0),
            ("90°0'0\"N 180°0'0\"W", 90.0, -180.0),
            ("89.999999, 179.999999", 89.999999, 179.999999),
            ("北緯 90°、西経 180°", 90.0, -180.0),
        ] {
            assert_coord(text, lat, lon);
        }
        assert_err(
            "90.000001, 0",
            CoordParseError::LatitudeOutOfRange(90.000001),
        );
        assert_err(
            "-90.000001, 0",
            CoordParseError::LatitudeOutOfRange(-90.000001),
        );
        assert_err(
            "0, 180.000001",
            CoordParseError::LongitudeOutOfRange(180.000001),
        );
        assert_err(
            "0, -180.000001",
            CoordParseError::LongitudeOutOfRange(-180.000001),
        );
        assert_err(
            "90.000001N 0E",
            CoordParseError::LatitudeOutOfRange(90.000001),
        );
        assert_err("90.5S 0E", CoordParseError::LatitudeOutOfRange(-90.5));
        assert_err("0N 180.5W", CoordParseError::LongitudeOutOfRange(-180.5));
        assert_err(
            "90°0'0.1\"N 0E",
            CoordParseError::LatitudeOutOfRange(dms(90.0, 0.0, 0.1)),
        );
        assert_err(
            "91°0'0\"N 0°0'0\"E",
            CoordParseError::LatitudeOutOfRange(91.0),
        );
        // 緯度と経度を取り違えた貼り付け（緯度を先に検査する）。
        assert_err(
            "139.767125, 35.681236",
            CoordParseError::LatitudeOutOfRange(139.767125),
        );
        assert_err("1000, 2000", CoordParseError::LatitudeOutOfRange(1000.0));
        // 桁が多すぎて無限になる値も範囲外。
        let huge = "9".repeat(400);
        assert_err(
            &format!("{huge}, 0"),
            CoordParseError::LatitudeOutOfRange(f64::INFINITY),
        );
        assert_err(
            &format!("0, -{huge}"),
            CoordParseError::LongitudeOutOfRange(f64::NEG_INFINITY),
        );
    }

    #[test]
    fn conflicting_directions_are_rejected() {
        for text in [
            "35N 139N",
            "35S 139N",
            "139E 35E",
            "139E 35W",
            "N35 N139",
            "35N, 139S",
            "139E, 35W",
            "北緯 35、北緯 139",
            "東経 139、西経 35",
            "35°40'N 139°46'N",
        ] {
            assert_err(text, CoordParseError::ConflictingDirections);
        }
    }

    #[test]
    fn sign_with_direction_is_rejected() {
        for text in [
            "-35N 139E",
            "+35N 139E",
            "35N -139E",
            "-33.8568S, 151.2153E",
            "N-35 E139",
            "北緯 -35、東経 139",
            "-35°40'N 139°E",
            "-35 40 N 139 E",
        ] {
            assert_err(text, CoordParseError::SignWithDirection);
        }
    }

    #[test]
    fn error_messages_are_japanese() {
        assert_eq!(CoordParseError::Empty.to_string(), "座標が空です");
        assert_eq!(
            CoordParseError::LatitudeOutOfRange(139.76).to_string(),
            "緯度が範囲外です（-90〜90）: 139.76"
        );
        assert_eq!(
            CoordParseError::LongitudeOutOfRange(-181.0).to_string(),
            "経度が範囲外です（-180〜180）: -181"
        );
    }

    #[test]
    fn format_coordinates_matches_the_ui() {
        let f = |lat, lon| format_coordinates(GpsCoord { lat, lon });
        // 画面の gpsDecimal（metadataLogic.test.ts）と同じ。
        assert_eq!(f(35.681236, 139.767125), "35.681236, 139.767125");
        assert_eq!(f(-33.8568, -151.2153), "-33.856800, -151.215300");
        assert_eq!(f(35.0, 139.0), "35.000000, 139.000000");
        assert_eq!(f(90.0, -180.0), "90.000000, -180.000000");
        assert_eq!(f(35.68123649, 139.76712551), "35.681236, 139.767126");
        // -0 は "0.000000"（toFixed と同じ）。0 より少し小さい値は "-0.000000"（toFixed と同じ）。
        assert_eq!(f(-0.0, -0.0), "0.000000, 0.000000");
        assert_eq!(f(-1e-9, 1e-9), "-0.000000, 0.000000");
        // ちょうど中間は絶対値の大きい側へ（toFixed と同じ。Rust の書式は偶数の側へ丸めて 35.007812 になる）。
        assert_eq!(f(35.0078125, -35.0078125), "35.007813, -35.007813");
        assert_eq!(f(0.0234375, -0.0234375), "0.023438, -0.023438");
        // 10 進で 35.6812365 と書いた値は 2 進ではわずかに小さいので、中間ではない（toFixed も 35.681236）。
        assert_eq!(f(35.6812365, 0.0), "35.681236, 0.000000");
    }

    #[test]
    fn formatted_text_parses_back() {
        for (lat, lon) in [
            (35.681236, 139.767125),
            (-33.8568, 151.2153),
            (90.0, -180.0),
            (-90.0, 180.0),
            (0.0, 0.0),
        ] {
            let g = GpsCoord { lat, lon };
            assert_eq!(parse_coordinates(&format_coordinates(g)), Ok(g));
        }
    }
}

#[cfg(test)]
mod proptests {
    use proptest::prelude::*;

    use super::*;

    /// JavaScript の `toFixed(6)` の定義どおりの丸め（値の正確な 10 進の展開を、ちょうど中間は絶対値の
    /// 大きい側へ丸める）。f64 の 10 進の展開は小数 1074 桁までで尽きるので、1100 桁で書けば正確。
    fn to_fixed6_reference(v: f64) -> String {
        let exact = format!("{:.1100}", v.abs());
        let (int, frac) = exact.split_once('.').unwrap();
        let mut n: u128 = format!("{int}{}", &frac[..6]).parse().unwrap();
        if frac.as_bytes()[6] >= b'5' {
            n += 1;
        }
        let sign = if v < 0.0 { "-" } else { "" };
        format!("{sign}{}.{:06}", n / 1_000_000, n % 1_000_000)
    }

    /// 座標の範囲の値と、小数 6 桁のちょうど中間の値（奇数 / 128）と、0 の近く。
    fn any_degrees() -> impl Strategy<Value = f64> {
        prop_oneof![
            -180.0f64..=180.0,
            (-11520i32..=11519).prop_map(|k| f64::from(2 * k + 1) / 128.0),
            -1.0e-5f64..1.0e-5,
            Just(-0.0),
        ]
    }

    proptest! {
        #[test]
        fn fixed6_matches_to_fixed(v in any_degrees()) {
            prop_assert_eq!(fixed6(v), to_fixed6_reference(v), "{:e}", v);
        }

        #[test]
        fn formatted_coordinates_round_trip(lat in -90.0f64..=90.0, lon in -180.0f64..=180.0) {
            let text = format_coordinates(GpsCoord { lat, lon });
            let back = parse_coordinates(&text).unwrap();
            // 小数 6 桁に丸めた分（5e-7 度まで）だけずれる。
            prop_assert!((back.lat - lat).abs() <= 5.0e-7 + 1e-12, "{} → {:?}", text, back);
            prop_assert!((back.lon - lon).abs() <= 5.0e-7 + 1e-12, "{} → {:?}", text, back);
            prop_assert_eq!(format_coordinates(back), text);
        }

        #[test]
        fn decimal_pairs_are_read_exactly(
            lat in -90.0f64..=90.0,
            lon in -180.0f64..=180.0,
            sep in prop::sample::select(vec![", ", ",", " ", ";", "; ", "、", "\t"]),
        ) {
            // Rust の最短の表記（`{}`）は f64 に正確に戻る。
            let text = format!("{lat}{sep}{lon}");
            let g = parse_coordinates(&text).unwrap();
            prop_assert_eq!(g, GpsCoord { lat: lat + 0.0, lon: lon + 0.0 });
        }

        #[test]
        fn arbitrary_text_never_panics(text in "\\PC{0,40}") {
            if let Ok(g) = parse_coordinates(&text) {
                prop_assert!(g.is_valid(), "{:?} → {:?}", text, g);
            }
        }

        #[test]
        fn coordinate_like_text_never_panics(
            text in "[0-9NSEWnsew°'\"′″., ;、+\\-北南東西緯経@?=&q:/%０-９，．－＋　]{0,32}",
        ) {
            if let Ok(g) = parse_coordinates(&text) {
                prop_assert!(g.is_valid(), "{:?} → {:?}", text, g);
            }
        }
    }
}
