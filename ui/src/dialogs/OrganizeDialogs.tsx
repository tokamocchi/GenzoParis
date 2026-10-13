// キーワード（LIB-09）・コレクション（LIB-10）のダイアログ: 作る・名前の変更・移動・削除の確認、写真を入れる
// コレクションを選ぶ。スマートコレクションの条件の編集は SmartCollectionDialog.tsx。
//
// - 名前の重なり（同じ階層の同じ名前。全角・半角、大文字・小文字の違いを含む）はコアが確かめ、種類 conflict の
//   エラーをダイアログの中に出す（閉じない）。
// - 削除は写真も元のファイルも消さない（キーワードの付与・コレクションのメンバーの記録だけ）ことを、確認の文で
//   示す。

import { useEffect, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type {
  CollectionId,
  CollectionInfo,
  CollectionKind,
  ErrorInfo,
  KeywordId,
  KeywordInfo,
  KeywordUsage,
  VariantId,
} from "../api/types";
import { Dialog } from "../components/Dialog";
import {
  COLLECTION_KIND_LABEL,
  collectionPathText,
  deleteCollectionMessage,
  manualCollections,
  parentChoices,
  validateCollectionName,
} from "../library/collectionLogic";
import {
  deleteKeywordMessage,
  keywordPath,
  keywordPathText,
  moveTargets,
  parseKeywordInput,
  validateKeywordName,
} from "../library/keywordLogic";
import { actions, appStore } from "../state/app";
import { setTargetCollection } from "../state/organize";
import { ErrorBox } from "./parts";

/** コアの呼び出し（失敗はダイアログの中に出す）。成功したら閉じる。 */
function useRunner() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await f();
      actions.closeDialog();
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, run };
}

/** 一覧を読む（開いたときに 1 回）。 */
function useList<T>(load: () => Promise<T>): { data: T | null; error: string | null } {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    load()
      .then((d) => alive && setData(d))
      .catch((e: unknown) => alive && setError(actions.describeError(e)));
    return () => {
      alive = false;
    };
    // 開いたときに 1 回だけ読む。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return { data, error };
}

function Buttons(props: { busy: boolean; ok: string; disabled?: boolean; onOk: () => void; danger?: boolean }) {
  return (
    <>
      <button className="btn" onClick={actions.closeDialog} disabled={props.busy}>
        キャンセル
      </button>
      <button className={`btn ${props.danger ? "danger" : "primary"}`} onClick={props.onOk} disabled={props.busy || props.disabled}>
        {props.ok}
      </button>
    </>
  );
}

// ---------------------------------------------------------------------------
// キーワード
// ---------------------------------------------------------------------------

