// エラーの扱い（genzo-api の ErrorInfo。04 の 6.3 節）。

import type { ErrorInfo, ErrorKind } from "./types";

/** コマンドの失敗（ErrorInfo を持つ）。 */
export class ApiError extends Error {
  readonly info: ErrorInfo;
  constructor(info: ErrorInfo) {
    super(info.message);
    this.name = "ApiError";
    this.info = info;
  }
  get kind(): ErrorKind {
    return this.info.kind;
  }
}

function isErrorInfo(v: unknown): v is ErrorInfo {
  return (
    typeof v === "object" &&
    v !== null &&
    typeof (v as ErrorInfo).kind === "string" &&
    typeof (v as ErrorInfo).message === "string"
  );
}

/** invoke の reject の値などを ApiError にする。 */
export function toApiError(e: unknown): ApiError {
  if (e instanceof ApiError) return e;
  if (isErrorInfo(e)) return new ApiError(e);
  if (typeof e === "object" && e !== null && isErrorInfo((e as { error?: unknown }).error)) {
    return new ApiError((e as { error: ErrorInfo }).error);
  }
  const message = e instanceof Error ? e.message : typeof e === "string" ? e : String(e);
  return new ApiError({
    kind: "internal",
    message: message || "不明なエラー",
    user_actionable: false,
    retryable: false,
    hint: null,
  });
}

/** 取り消された（新しい要求で置き換えられた）か。利用者には見せない。 */
export function isCancelled(e: unknown): boolean {
  return toApiError(e).kind === "cancelled";
}

/** 検索結果が新しい世代になった（要求し直す）か。 */
export function isStale(e: unknown): boolean {
  return toApiError(e).kind === "stale";
}

/** 表示用の文（説明と案内）。 */
export function describeError(e: unknown): string {
  const info = toApiError(e).info;
  return info.hint ? `${info.message}\n${info.hint}` : info.message;
}
