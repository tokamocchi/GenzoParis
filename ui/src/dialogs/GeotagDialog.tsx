// 位置情報の後付けのダイアログ（LIB-21。ジオタグ付け。docs/04 の 3.16 節）。
//
// - 対象: 操作の対象の写真（グリッドでは選択中のすべて、ルーペでは表示中の 1 枚。actions.targets）。同じ写真の
//   仮想コピーと RAW＋JPEG の組は位置を共有する。動画は除く（コアが除いて数を返す）。
// - 座標を貼り付ける（Google マップで地点を右クリックしてコピーした「35.681236, 139.767125」・度分秒・北緯と東経・
//   Google マップの URL など。library/geotagLogic.ts の parseCoordinates。genzo-model と同じ規則）。入力するたびに
//   このパソコンの中で読み、読んだ位置か誤りを出す（座標そのものは外部に送らない。ただしオンラインの地図が有効なら、
//   確定した座標へ地図を動かし、その付近のタイルを提供元に要求するので、欄の下の注記でそう書く。geoMapLogic.ts の
//   coordinateNote）。
// - 読めたら、コアに見込み（変わる数・カメラの位置がある数・除く動画の数）を問い合わせる（何も変えない）。
// - 「位置を付ける」「付けた位置を消す（カメラの位置に戻す）」はカタログの中だけを変え、元のファイルには書かない。
//   最後の変更から順に「直前の変更を取り消す」で戻せる。
// - 地図と地名の検索（第 2 段階。オンラインの地図）は「位置」の節の座標の欄の上に置く（library/GeoMap.tsx）。地図の
//   クリック・ピンのドラッグ・検索の結果の選択で setCoordinate を呼び、座標の欄に入れる（見込み・付ける操作は座標の
//   欄の値をそのまま使う。読めた座標がピンになる）。地図を動かすのは、座標の欄を確定したとき（Enter・欄を離れる・
//   貼り付けた後）と setCoordinate のときだけ（打つ途中で読める値になるたびに動かすと、見るつもりのない場所の
//   タイルを提供元に要求するため。geoMapLogic.ts の coordFieldReducer）。
//   - オンラインの地図が無効（既定）なら通信しない。地図の代わりに 1 行の案内を出し、押すと有効にする前の説明と
//     確認（MapConsent.tsx）を出す。無効のままでも座標の貼り付けで付けられる（docs/02 の SEC-01・SEC-02）。
//   - 地名の検索（Nominatim）は Enter か「検索」のボタンのときだけ送る（入力の途中では送らない。オートコンプリートの
//     禁止。IME の変換を確定する Enter は除く）。前の検索が終わるまでは次の検索を受け付けず、前の要求の開始から
//     1 秒あけて送る（library/geoMapLogic.ts の searchWaitMs。genzo-geo もプロセス全体で 1 秒に 1 回にし、結果を
//     キャッシュする）。座標の形の語は送らず、読めれば座標の欄に入れる（geoMapLogic.ts の searchInput）。
//   - 開いたときのフォーカスは座標の欄（data-autofocus。地名の検索の欄が先にあるが、座標を貼って Enter で付ける
//     主な使い方で、検索に送らないため）。

import { useEffect, useReducer, useRef, useState } from "react";

import * as app from "../api/app";
import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type {
  ErrorInfo,
  GpsCoord,
  LocationEditInfo,
  LocationEditPlan,
  MapPreferences,
  Place,
  VariantDetails,
  VariantId,
} from "../api/types";
import { Dialog } from "../components/Dialog";
import { GeoMap, type MapFocus } from "../library/GeoMap";
import {
  coordFieldReducer,
  coordinateNote,
  describePlaces,
  EMPTY_COORD_FIELD,
  mapPreferencesOf,
  placeLabel,
  placeView,
  searchInput,
  searchWaitMs,
} from "../library/geoMapLogic";
import {
  coordErrorMessage,
  describeLocationEdit,
  describeLocationPlan,
  describeLocationUndo,
  describeTargets,
  locationSummary,
  MAX_LOCATION_EDITS,
  parseCoordinates,
} from "../library/geotagLogic";
import { formatGps, gpsDecimal } from "../library/metadataLogic";
import { actions, useApp } from "../state/app";
import { saveMapPreferences } from "../state/map";
import { MapConsentPanel } from "./MapConsent";
import { Busy, ErrorBox } from "./parts";