export function KeywordDialog(props: { mode: "create" | "rename" | "move"; keywordId?: KeywordId; parentId?: KeywordId | null }) {
  const { data: list, error: loadError } = useList<KeywordInfo[]>(core.keywordTree);
  const { busy, error, run } = useRunner();
  const me = list?.find((k) => k.keyword_id === props.keywordId) ?? null;
  const [name, setName] = useState<string | null>(null);
  const [parent, setParent] = useState<string | null>(null);
  const value = name ?? (props.mode === "rename" ? (me?.name ?? "") : "");
  const parentPath = props.parentId != null && list ? keywordPathText(keywordPath(list, props.parentId)) : null;

  let title: string;
  let body;
  let ok: () => void;
  let disabled = false;
  if (props.mode === "create") {
    title = "キーワードを作る";
    // 「,」「、」は入力欄の区切りなので、ここでも 1 つのパスだけを受け付ける（複数なら作らない）。
    const paths = parseKeywordInput(value);
    // 区切りは parseKeywordInput が除くので、ここで確かめるのは各段の長さ・制御文字。
    const problem = paths.length === 1 ? (paths[0].map(validateKeywordName).find((p) => p !== null) ?? null) : null;
    disabled = paths.length !== 1 || problem !== null;
    ok = () =>
      void run(async () => {
        await core.createKeyword(props.parentId ?? null, paths[0]);
      });
    body = (
      <>
        <p className="muted" style={{ marginTop: 0 }}>
          場所: {parentPath ?? "（最上位）"}
        </p>
        <div className="field">
          <label htmlFor="kw-name">名前</label>
          <input id="kw-name" autoFocus value={value} onChange={(e) => setName(e.target.value)} placeholder="例: 京都（「場所 > 京都」で階層ごと作れます）" />
        </div>
        {paths.length > 1 && <p className="hint">1 つずつ作ってください（「,」「、」は区切りとして扱います）。</p>}
        {problem && <p className="hint">{problem}</p>}
      </>
    );
  } else if (props.mode === "rename") {
    title = "キーワードの名前を変える";
    // 名前の変更は 1 つの段の名前なので、区切りの文字（「>」「,」など）を含む名前は受け付けない（コアと同じ規則）。
    const problem = value.trim() === "" ? null : validateKeywordName(value);
    disabled = value.trim() === "" || value.trim() === me?.name || problem !== null;
    ok = () =>
      void run(async () => {
        await core.renameKeyword(props.keywordId!, value.trim());
      });
    body = (
      <>
        <div className="field">
          <label htmlFor="kw-name">名前</label>
          <input id="kw-name" autoFocus value={value} onChange={(e) => setName(e.target.value)} />
        </div>
        {problem && <p className="hint">{problem}</p>}
      </>
    );
  } else {
    title = "キーワードを移動";
    const choices = list && props.keywordId !== undefined ? moveTargets(list, props.keywordId) : [];
    const current = me?.parent_id === null || me?.parent_id === undefined ? "root" : String(me.parent_id);
    const chosen = parent ?? current;
    disabled = chosen === current;
    ok = () =>
      void run(async () => {
        await core.moveKeyword(props.keywordId!, chosen === "root" ? null : Number(chosen));
      });
    body = (
      <>
        <p className="muted" style={{ marginTop: 0 }}>
          {me ? keywordPathText(keywordPath(list ?? [], me.keyword_id)) : ""}
        </p>
        <div className="field">
          <label htmlFor="kw-parent">移動先</label>
          <select id="kw-parent" value={chosen} onChange={(e) => setParent(e.target.value)}>
            {choices.map((c) => (
              <option key={c.id ?? "root"} value={c.id === null ? "root" : String(c.id)}>
                {c.label}
              </option>
            ))}
          </select>
        </div>
      </>
    );
  }
  return (
    <Dialog
      title={title}
      width={460}
      onClose={actions.closeDialog}
      busy={busy}
      actions={<Buttons busy={busy} ok={props.mode === "create" ? "作る" : props.mode === "rename" ? "変える" : "移動"} disabled={disabled || !list} onOk={ok} />}
    >
      {loadError && <p className="error-text">キーワードの一覧を読めません: {loadError}</p>}
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!disabled && !busy) ok();
        }}
      >
        {body}
      </form>
      <ErrorBox error={error} />
    </Dialog>
  );
}

