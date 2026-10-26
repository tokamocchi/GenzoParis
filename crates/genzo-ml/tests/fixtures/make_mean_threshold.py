#!/usr/bin/env python3
"""テスト用の小さな ONNX のモデル（mean_threshold.onnx）を作る。

入力 "input"（[1, 3, h, w] の f32）のチャンネルの平均 m から、出力 "output"（[1, 1, h, w]）= sigmoid((m − 0.5) × 20) を返す。
学習したモデルではなく、genzo-ml の ONNX Runtime の backend（読み込み・入出力・形）を確かめるためだけのもの。

    python3 -I make_mean_threshold.py   # 同じフォルダに mean_threshold.onnx を書く（onnx のパッケージが要る）
"""

from pathlib import Path

import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper

inp = helper.make_tensor_value_info("input", TensorProto.FLOAT, [1, 3, "h", "w"])
out = helper.make_tensor_value_info("output", TensorProto.FLOAT, [1, 1, "h", "w"])
half = numpy_helper.from_array(np.array(0.5, dtype=np.float32), "half")
gain = numpy_helper.from_array(np.array(20.0, dtype=np.float32), "gain")
nodes = [
    helper.make_node("ReduceMean", ["input"], ["mean"], axes=[1], keepdims=1),
    helper.make_node("Sub", ["mean", "half"], ["centered"]),
    helper.make_node("Mul", ["centered", "gain"], ["scaled"]),
    helper.make_node("Sigmoid", ["scaled"], ["output"]),
]
graph = helper.make_graph(nodes, "mean_threshold", [inp], [out], initializer=[half, gain])
model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)], producer_name="genzo-ml-test")
model.ir_version = 8
onnx.checker.check_model(model)
onnx.save(model, Path(__file__).with_name("mean_threshold.onnx"))
print("wrote mean_threshold.onnx")
