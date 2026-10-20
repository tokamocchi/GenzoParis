// オンラインの地図（位置情報の後付け LIB-21 の第 2 段階。docs/04 の 3.16 節）の設定の保存。
//
// - 保存は genzo-app の専用のコマンド save_map_preferences（すぐ効く。設定の画面の「保存」・「再起動すると有効」
//   とは別）。コマンドは欄をすべて受け取り、欠けた欄を既定値にするので、今の設定に変える欄を重ねて送る
//   （有効にするときに、設定した提供元の URL を消さない）。
// - 初めて有効にした日時（consented_at）はアプリが入れる（画面が送った値は使わない）。
// - 保存した後のアプリの設定で、アプリの状態（status.settings）を置き換える（位置を付けるダイアログの地図と
//   設定の画面が同じ値を見る）。
// - OSM のページ（帰属の表示のリンク先・利用ポリシーなど）は、WebView の中ではなく既定のブラウザで開く
//   （openMapPage。開けるのは genzo-app の MapLink の決めたページだけ）。

import * as app from "../api/app";
import type { AppSettings, MapLink, MapPreferences } from "../api/types";
import { mapPreferencesOf, withMapPreferences } from "../library/geoMapLogic";
import { actions, appStore } from "./app";

/** オンラインの地図の今の設定（古いアプリでは既定値。無効）。 */
export function currentMapPreferences(): MapPreferences {
  return mapPreferencesOf(appStore.get().status?.settings);
}

/** オンラインの地図の設定の一部を変えて保存する（失敗は投げる。ApiError）。 */
export async function saveMapPreferences(patch: Partial<MapPreferences>): Promise<AppSettings> {
  const settings = await app.saveMapPreferences(withMapPreferences(currentMapPreferences(), patch));
  const cur = appStore.get().status;
  if (cur) actions.setStatus({ ...cur, settings });
  return settings;
}

/** OSM のページを既定のブラウザで開く（開けなければ知らせる。オンラインの地図が無効でも開ける）。 */
export function openMapPage(kind: MapLink): void {
  app.openMapLink(kind).catch((e: unknown) => actions.reportError(e, "ページを開けません"));
}