export function KeywordDeleteDialog(props: { keywordId: KeywordId }) {
  const { data, error: loadError } = useList<{ usage: KeywordUsage; list: KeywordInfo[] }>(async () => ({
    usage: await core.keywordUsage(props.keywordId),
    list: await core.keywordTree(),
  }));
  const { busy, error, run } = useRunner();
  const name = data ? keywordPathText(keywordPath(data.list, props.keywordId)) : "";
  return (
    <Dialog
      title="キーワードを削除"
      width={480}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <Buttons
          busy={busy}
          danger
          ok="削除"
          disabled={!data}
          onOk={() =>
            void run(async () => {
              const n = await core.deleteKeyword(props.keywordId);
              // 絞り込みに使っていたら外す。
              const kws = appStore.get().filter.keywords ?? [];
              if (kws.length > 0) {
                const alive = new Set((await core.keywordTree()).map((k) => k.keyword_id));
                const left = kws.filter((k) => alive.has(k));
                if (left.length !== kws.length) actions.setFilter({ keywords: left });
              }
              actions.notify("info", `キーワードを ${n.toLocaleString("ja-JP")} 個削除しました`);
            })
          }
        />
      }
    >
      {loadError && <p className="error-text">キーワードを読めません: {loadError}</p>}
      {data && deleteKeywordMessage(name, data.usage).map((l) => <p key={l}>{l}</p>)}
      <ErrorBox error={error} />
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// コレクション
// ---------------------------------------------------------------------------

export function CollectionDialog(props: {
  mode: "create" | "rename" | "move";
  collectionKind?: CollectionKind;
  collectionId?: CollectionId;
  parentId?: CollectionId | null;
  variantIds?: VariantId[];
}) {
  const { data: list, error: loadError } = useList<CollectionInfo[]>(() => core.collections(false));
  const { busy, error, run } = useRunner();
  const me = list?.find((c) => c.collection_id === props.collectionId) ?? null;
  const kind: CollectionKind = props.collectionKind ?? me?.kind ?? "manual";
  const [name, setName] = useState<string | null>(null);
  const [parent, setParent] = useState<string | null>(null);
  const [include, setInclude] = useState(true);
  const [makeTarget, setMakeTarget] = useState(false);
  const value = name ?? (props.mode === "rename" ? (me?.name ?? "") : "");
  const photos = props.variantIds ?? [];
  const initialParent = props.mode === "move" ? (me?.parent_id ?? null) : (props.parentId ?? null);
  const chosenParent = parent ?? (initialParent === null ? "root" : String(initialParent));
  const parentValue = chosenParent === "root" ? null : Number(chosenParent);
  const choices = list ? parentChoices(list, props.mode === "move" ? (props.collectionId ?? null) : null) : [];
  const nameProblem = props.mode === "move" ? null : validateCollectionName(value);

  const ok = () =>
    void run(async () => {
      if (props.mode === "create") {
        const id = await core.createCollection(parentValue, value.trim(), kind, null, kind === "manual" && include ? photos : []);
        if (kind === "manual" && makeTarget) setTargetCollection(id);
        actions.notify(
          "success",
          kind === "manual" && include && photos.length > 0
            ? `コレクション「${value.trim()}」を作り、${photos.length.toLocaleString("ja-JP")} 枚を入れました`
            : `${COLLECTION_KIND_LABEL[kind]}「${value.trim()}」を作りました`,
        );
      } else if (props.mode === "rename") {
        await core.renameCollection(props.collectionId!, value.trim());
      } else {
        await core.moveCollection(props.collectionId!, parentValue);
      }
    });
  const title =
    props.mode === "create"
      ? `${COLLECTION_KIND_LABEL[kind]}を作る`
      : props.mode === "rename"
        ? `${COLLECTION_KIND_LABEL[kind]}の名前を変える`
        : `${COLLECTION_KIND_LABEL[kind]}を移動`;
  const disabled =
    !list ||
    nameProblem !== null ||
    (props.mode === "rename" && value.trim() === me?.name) ||
    (props.mode === "move" && parentValue === (me?.parent_id ?? null));
  return (
    <Dialog
      title={title}
      width={480}
      onClose={actions.closeDialog}
      busy={busy}
      actions={<Buttons busy={busy} ok={props.mode === "create" ? "作る" : props.mode === "rename" ? "変える" : "移動"} disabled={disabled} onOk={ok} />}
    >
      {loadError && <p className="error-text">コレクションの一覧を読めません: {loadError}</p>}
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!disabled && !busy) ok();
        }}
      >
        {props.mode !== "move" && (
          <div className="field">
            <label htmlFor="col-name">名前</label>
            <input id="col-name" autoFocus value={value} onChange={(e) => setName(e.target.value)} />
          </div>
        )}
        {props.mode !== "rename" && (
          <div className="field">
            <label htmlFor="col-parent">{props.mode === "move" ? "移動先" : "場所"}</label>
            <select id="col-parent" value={chosenParent} onChange={(e) => setParent(e.target.value)}>
              {choices.map((c) => (
                <option key={c.id ?? "root"} value={c.id === null ? "root" : String(c.id)}>
                  {c.label}
                </option>
              ))}
            </select>
          </div>
        )}
        {props.mode === "create" && kind === "manual" && photos.length > 0 && (
          <label className="dlg-check">
            <input type="checkbox" checked={include} onChange={(e) => setInclude(e.target.checked)} />
            操作の対象の写真 {photos.length.toLocaleString("ja-JP")} 枚を入れる
          </label>
        )}
        {props.mode === "create" && kind === "manual" && (
          <label className="dlg-check">
            <input type="checkbox" checked={makeTarget} onChange={(e) => setMakeTarget(e.target.checked)} />
            ターゲットコレクションにする（B で写真を入れる・外す）
          </label>
        )}
        {props.mode === "move" && me && <p className="muted">{collectionPathText(list ?? [], me.collection_id)}</p>}
      </form>
      <ErrorBox error={error} />
    </Dialog>
  );
}