/** 見込みを問い合わせるまでの時間（ms。入力の途中で問い合わせすぎない。コアへの問い合わせで、外部への通信ではない。仮置き）。 */
const PREVIEW_DELAY_MS = 300;

/** 貼り付けられる形の案内（library/geotagLogic.ts の冒頭の一覧の主なもの）。 */
const FORMATS_HINT =
  "貼り付けられる形: Google マップで地点を右クリックしてコピーした値（35.681236, 139.767125）・度分秒" +
  "（35°40'52.4\"N 139°46'01.7\"E）・北緯と東経（北緯 35.681236°、東経 139.767125°）・Google マップの場所の URL" +
  "（/place/…。ピンの位置を読みます）。URL の @緯度,経度 は地図の中心で、ピンとずれることがあります（場所の URL か、" +
  "ピンを右クリックしてコピーした座標を使ってください）。";

export function GeotagDialog(props: { variantIds: VariantId[]; current: VariantId | null }) {
  const ids = props.variantIds;
  const idsKey = ids.join(",");
  // 座標の欄の文字と、地図を動かす先（確定した座標。打つ途中の値では地図を動かさない。geoMapLogic.ts の
  // coordFieldReducer）。
  const [field, dispatchField] = useReducer(coordFieldReducer, EMPTY_COORD_FIELD);
  const text = field.text;
  const [ref, setRef] = useState<VariantDetails | null>(null);
  // 対象の内訳（付けた位置を消す見込み。「付けた位置を消す」の数にも使う）。
  const [targets, setTargets] = useState<LocationEditPlan | null>(null);
  const [history, setHistory] = useState<LocationEditInfo[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);

  const [plan, setPlan] = useState<LocationEditPlan | null>(null);
  const [planError, setPlanError] = useState<ErrorInfo | null>(null);
  const [counting, setCounting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const seq = useRef(0);
  const composing = useRef(false);
  // オンラインの地図の設定（オブジェクトは保存するまで同じものなので、そのまま購読する）。
  const mapSettings = useApp((s) => s.status?.settings.map);
  const mapPrefs = mapPreferencesOf({ map: mapSettings });

  const parsed = parseCoordinates(text);
  const coord = parsed.ok ? parsed.coord : null;
  const coordKey = coord ? `${coord.lat},${coord.lon}` : "";

  /** 座標の欄に入れる（第 2 段階の地図・検索の結果・写真の位置。すぐ確定して、地図もその位置へ動かす）。 */
  const setCoordinate = (g: GpsCoord) => dispatchField({ kind: "set", text: gpsDecimal(g) });
  /** 座標の欄を確定する（Enter・欄を離れる・貼り付けた後。地図をその位置へ動かす）。 */
  const commitField = () => dispatchField({ kind: "commit" });

  const reloadHistory = () =>
    core
      .locationEdits()
      .then(setHistory)
      .catch(() => setHistory([]));

  // 開いたとき: 表示中の写真の詳細・対象の内訳・履歴を読む。
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const [details, base] = await Promise.all([
          props.current !== null ? core.variantDetails(props.current) : null,
          ids.length > 0 ? core.planLocationEdit(ids, null) : null,
        ]);
        if (!alive) return;
        setRef(details);
        setTargets(base);
      } catch (e) {
        if (alive) setLoadError(actions.describeError(e));
      }
    })();
    void reloadHistory();
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.current, idsKey]);

  // 座標が変わったら見込みを問い合わせる。
  useEffect(() => {
    const my = ++seq.current;
    setPlan(null);
    setPlanError(null);
    if (!coord || ids.length === 0) {
      setCounting(false);
      return;
    }
    setCounting(true);
    const t = setTimeout(() => {
      core
        .planLocationEdit(ids, coord)
        .then((p) => {
          if (my === seq.current) setPlan(p);
        })
        .catch((e: unknown) => {
          if (my === seq.current) setPlanError(toApiError(e).info);
        })
        .finally(() => {
          if (my === seq.current) setCounting(false);
        });
    }, PREVIEW_DELAY_MS);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [coordKey, idsKey]);

  /** 位置を付ける（location が null なら付けた位置を消す）。 */
  const apply = async (location: GpsCoord | null) => {
    setBusy(true);
    setError(null);
    try {
      const r = await core.editLocations(ids, location);
      actions.closeDialog();
      actions.notify(
        r.changed > 0 ? "success" : "info",
        describeLocationPlan(r, location ? "set" : "clear", true).join(" "),
        r.changed > 0 ? "元のファイルは変えていません。位置を付けるダイアログの「直前の変更を取り消す」で戻せます。" : undefined,
      );
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };

  const undo = async () => {
    setBusy(true);
    setError(null);
    try {
      const u = await core.undoLocationEdit();
      actions.notify("info", u ? describeLocationUndo(u) : "取り消す変更はありません");
      await reloadHistory();
      if (props.current !== null) setRef(await core.variantDetails(props.current));
      if (ids.length > 0) setTargets(await core.planLocationEdit(ids, null));
      // 見込みを問い合わせ直す。
      const my = ++seq.current;
      setPlan(null);
      setCounting(false);
      if (coord && ids.length > 0) {
        const p = await core.planLocationEdit(ids, coord);
        if (my === seq.current) setPlan(p);
      }
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };

  const applyDisabled = busy || ids.length === 0 || !coord || !plan || counting || plan.changed === 0;
  const clearDisabled = busy || !targets || targets.changed === 0;
  const invalid = !parsed.ok && parsed.error.kind !== "empty";
  const n = (v: number) => v.toLocaleString("ja-JP");

  return (
    <Dialog
      title="位置を付ける"
      width={680}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button
            className="btn"
            onClick={() => void undo()}
            disabled={busy || history.length === 0}
            title={history[0] ? describeLocationEdit(history[0]) : undefined}
          >
            直前の変更を取り消す
          </button>
          <span className="spacer" />
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => coord && void apply(coord)} disabled={applyDisabled}>
            {busy ? "変えています…" : "位置を付ける"}
          </button>
        </>
      }
    >
      {loadError && <p className="error-text">読み込めません: {loadError}</p>}
      <p className="hint" style={{ marginTop: 0 }}>
        カタログの中の位置だけを変えます（元のファイルには書きません。カメラが記録した位置も残ります）。書き出したファイルの
        Exif には付けた位置が入ります（書き出しの「位置情報（GPS）を削除する」を選ぶと入りません）。動画は対象外です。
      </p>

      <div className="dlg-section">
        <h3>対象</h3>
        {ids.length === 0 && <p className="hint">写真を選んでください（グリッドで選ぶか、ルーペで表示してください）。</p>}
        {ids.length > 0 && !targets && !loadError && <Busy>対象を数えています…</Busy>}
        {targets && <div className="dlg-note">{describeTargets(targets).join("\n")}</div>}
        {ref && (
          <p className="hint">
            表示中の写真: {ref.summary.file_name}（{locationSummary(ref)}）
          </p>
        )}
      </div>

      <div className="dlg-section">
        <h3>位置</h3>
        {mapPrefs.online ? (
          <MapPicker
            pin={coord}
            target={field.target}
            photo={ref?.gps ?? null}
            onPick={setCoordinate}
            disabled={busy}
          />
        ) : (
          <MapOffline preferences={mapPrefs} />
        )}
        <div className="field">
          <label htmlFor="geo-coord">座標（緯度, 経度）</label>
          {/* 開いたときのフォーカスはこの欄（有効なときは地名の検索の欄が先にあるが、座標を貼って Enter で付ける
              主な使い方で、検索に送らないため。Dialog の initialFocusTarget）。 */}
          <input
            id="geo-coord"
            type="text"
            data-autofocus=""
            value={text}
            placeholder="35.681236, 139.767125"
            spellCheck={false}
            autoComplete="off"
            aria-invalid={invalid}
            onChange={(e) => dispatchField({ kind: "input", text: e.target.value })}
            // 貼り付けたら確定する（貼り付けの既定の動作で欄の値が変わった後。打つ途中では地図を動かさない）。
            onPaste={() => {
              setTimeout(commitField, 0);
            }}
            onBlur={commitField}
            onCompositionStart={() => {
              composing.current = true;
            }}
            onCompositionEnd={() => {
              composing.current = false;
            }}
            onKeyDown={(e) => {
              // Enter で付ける（IME の変換の確定は除く）。
              if (composing.current || e.nativeEvent.isComposing || e.keyCode === 229) return;
              if (e.key === "Enter") {
                e.preventDefault();
                // 付けられるなら付ける（ダイアログを閉じるので地図は動かさない）。付けられなければ確定して地図を動かす。
                if (!applyDisabled && coord) void apply(coord);
                else commitField();
              }
            }}
          />
        </div>
        {parsed.ok && <div className="dlg-note">読んだ位置: {formatGps(parsed.coord)}</div>}
        {invalid && !parsed.ok && <p className="dlg-field-error">{coordErrorMessage(parsed.error)}</p>}
        {ref?.gps && (
          <div className="dlg-row">
            <button type="button" className="btn small" onClick={() => ref.gps && setCoordinate(ref.gps)} disabled={busy}>
              表示中の写真の位置を入れる
            </button>
          </div>
        )}
        <p className="hint">
          {FORMATS_HINT}
          <br />
          {coordinateNote(mapPrefs)}
        </p>
      </div>

      <div className="dlg-section">
        <h3>見込み</h3>
        {ids.length > 0 && !coord && <p className="hint">座標を入れると、位置の変わる写真の数を出します。</p>}
        {counting && <Busy>見込みを数えています…</Busy>}
        {plan && !counting && <div className="dlg-note">{describeLocationPlan(plan, "set", false).join("\n")}</div>}
        <ErrorBox error={planError} />
      </div>

      <div className="dlg-section">
        <h3>付けた位置を消す</h3>
        <p className="hint">
          選んだ写真の付けた位置を消して、カメラの位置に戻します（カメラの位置がない写真は位置なしになります）。
        </p>
        {targets && targets.changed === 0 && targets.photos > 0 && <p className="hint">付けた位置のある写真はありません。</p>}
        <div className="dlg-row">
          <button type="button" className="btn" onClick={() => void apply(null)} disabled={clearDisabled}>
            付けた位置を消す（カメラの位置に戻す）{targets && targets.changed > 0 ? `: ${n(targets.changed)} 枚` : ""}
          </button>
        </div>
      </div>

      {history.length > 0 && (
        <p className="hint">
          直前の変更: {describeLocationEdit(history[0])}（{n(history[0].asset_count)} 枚）。変更の履歴は {n(history.length)} 件（新しい{" "}
          {n(MAX_LOCATION_EDITS)} 件まで残します）。
        </p>
      )}
      <ErrorBox error={error} />
    </Dialog>
  );
}

