// 位置情報の後付け（LIB-21）の画面の計算（library/geotagLogic.ts）のテスト。
//
// 座標の解釈のケースは、genzo-model の crates/genzo-model/src/location.rs の tests の写し（同じ形を同じ規則で読む
// ことを確かめる。規則を変えるときは両方を直す）。

import { describe, expect, it } from "vitest";

import { MOCK_GEOTAGGED_DETAILS, MOCK_LOCATION_EDITS, MOCK_LOCATION_PLAN } from "../api/mock";
import type { LocationEditPlan } from "../api/types";
import {
  type CoordParseError,
  coordErrorMessage,
  describeLocationEdit,
  describeLocationPlan,
  describeLocationUndo,
  describeTargets,
  gpsSourceNote,
  locationSummary,
  parseCoordinates,
} from "./geotagLogic";
import { formatGps, gpsDecimal } from "./metadataLogic";

// テストの文字列に入れる特別な文字（ソースの中では見分けにくいので名前を付ける）。
const NBSP = String.fromCharCode(0xa0);
const IDEOGRAPHIC_SPACE = String.fromCharCode(0x3000);
const MINUS_SIGN = String.fromCharCode(0x2212);
const NEL = String.fromCharCode(0x85);
const BOM = String.fromCharCode(0xfeff);

/** 読んだ座標が期待の値（1e-9 度以内）か。 */
function expectCoord(text: string, lat: number, lon: number): void {
  const r = parseCoordinates(text);
  if (!r.ok) throw new Error(`${JSON.stringify(text)}: ${coordErrorMessage(r.error)}`);
  expect(
    Math.abs(r.coord.lat - lat) < 1e-9 && Math.abs(r.coord.lon - lon) < 1e-9,
    `${JSON.stringify(text)}: ${JSON.stringify(r.coord)}（期待 ${lat}, ${lon}）`,
  ).toBe(true);
}

function expectError(text: string, error: CoordParseError): void {
  expect(parseCoordinates(text), JSON.stringify(text)).toEqual({ ok: false, error });
}

const UNREADABLE: CoordParseError = { kind: "unreadable" };

/** 度分秒を 10 進の度にする（期待の値の計算用）。 */
const dms = (d: number, m: number, s: number) => d + m / 60 + s / 3600;

