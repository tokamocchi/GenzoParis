// 説明のメタデータの編集（LIB-15）: タイトル・キャプション・作成者と連絡先・著作権・撮影場所など。
//
// - カタログの中だけに保存する（元のファイルには書かない。書き出したファイルには XMP と Exif で入る）。
// - 対象（targets）は、グリッドでは選択中のすべて（一括編集）、ルーペでは表示中の 1 枚（actions.targets）。
//   複数のときは、写真によって値が異なる項目を「（複数の値）」と出し、入力した項目だけをすべてに書く。
// - 入力欄はフォーカスを外したとき・Enter（1 行の項目）・Ctrl / ⌘ ＋ Enter（複数行の項目）で保存する（IME の
//   変換を確定する Enter では保存しない）。Esc で編集を取り消す。空にすると削除。著作権の状態は選んだときに
//   保存する。対象が変わったとき（欄が消えるとき）・カタログを閉じる前にも、編集中の内容を保存する
//   （MVP のキャプションの欄と同じ）。

import { useEffect, useRef, useState } from "react";

import * as core from "../api/core";
import type { MetadataField, VariantId } from "../api/types";
import { Section } from "../components/Section";
import { comboLabel } from "../keyboard";
import { actions } from "../state/app";
import { registerFlush } from "../state/flush";
import {
  COPYRIGHT_STATUS_OPTIONS,
  type FieldState,
  type FieldStates,
  MAX_METADATA_CHARS,
  METADATA_GROUPS,
  type MetadataFieldDef,
  fieldHint,
  filledCount,
  normalizeDraft,
  tooLong,
} from "./metadataEdit";

export function MetadataEditor(props: {
  /** 書く対象（変わったら欄を作り直す。呼び出し側で key を付ける）。 */
  targets: VariantId[];
  states: FieldStates;
  /** 新しい対象の値が届くまで（前の写真の値で新しい対象を変えないように）。 */
  disabled: boolean;
  /** 一括編集の説明（複数のとき）。 */
  note?: string | null;
}) {
  return (
    <>
      {props.note && <p className="hint lib-meta-note">{props.note}</p>}
      {METADATA_GROUPS.map((g) => {
        const filled = filledCount(props.states, g);
        return (
          <Section key={g.id} id={`lib.meta.iptc.${g.id}`} title={filled > 0 ? `${g.title}（${filled}）` : g.title}>
            <div className="lib-iptc">
              {g.fields.map((d) =>
                d.select ? (
                  <StatusSelect key={d.field} def={d} targets={props.targets} state={props.states[d.field]} disabled={props.disabled} />
                ) : (
                  <FieldEditor key={d.field} def={d} targets={props.targets} state={props.states[d.field]} disabled={props.disabled} />
                ),
              )}
            </div>
          </Section>
        );
      })}
    </>
  );
}

/** 保存（失敗したら知らせて例外を返す）。 */
async function save(targets: VariantId[], field: MetadataField, value: string | null): Promise<void> {
  try {
    await core.setMetadata(targets, { [field]: value });
  } catch (e) {
    actions.reportError(e, "メタデータを保存できません");
    throw e;
  }
}