export function CollectionDeleteDialog(props: { collectionId: CollectionId }) {
  const { data: list, error: loadError } = useList<CollectionInfo[]>(() => core.collections(false));
  const { busy, error, run } = useRunner();
  return (
    <Dialog
      title="コレクションを削除"
      width={480}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <Buttons
          busy={busy}
          danger
          ok="削除"
          disabled={!list}
          onOk={() =>
            void run(async () => {
              const n = await core.deleteCollection(props.collectionId);
              const s = appStore.get();
              // 表示中・ターゲットのコレクション（中のものを含む）を消したら、外す。
              const alive = new Set((await core.collections(false)).map((c) => c.collection_id));
              if (s.filter.collection_id != null && !alive.has(s.filter.collection_id)) actions.showAll();
              if (s.targetCollection !== null && !alive.has(s.targetCollection)) setTargetCollection(null);
              actions.notify("info", `コレクションを ${n.toLocaleString("ja-JP")} 個削除しました（写真は削除していません）`);
            })
          }
        />
      }
    >
      {loadError && <p className="error-text">コレクションを読めません: {loadError}</p>}
      {list && deleteCollectionMessage(list, props.collectionId).map((l) => <p key={l}>{l}</p>)}
      <ErrorBox error={error} />
    </Dialog>
  );
}

/** 写真を入れるコレクションを選ぶ（B でターゲットがないとき・メニュー）。新しいコレクションも作れる。 */
export function AddToCollectionDialog(props: { variantIds: VariantId[] }) {
  const { data: list, error: loadError } = useList<CollectionInfo[]>(() => core.collections(false));
  const { busy, error, run } = useRunner();
  const [choice, setChoice] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [makeTarget, setMakeTarget] = useState(true);
  const manual = list ? manualCollections(list) : [];
  const chosen = choice ?? (manual.length > 0 ? String(manual[0].id) : "new");
  const creating = chosen === "new";
  const disabled = !list || props.variantIds.length === 0 || (creating && validateCollectionName(newName) !== null);
  const ok = () =>
    void run(async () => {
      let id: CollectionId;
      let name: string;
      let added: number;
      if (creating) {
        name = newName.trim();
        id = await core.createCollection(null, name, "manual", null, props.variantIds);
        added = props.variantIds.length;
      } else {
        id = Number(chosen);
        name = manual.find((m) => m.id === id)?.label ?? "";
        added = await core.addToCollection(id, props.variantIds);
      }
      if (makeTarget) setTargetCollection(id);
      actions.notify(
        "success",
        added > 0 ? `${added.toLocaleString("ja-JP")} 枚を「${name}」に入れました` : `すでに「${name}」に入っています`,
      );
    });
  return (
    <Dialog
      title={`コレクションに入れる（${props.variantIds.length.toLocaleString("ja-JP")} 枚）`}
      width={480}
      onClose={actions.closeDialog}
      busy={busy}
      actions={<Buttons busy={busy} ok="入れる" disabled={disabled} onOk={ok} />}
    >
      {loadError && <p className="error-text">コレクションの一覧を読めません: {loadError}</p>}
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!disabled && !busy) ok();
        }}
      >
        <div className="field">
          <label htmlFor="add-col">コレクション</label>
          <select id="add-col" value={chosen} onChange={(e) => setChoice(e.target.value)}>
            {manual.map((m) => (
              <option key={m.id} value={String(m.id)}>
                {m.label}
              </option>
            ))}
            <option value="new">（新しいコレクションを作る）</option>
          </select>
        </div>
        {creating && (
          <div className="field">
            <label htmlFor="add-col-name">新しい名前</label>
            <input id="add-col-name" autoFocus value={newName} onChange={(e) => setNewName(e.target.value)} />
          </div>
        )}
        <label className="dlg-check">
          <input type="checkbox" checked={makeTarget} onChange={(e) => setMakeTarget(e.target.checked)} />
          ターゲットコレクションにする（次からは B で入れる・外す）
        </label>
      </form>
      <ErrorBox error={error} />
    </Dialog>
  );
}