describe("座標の解釈: 受け付ける形", () => {
  it("10 進の度（区切りごと）", () => {
    for (const text of [
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
      `35.681236${NBSP}139.767125`,
      "35.681236°, 139.767125°",
      "35.681236° 139.767125°",
      "+35.681236, +139.767125",
    ]) {
      expectCoord(text, 35.681236, 139.767125);
    }
  });

  it("10 進の度（符号・桁）", () => {
    expectCoord("-33.8568, 151.2153", -33.8568, 151.2153);
    expectCoord("-33.8568 151.2153", -33.8568, 151.2153);
    expectCoord("40.7128, -74.0060", 40.7128, -74.006);
    expectCoord("35.68 -139.76", 35.68, -139.76);
    expectCoord("35.68,-139.76", 35.68, -139.76);
    expectCoord("35, 139", 35, 139);
    expectCoord("0, 0", 0, 0);
    expectCoord("035.5, 0139.5", 35.5, 139.5);
    // 小数の桁は制限しない（最も近い値に読む）。
    const r = parseCoordinates("35.681236123456789012, 139.767125987654321098");
    expect(r).toEqual({ ok: true, coord: { lat: Number("35.681236123456789012"), lon: Number("139.767125987654321098") } });
    // -0 は 0 にする。
    const z = parseCoordinates("-0, -0.0");
    expect(z.ok && Object.is(z.coord.lat, 0) && Object.is(z.coord.lon, 0)).toBe(true);
  });

  it("方角の記号", () => {
    for (const text of [
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
    ]) {
      expectCoord(text, 35.681236, 139.767125);
    }
    // 符号と方角の記号は、別々の数になら使える。
    expectCoord("-35, 139E", -35, 139);
    // S・W は負。
    expectCoord("33.8568S 151.2153E", -33.8568, 151.2153);
    expectCoord("S33.8568 E151.2153", -33.8568, 151.2153);
    expectCoord("40.7128N 74.0060W", 40.7128, -74.006);
    expectCoord("33.8568s, 70.5w", -33.8568, -70.5);
    expectCoord("S 0, W 0", 0, 0);
  });

  it("経度が先なら入れ替える", () => {
    for (const text of [
      "139.767125E 35.681236N",
      "E139.767125 N35.681236",
      "139.767125E, 35.681236N",
      "139.767125° E, 35.681236° N",
      "139.767125E, 35.681236",
      "139.767125, 35.681236N",
      "東経 139.767125°、北緯 35.681236°",
    ]) {
      expectCoord(text, 35.681236, 139.767125);
    }
    expectCoord("74.0060W 40.7128N", 40.7128, -74.006);
    expectCoord("151.2153E, 33.8568S", -33.8568, 151.2153);
  });

  it("北緯・南緯・東経・西経", () => {
    // 画面の formatGps の出力の形。
    expectCoord("北緯 35.681236°、東経 139.767125°", 35.681236, 139.767125);
    expectCoord("南緯 33.856800°、西経 151.215300°", -33.8568, -151.2153);
    expectCoord("南緯 33.856800°、東経 151.215300°", -33.8568, 151.2153);
    expectCoord("北緯35.681236、東経139.767125", 35.681236, 139.767125);
    expectCoord("北緯35.681236 東経139.767125", 35.681236, 139.767125);
    expectCoord("北緯 35.681236° 東経 139.767125°", 35.681236, 139.767125);
    expectCoord("北緯 35.681236, 東経 139.767125", 35.681236, 139.767125);
    expectCoord("35.681236北緯 139.767125東経", 35.681236, 139.767125);
    // 度分秒と組み合わせる。
    expectCoord(`北緯 35°40'52.4"、東経 139°46'1.7"`, dms(35, 40, 52.4), dms(139, 46, 1.7));
  });

  it("度分秒", () => {
    const [lat, lon] = [dms(35, 40, 52.4), dms(139, 46, 1.7)];
    for (const text of [
      `35°40'52.4"N 139°46'01.7"E`,
      `35°40'52.4"N, 139°46'01.7"E`,
      "35°40′52.4″N 139°46′1.7″E",
      `35° 40' 52.4" N, 139° 46' 1.7" E`,
      `35° 40' 52.4" N 139° 46' 1.7" E`,
      `N 35°40'52.4" E 139°46'1.7"`,
      `N35°40'52.4"E139°46'1.7"`,
      `35°40'52.4"n 139°46'01.7"e`,
      `139°46'01.7"E 35°40'52.4"N`,
      // 記号のない度分秒（方角の記号があるときだけ）。
      "35 40 52.4 N 139 46 1.7 E",
      "35 40 52.4N 139 46 1.7E",
      "N 35 40 52.4 E 139 46 1.7",
      "35 40 52.4 N, 139 46 1.7 E",
      `35 40 52.4 N, 139°46'1.7"`,
      // 方角の記号がなくても、度分秒の記号があればよい。
      `35°40'52.4" 139°46'1.7"`,
      `35°40'52.4", 139°46'1.7"`,
    ]) {
      expectCoord(text, lat, lon);
    }
    expectCoord(`33°51'24.5"S 151°12'55.1"E`, -dms(33, 51, 24.5), dms(151, 12, 55.1));
    // 符号付きの度分秒（方角の記号なし）は全体に符号が付く。
    expectCoord(`-33°51'24.5", 151°12'55.1"`, -dms(33, 51, 24.5), dms(151, 12, 55.1));
    expectCoord(`-33°51'24.5" -70°40'1"`, -dms(33, 51, 24.5), -dms(70, 40, 1));
    expectCoord(`35°0'0"N 139°0'0"E`, 35, 139);
  });

  it("度分", () => {
    const [lat, lon] = [35 + 40.873 / 60, 139 + 46.028 / 60];
    for (const text of [
      "35°40.873'N 139°46.028'E",
      "35°40.873′N, 139°46.028′E",
      "N35°40.873' E139°46.028'",
      "35 40.873 N 139 46.028 E",
      "35°40.873', 139°46.028'",
    ]) {
      expectCoord(text, lat, lon);
    }
    expectCoord("35°40'N 139°46'E", dms(35, 40, 0), dms(139, 46, 0));
  });

  it("分・秒の範囲と整数", () => {
    // 分・秒は 0 以上 60 未満。
    expectCoord(`35°59'59.999"N 139°0'0"E`, dms(35, 59, 59.999), 139);
    expectCoord("35°59.999999'N 139°0'E", 35 + 59.999999 / 60, 139);
    const invalid: CoordParseError = { kind: "invalid_dms" };
    expectError("35°60'N 139°0'E", invalid);
    expectError(`35°0'60"N 139°0'E`, invalid);
    expectError(`35°40'60.0"N 139°46'1.7"E`, invalid);
    expectError("35 60 N 139 E", invalid);
    expectError("35 40 60 N 139 E", invalid);
    expectError(`35°40'52.4"N 139°60'1.7"E`, invalid);
    // 度分・度分秒の度は整数、秒があれば分も整数。
    expectError("35.5°30'N 139°E", invalid);
    expectError(`35°40.5'30"N 139°E`, invalid);
    expectError("35.5 30 N 139 E", invalid);
  });

  it("全角の文字を半角にしてから読む", () => {
    expectCoord("３５．６８１２３６，１３９．７６７１２５", 35.681236, 139.767125);
    expectCoord(`３５．６８１２３６，${IDEOGRAPHIC_SPACE}１３９．７６７１２５`, 35.681236, 139.767125);
    expectCoord(`３５．６８１２３６${IDEOGRAPHIC_SPACE}１３９．７６７１２５`, 35.681236, 139.767125);
    expectCoord(`－３３．８５６８，${IDEOGRAPHIC_SPACE}＋１５１．２１５３`, -33.8568, 151.2153);
    expectCoord(`${IDEOGRAPHIC_SPACE}35.681236, 139.767125${IDEOGRAPHIC_SPACE}`, 35.681236, 139.767125);
    expectCoord(`北緯${IDEOGRAPHIC_SPACE}３５．６８１２３６°、東経${IDEOGRAPHIC_SPACE}１３９．７６７１２５°`, 35.681236, 139.767125);
    expectCoord(`３５°４０'５２．４"N${IDEOGRAPHIC_SPACE}１３９°４６'１．７"E`, dms(35, 40, 52.4), dms(139, 46, 1.7));
  });

  it("Google マップの URL", () => {
    for (const text of [
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
    ]) {
      expectCoord(text, 35.681236, 139.767125);
    }
    expectCoord("https://www.google.com/maps/@-33.8568,151.2153,15z", -33.8568, 151.2153);
    expectCoord("https://maps.google.com/?q=-33.8568,-70.5", -33.8568, -70.5);
    // q → ll → @ の順。座標でない q は飛ばす。
    expectCoord("https://maps.google.com/maps?ll=1,2&q=3,4", 3, 4);
    expectCoord("https://www.google.com/maps/@5,6,17z?ll=1,2", 1, 2);
    expectCoord("https://maps.google.com/maps?q=Tokyo+Station&ll=35.681236,139.767125", 35.681236, 139.767125);
    expectCoord(
      "https://www.google.com/maps/place/Tokyo/@35.681236,139.767125,17z?q=%E6%9D%B1%E4%BA%AC",
      35.681236,
      139.767125,
    );
  });

  it("書いた座標を読み直せる（gpsDecimal・formatGps）", () => {
    for (const g of [
      { lat: 35.681236, lon: 139.767125 },
      { lat: -33.8568, lon: 151.2153 },
      { lat: 90, lon: -180 },
      { lat: -90, lon: 180 },
      { lat: 0, lon: 0 },
    ]) {
      expect(parseCoordinates(gpsDecimal(g))).toEqual({ ok: true, coord: g });
      expect(parseCoordinates(formatGps(g)!)).toEqual({ ok: true, coord: g });
    }
  });
});

