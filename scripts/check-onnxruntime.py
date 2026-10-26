#!/usr/bin/env python3
"""配布物に同梱した ONNX Runtime を読み込めるかを確かめる（.github/workflows/release.yml の確認。DEV-20）。

    python3 scripts/check-onnxruntime.py <共有ライブラリ> <期待する版>

共有ライブラリを読み込み、C API の OrtGetApiBase()->GetVersionString() で版を聞いて、期待する版と比べる
（scripts/lib/onnxruntime.mjs の ONNXRUNTIME_VERSION）。推論の環境は作らない（テレメトリーの仕組みも動かさない。念のため
環境変数 ORT_DISABLE_TELEMETRY=1 を設定してから読み込む）。標準ライブラリだけを使う。
"""

import ctypes
import os
import sys


class _ApiBase(ctypes.Structure):
    # onnxruntime_c_api.h の OrtApiBase（GetApi, GetVersionString の順）。
    _fields_ = [
        ("GetApi", ctypes.c_void_p),
        ("GetVersionString", ctypes.CFUNCTYPE(ctypes.c_char_p)),
    ]


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    library, expected = sys.argv[1], sys.argv[2]
    os.environ["ORT_DISABLE_TELEMETRY"] = "1"
    lib = ctypes.CDLL(library)
    lib.OrtGetApiBase.restype = ctypes.POINTER(_ApiBase)
    version = lib.OrtGetApiBase().contents.GetVersionString().decode()
    print(f"ONNX Runtime {version}: {library}")
    if version != expected:
        print(f"版が違う（期待 {expected}）", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