function StatusSelect(props: { def: MetadataFieldDef; targets: VariantId[]; state: FieldState; disabled: boolean }) {
  const { def, state } = props;
  const [saving, setSaving] = useState(false);
  const value = state.mixed ? "__mixed" : (state.value ?? "");
  return (
    <label className="lib-iptc-row">
      <span className="label">{def.label}</span>
      <select
        value={value}
        disabled={props.disabled || saving}
        aria-label={def.label}
        onChange={(e) => {
          const v = e.target.value;
          if (v === "__mixed") return;
          setSaving(true);
          save(props.targets, def.field, v === "" ? null : v)
            .catch(() => undefined)
            .finally(() => setSaving(false));
        }}
      >
        {state.mixed && <option value="__mixed">（複数の値）</option>}
        {COPYRIGHT_STATUS_OPTIONS.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}

function FieldEditor(props: { def: MetadataFieldDef; targets: VariantId[]; state: FieldState; disabled: boolean }) {
  const { def, state } = props;
  const shown = state.value ?? "";
  const [draft, setDraft] = useState(shown);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dirty = useRef(false);
  const composing = useRef(false);
  const latest = useRef({ targets: props.targets, draft, shown });
  latest.current = { targets: props.targets, draft, shown };
  /** 保存の列（保存が重なっても、後に保存した値が最後に書かれるように順に送る）。 */
  const chain = useRef<Promise<void>>(Promise.resolve());
  /** 最後の保存の失敗（成功したら null）。閉じる前の書き込みで、失敗を伝えるため。 */
  const failure = useRef<unknown>(null);

  // 保存した値（読み直した詳細・まとめ）が届いたら、編集していなければ合わせる。
  useEffect(() => {
    if (!dirty.current) setDraft(state.value ?? "");
  }, [state.value, state.mixed]);

  const commit = (): Promise<void> => {
    if (dirty.current) {
      const { targets, draft: text } = latest.current;
      if (tooLong(text)) {
        setError(`長すぎます（上限 ${MAX_METADATA_CHARS.toLocaleString("ja-JP")} 文字）`);
        return chain.current;
      }
      dirty.current = false;
      setError(null);
      const value = normalizeDraft(text);
      chain.current = chain.current.then(async () => {
        setSaving(true);
        try {
          await save(targets, def.field, value);
          failure.current = null;
        } catch (e) {
          dirty.current = true;
          failure.current = e;
        } finally {
          setSaving(false);
        }
      });
    }
    return chain.current;
  };

  const commitRef = useRef(commit);
  commitRef.current = commit;
  useEffect(() => {
    const unregister = registerFlush(async () => {
      await commitRef.current();
      if (dirty.current && failure.current !== null) throw failure.current;
    });
    return () => {
      unregister();
      void commitRef.current();
    };
  }, []);

  const common = {
    value: draft,
    disabled: props.disabled,
    placeholder: state.mixed ? "（複数の値）" : undefined,
    "aria-label": def.label,
    title: fieldHint(def),
    onChange: (e: { target: { value: string } }) => {
      dirty.current = true;
      setError(null);
      setDraft(e.target.value);
    },
    onCompositionStart: () => {
      composing.current = true;
    },
    onCompositionEnd: () => {
      composing.current = false;
    },
    onBlur: () => void commit(),
    onKeyDown: (e: React.KeyboardEvent<HTMLInputElement | HTMLTextAreaElement>) => {
      if (composing.current || e.nativeEvent.isComposing || e.keyCode === 229) return;
      const shouldSave = def.multiline ? e.key === "Enter" && (e.ctrlKey || e.metaKey) : e.key === "Enter";
      if (shouldSave) {
        e.preventDefault();
        void commit();
      } else if (e.key === "Escape") {
        // 編集を取り消す（グリッドに戻る Esc にしない）。
        e.stopPropagation();
        dirty.current = false;
        setError(null);
        setDraft(latest.current.shown);
        e.currentTarget.blur();
      }
    },
  };
  return (
    <label className={`lib-iptc-row${def.multiline ? " multiline" : ""}`}>
      <span className="label">
        {def.label}
        {saving && <span className="saving">保存中…</span>}
      </span>
      {def.multiline ? (
        <textarea className="lib-caption" rows={def.field === "caption" ? 3 : 2} {...common} />
      ) : (
        <input type={def.inputType ?? "text"} spellCheck={false} {...common} />
      )}
      {error && <span className="error-text">{error}</span>}
    </label>
  );
}

/** 入力の決まりの説明（パネルの下に 1 回だけ出す）。 */
export function MetadataHelp() {
  return (
    <p className="hint lib-meta-note">
      カタログの中だけに保存します（元のファイルは変えません。書き出したファイルには入ります）。1 行の欄は Enter、
      複数行の欄は {comboLabel("mod+Enter")}、またはフォーカスを外すと保存します。空にすると削除、Esc で取り消します。
    </p>
  );
}