describe("座標の解釈: 拒む形", () => {
  it("座標のない URL", () => {
    for (const text of [
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
    ]) {
      expectError(text, { kind: "no_coordinates_in_url" });
    }
    // URL の中の座標も範囲を検査する。
    expectError("https://www.google.com/maps/@91,0,17z", { kind: "latitude_out_of_range", value: 91 });
    expectError("https://maps.google.com/?q=0,180.5", { kind: "longitude_out_of_range", value: 180.5 });
    // http(s) で始まらなければ URL として読まない。
    expectError("www.google.com/maps/@35.681236,139.767125,17z", UNREADABLE);
  });

  it("空", () => {
    for (const text of ["", " ", "\t\n", IDEOGRAPHIC_SPACE, `${IDEOGRAPHIC_SPACE} ${IDEOGRAPHIC_SPACE}`]) {
      expectError(text, { kind: "empty" });
    }
  });

  it("空白は Rust の char::is_whitespace と同じ（U+0085 は空白、U+FEFF は空白でない）", () => {
    expectError(NEL, { kind: "empty" });
    expectCoord(`35.681236${NEL}139.767125`, 35.681236, 139.767125);
    expectError(`${BOM}35.681236, 139.767125`, UNREADABLE);
  });

  it("読めない形", () => {
    for (const text of [
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
      `35°40'52.4"139°46'1.7"`,
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
      `35°52"N 139°1"E`,
      "35°40'52.4N 139°46'1.7E",
      "35'N 139'E",
      "35°°, 139",
      `35°40'52.4""N 139E`,
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
      `35.68${MINUS_SIGN}139.76`,
      "35.68º, 139.76º",
      "Ｎ35.68 Ｅ139.76",
    ]) {
      expectError(text, UNREADABLE);
    }
  });

  it("範囲の境界", () => {
    for (const [text, lat, lon] of [
      ["90, 180", 90, 180],
      ["-90, -180", -90, -180],
      ["90N 180E", 90, 180],
      ["90S 180W", -90, -180],
      [`90°0'0"N 180°0'0"W`, 90, -180],
      ["89.999999, 179.999999", 89.999999, 179.999999],
      ["北緯 90°、西経 180°", 90, -180],
    ] as const) {
      expectCoord(text, lat, lon);
    }
    expectError("90.000001, 0", { kind: "latitude_out_of_range", value: 90.000001 });
    expectError("-90.000001, 0", { kind: "latitude_out_of_range", value: -90.000001 });
    expectError("0, 180.000001", { kind: "longitude_out_of_range", value: 180.000001 });
    expectError("0, -180.000001", { kind: "longitude_out_of_range", value: -180.000001 });
    expectError("90.000001N 0E", { kind: "latitude_out_of_range", value: 90.000001 });
    expectError("90.5S 0E", { kind: "latitude_out_of_range", value: -90.5 });
    expectError("0N 180.5W", { kind: "longitude_out_of_range", value: -180.5 });
    expectError(`90°0'0.1"N 0E`, { kind: "latitude_out_of_range", value: dms(90, 0, 0.1) });
    expectError(`91°0'0"N 0°0'0"E`, { kind: "latitude_out_of_range", value: 91 });
    // 緯度と経度を取り違えた貼り付け（緯度を先に検査する）。
    expectError("139.767125, 35.681236", { kind: "latitude_out_of_range", value: 139.767125 });
    expectError("1000, 2000", { kind: "latitude_out_of_range", value: 1000 });
    // 桁が多すぎて無限になる値も範囲外。
    const huge = "9".repeat(400);
    expectError(`${huge}, 0`, { kind: "latitude_out_of_range", value: Infinity });
    expectError(`0, -${huge}`, { kind: "longitude_out_of_range", value: -Infinity });
  });

  it("方角の記号の矛盾", () => {
    for (const text of [
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
    ]) {
      expectError(text, { kind: "conflicting_directions" });
    }
  });

  it("符号と方角の記号を同じ数に付ける", () => {
    for (const text of [
      "-35N 139E",
      "+35N 139E",
      "35N -139E",
      "-33.8568S, 151.2153E",
      "N-35 E139",
      "北緯 -35、東経 139",
      "-35°40'N 139°E",
      "-35 40 N 139 E",
    ]) {
      expectError(text, { kind: "sign_with_direction" });
    }
  });

  it("誤りの説明は genzo-model と同じ文", () => {
    expect(coordErrorMessage({ kind: "empty" })).toBe("座標が空です");
    expect(coordErrorMessage({ kind: "unreadable" })).toBe("座標として読めません（例: 35.681236, 139.767125）");
    expect(coordErrorMessage({ kind: "latitude_out_of_range", value: 139.76 })).toBe("緯度が範囲外です（-90〜90）: 139.76");
    expect(coordErrorMessage({ kind: "longitude_out_of_range", value: -181 })).toBe("経度が範囲外です（-180〜180）: -181");
  });
});