/** 地名の検索と地図（オンラインの地図が有効なとき）。 */
function MapPicker(props: {
  /** 付ける位置の印（座標の欄で読めた値）。 */
  pin: GpsCoord | null;
  /** 地図を動かす先（確定した座標）。 */
  target: GpsCoord | null;
  photo: GpsCoord | null;
  onPick: (g: GpsCoord) => void;
  disabled: boolean;
}) {
  const [query, setQuery] = useState("");
  const [places, setPlaces] = useState<Place[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  // 検索の欄の語を座標として読んだときの案内。
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [focus, setFocus] = useState<MapFocus | null>(null);
  // 前の検索の要求を送った時刻（ms）。
  const lastStart = useRef<number | null>(null);
  // 検索の途中か（待っている間も。続けて押されたときに、描き直す前でも 2 つ目を送らない）。
  const inFlight = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const alive = useRef(true);
  const composing = useRef(false);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      if (timer.current !== null) clearTimeout(timer.current);
      timer.current = null;
      inFlight.current = false;
    };
  }, []);

  /**
   * 検索する（Enter か「検索」のボタンのときだけ。前の検索の途中なら何もしない）。座標の形の語は送らず、読めれば
   * 座標の欄に入れる（geoMapLogic.ts の searchInput）。
   */
  const search = () => {
    if (inFlight.current || props.disabled) return;
    const input = searchInput(query);
    setProblem(input.kind === "problem" ? input.message : null);
    setNotice(null);
    if (input.kind === "problem") return;
    if (input.kind === "coordinate") {
      setNotice("座標として読み、座標の欄に入れました（地名の検索には送っていません）。");
      props.onPick(input.coord);
      return;
    }
    const q = input.query;
    inFlight.current = true;
    setSearching(true);
    setError(null);
    timer.current = setTimeout(() => {
      timer.current = null;
      lastStart.current = Date.now();
      app
        .searchPlaces(q)
        .then((r) => {
          if (alive.current) setPlaces(r);
        })
        .catch((e: unknown) => {
          if (!alive.current) return;
          setPlaces(null);
          setError(toApiError(e).info);
        })
        .finally(() => {
          inFlight.current = false;
          if (alive.current) setSearching(false);
        });
    }, searchWaitMs(lastStart.current, Date.now()));
  };

  /** 検索の結果を選ぶ: 地図をその場所へ動かし、ピンを置く（座標の欄に入れる）。 */
  const choose = (p: Place) => {
    setFocus((f) => ({ seq: (f?.seq ?? 0) + 1, view: placeView(p) }));
    props.onPick({ lat: p.lat, lon: p.lon });
  };

  return (
    <div>
      <div className="dlg-row">
        <input
          type="search"
          className="dlg-grow"
          value={query}
          placeholder="地名・駅名・施設名で検索（例: 東京駅）"
          aria-label="地名の検索"
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => {
            // 入力の途中では検索しない（オートコンプリートの禁止）。誤りの表示だけ消す。
            setQuery(e.target.value);
            setProblem(null);
            setNotice(null);
          }}
          onCompositionStart={() => {
            composing.current = true;
          }}
          onCompositionEnd={() => {
            composing.current = false;
          }}
          onKeyDown={(e) => {
            // IME の変換を確定する Enter では検索しない。
            if (composing.current || e.nativeEvent.isComposing || e.keyCode === 229) return;
            if (e.key === "Enter") {
              e.preventDefault();
              search();
            }
          }}
        />
        <button type="button" className="btn" onClick={search} disabled={searching || props.disabled}>
          {searching ? "検索しています…" : "検索"}
        </button>
      </div>
      {problem && <p className="dlg-field-error">{problem}</p>}
      {notice && <p className="hint">{notice}</p>}
      <ErrorBox error={error} title="検索できません" />
      {places && <p className="hint">{describePlaces(places)}</p>}
      {places && places.length > 0 && (
        <ul className="geo-places">
          {places.map((p, i) => {
            const label = placeLabel(p);
            return (
              <li key={`${i}:${p.lat},${p.lon}`}>
                <button type="button" className="geo-place" onClick={() => choose(p)} disabled={props.disabled}>
                  <span className="title">{label.title}</span>
                  {label.detail && <span className="detail">{label.detail}</span>}
                </button>
              </li>
            );
          })}
        </ul>
      )}
      <GeoMap
        pin={props.pin}
        target={props.target}
        photo={props.photo}
        focus={focus}
        onPick={props.onPick}
        disabled={props.disabled}
      />
      <p className="hint geo-legend">
        <span>
          <span className="geo-legend-pin" />
          付ける位置（地図をクリックするか、ピンをドラッグして選びます）
        </span>
        {props.photo && (
          <span>
            <span className="geo-legend-photo" />
            表示中の写真の位置
          </span>
        )}
      </p>
    </div>
  );
}

/** オンラインの地図が無効なとき: 1 行の案内と、押すと出す有効にする前の説明と確認。 */
function MapOffline(props: { preferences: MapPreferences }) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  const enable = async () => {
    setBusy(true);
    setError(null);
    try {
      await saveMapPreferences({ online: true });
      actions.notify("success", "オンラインの地図を有効にしました", "設定の「一般」でいつでも無効にできます。");
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };

  if (!open) {
    return (
      <div className="dlg-row" style={{ marginBottom: 6 }}>
        <span className="hint dlg-grow">
          地図で選ぶ・地名で検索するには、オンラインの地図（OpenStreetMap）を有効にします（今は無効で、外部と通信しません）。
        </span>
        <button type="button" className="btn small" onClick={() => setOpen(true)}>
          オンラインの地図について…
        </button>
      </div>
    );
  }
  return (
    <MapConsentPanel
      preferences={props.preferences}
      busy={busy}
      error={error}
      onEnable={() => void enable()}
      onCancel={() => setOpen(false)}
    />
  );
}
