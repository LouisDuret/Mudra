"""Bout en bout sur un dataset synthétique : lecture, entraînement court,
export ONNX, métadonnées, vecteurs dorés (§10.3)."""

import json

import numpy as np
import onnx
import onnxruntime as ort
import pytest

from mudra_train import FEATURE_VERSION
from mudra_train.cli import main
from mudra_train.data import load_sessions, segments, split_by_session
from mudra_train.synth import generate


@pytest.fixture(scope="module")
def workspace(tmp_path_factory):
    root = tmp_path_factory.mktemp("mudra")
    generate(root / "dataset", sessions=3, frames=90, reps=8, seed=1)
    code = main(
        [
            "all",
            "--dataset", str(root / "dataset"),
            "--models", str(root / "models"),
            "--golden", str(root / "golden"),
            "--reports", str(root / "reports"),
            "--epochs", "12",
        ]
    )  # fmt: skip
    assert code == 0
    return root


def test_dataset_reading_and_session_split(workspace):
    static = load_sessions(workspace / "dataset", "static")
    assert {s.label for s in static} >= {"none", "open_palm", "fist", "thumbs_up"}
    session = static[0]
    assert session.image.shape == (90, 21, 3) and session.world.shape == (90, 21, 3)
    # Repère isotrope : y a été ramené à l'échelle de la largeur.
    assert session.image[:, :, 1].max() < 720 / 1280 + 0.2
    train, val, warnings = split_by_session(static, seed=0)
    assert not warnings
    assert {id(s) for s in train}.isdisjoint({id(s) for s in val})
    assert {s.label for s in val} == {s.label for s in static}
    dynamic = load_sessions(workspace / "dataset", "dynamic")
    clips = segments(dynamic[0])
    assert len(clips) == 8 and all(len(c) == 60 for c in clips)


def _metadata(path):
    return {p.key: p.value for p in onnx.load(str(path)).metadata_props}


def test_static_model_is_valid_onnx_with_metadata(workspace):
    path = workspace / "models" / "static_gestures.onnx"
    meta = _metadata(path)
    labels = json.loads(meta["labels"])
    assert labels[0] == "none" and "thumbs_up" in labels
    assert meta["feature_version"] == FEATURE_VERSION
    assert len(meta["dataset_hash"]) == 16 and meta["created_at"].endswith("Z")

    session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    assert [(i.name, i.shape) for i in session.get_inputs()] == [
        ("world", [1, 21, 3]),
        ("handedness", [1, 1]),
    ]
    assert [(o.name, o.shape) for o in session.get_outputs()] == [("probs", [1, len(labels)])]

    golden = json.loads((workspace / "golden" / "static_golden.json").read_text(encoding="utf-8"))
    assert golden["model"].endswith("static_gestures.onnx")
    for case in golden["cases"]:
        feed = {
            "world": np.array(case["world"], dtype=np.float32)[None],
            "handedness": np.array([[case["handedness"]]], dtype=np.float32),
        }
        probs = session.run(["probs"], feed)[0][0]
        assert abs(probs.sum() - 1.0) < 1e-4
        assert np.abs(probs - np.array(case["probs"])).max() < 1e-5


def test_dynamic_model_is_valid_onnx_with_metadata(workspace):
    path = workspace / "models" / "dynamic_gestures.onnx"
    meta = _metadata(path)
    labels = json.loads(meta["labels"])
    assert set(labels) == {"none", "swipe_left", "swipe_right", "swipe_up", "swipe_down"}
    assert (meta["window_T"], meta["sample_rate_hz"]) == ("32", "60")
    session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    assert [(i.name, i.shape) for i in session.get_inputs()] == [
        ("image", [1, 32, 21, 3]),
        ("world", [1, 32, 21, 3]),
        ("handedness", [1, 1]),
    ]
    golden = json.loads((workspace / "golden" / "dynamic_golden.json").read_text(encoding="utf-8"))
    assert golden["cases"]


def test_synthetic_poses_are_learned(workspace):
    report = (workspace / "reports" / "static.md").read_text(encoding="utf-8")
    line = next(line for line in report.splitlines() if "Macro-F1" in line)
    score = float(line.split(":")[1].replace("%", "").strip())
    assert score > 90.0, report
    assert (workspace / "reports" / "dynamic.md").exists()