describe("座標の解釈: いろいろな入力で例外を投げない", () => {
  /** 決まった種から作る疑似乱数（0 以上 1 未満。テストの結果を毎回同じにする）。 */
  function random(seed: number): () => number {
    let s = seed >>> 0;
    return () => {
      s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
      return s / 2 ** 32;
    };
  }

  it("10 進の度の組は正確に読む", () => {
    const next = random(21);
    for (let i = 0; i < 300; i++) {
      const lat = (next() * 2 - 1) * 90;
      const lon = (next() * 2 - 1) * 180;
      const sep = [", ", ",", " ", ";", "; ", "、", "\t"][i % 7];
      const text = `${lat}${sep}${lon}`;
      // JavaScript の最短の表記は f64 に正確に戻る（指数の表記になる小さい値は除く）。
      if (text.includes("e")) continue;
      expect(parseCoordinates(text), text).toEqual({ ok: true, coord: { lat: lat + 0, lon: lon + 0 } });
    }
  });

  it("座標に使う文字の並び", () => {
    const chars = [..."0123456789NSEWnsew°'\"′″., ;、+-北南東西緯経@?=&q:/%０９，．－＋", IDEOGRAPHIC_SPACE];
    const next = random(7);
    for (let i = 0; i < 3000; i++) {
      const len = Math.floor(next() * 33);
      const text = Array.from({ length: len }, () => chars[Math.floor(next() * chars.length)]).join("");
      const r = parseCoordinates(text);
      if (r.ok) {
        const { lat, lon } = r.coord;
        expect(Number.isFinite(lat) && lat >= -90 && lat <= 90 && Number.isFinite(lon) && lon >= -180 && lon <= 180, text).toBe(true);
      }
    }
  });
});

describe("位置の変更の文", () => {
  const plan = (over: Partial<LocationEditPlan> = {}): LocationEditPlan => ({ ...MOCK_LOCATION_PLAN, ...over });

  it("付ける見込み: 変わる数・置き換える数・カメラの位置・除く動画", () => {
    const lines = describeLocationPlan(MOCK_LOCATION_PLAN, "set", false);
    expect(lines[0]).toBe("3 枚に位置を付けます（対象 3 枚）。");
    expect(lines).toContain("1 枚は、前に付けた位置を置き換えます。");
    expect(lines).toContain("カメラの位置がある 1 枚は、付けた位置を優先します（カメラの位置は残ります）。");
    expect(lines).toContain("動画 1 本は除きます（動画には位置を付けません）。");
    // 同じ位置が付いているものは、置き換える数に入れない。
    const same = describeLocationPlan(plan({ changed: 2, unchanged: 1, with_assigned: 1 }), "set", false);
    expect(same).toContain("1 枚には同じ位置が付いているので変えません。");
    expect(same.join("")).not.toContain("置き換えます");
    expect(describeLocationPlan(plan({ changed: 0, unchanged: 3 }), "set", false)[0]).toBe(
      "位置の変わる写真はありません（対象 3 枚。すでに同じ位置が付いています）。",
    );
    expect(describeLocationPlan(plan({ photos: 0, changed: 0, missing: 2 }), "set", false)).toEqual([
      "対象の写真がありません。",
      "動画 1 本は除きます（動画には位置を付けません）。",
      "見つからない 2 枚は除きます（削除された写真など）。",
    ]);
  });

  it("結果は短く", () => {
    expect(describeLocationPlan(MOCK_LOCATION_PLAN, "set", true)).toEqual(["3 枚に位置を付けました（対象 3 枚）。", "動画 1 本は除きました。"]);
    expect(describeLocationPlan(plan({ changed: 1, skipped_videos: 0 }), "clear", true)).toEqual([
      "1 枚の付けた位置を消しました（対象 3 枚）。",
    ]);
  });

  it("消す見込みと対象の内訳", () => {
    expect(describeLocationPlan(plan({ changed: 1, skipped_videos: 0 }), "clear", false)[0]).toContain(
      "1 枚の付けた位置を消します（対象 3 枚）。カメラの位置がある写真はカメラの位置に",
    );
    expect(describeLocationPlan(plan({ changed: 0 }), "clear", false)[0]).toBe("付けた位置のある写真はありません（対象 3 枚）。");
    expect(describeTargets(MOCK_LOCATION_PLAN)).toEqual([
      "写真 3 枚（カメラの位置がある 1 枚・位置を付けた 1 枚）。",
      "動画 1 本は除きます（動画には位置を付けません）。",
    ]);
    expect(describeTargets(plan({ photos: 0, skipped_videos: 2 }))[0]).toBe("対象の写真がありません。");
  });

  it("履歴と取り消し", () => {
    expect(describeLocationEdit(MOCK_LOCATION_EDITS[1])).toBe("位置を付ける（北緯 35.681236°、東経 139.767125°）");
    expect(describeLocationEdit(MOCK_LOCATION_EDITS[0])).toBe("付けた位置を消す");
    expect(describeLocationUndo({ edit: MOCK_LOCATION_EDITS[1], restored: 3, removed: 0 })).toBe(
      "位置の変更（位置を付ける（北緯 35.681236°、東経 139.767125°））を取り消しました（3 枚）",
    );
    expect(describeLocationUndo({ edit: MOCK_LOCATION_EDITS[0], restored: 1, removed: 2 })).toBe(
      "位置の変更（付けた位置を消す）を取り消しました（1 枚。その後に削除された 2 枚は除きました）",
    );
  });

  it("使う位置の出どころ（右パネルの注記・ダイアログ）", () => {
    expect(gpsSourceNote(MOCK_GEOTAGGED_DETAILS)).toBe("後から付けた位置（カメラの位置: 北緯 35.658600°、東経 139.745400°）");
    expect(gpsSourceNote({ gps_source: "assigned", camera_gps: null })).toBe("後から付けた位置（カメラの位置: なし）");
    expect(gpsSourceNote({ gps_source: "camera", camera_gps: { lat: 1, lon: 2 } })).toBeNull();
    // 古いコア（gps_source が無い）では注記を出さない。
    expect(gpsSourceNote({})).toBeNull();
    expect(locationSummary(MOCK_GEOTAGGED_DETAILS)).toBe("北緯 35.681236°、東経 139.767125°（後から付けた位置）");
    expect(locationSummary({ gps: { lat: 1, lon: 2 }, gps_source: "camera" })).toBe("北緯 1.000000°、東経 2.000000°（カメラの位置）");
    expect(locationSummary({ gps: null, gps_source: null })).toBe("位置なし");
    expect(locationSummary({ gps: { lat: 1, lon: 2 } })).toBe("北緯 1.000000°、東経 2.000000°");
  });
});
