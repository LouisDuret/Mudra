"""Export ONNX, métadonnées et vecteurs dorés (§8.5, D-08 à D-10)."""

from __future__ import annotations

import inspect
import json
import os
from datetime import UTC, datetime
from pathlib import Path

import numpy as np
import onnx
import onnxruntime as ort
import torch

from . import FEATURE_VERSION

OPSET = 17
#: Écart maximal toléré entre PyTorch et ONNX Runtime.
TOLERANCE = 1e-5


def _export(model: torch.nn.Module, args: tuple, path: Path, input_names: list[str]) -> None:
    model.eval()
    kwargs = dict(
        input_names=input_names, output_names=["probs"], opset_version=OPSET, do_constant_folding=True
    )
    # Selon la version de PyTorch, l'exporteur par défaut change (TorchScript ou
    # dynamo). On fixe l'exporteur TorchScript : stable, et sans dépendance
    # supplémentaire. Batch fixe à 1 : aucun axe dynamique.
    if "dynamo" in inspect.signature(torch.onnx.export).parameters:
        kwargs["dynamo"] = False
    path.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(model, args, str(path), **kwargs)


def _add_metadata(path: Path, metadata: dict[str, str]) -> None:
    proto = onnx.load(str(path))
    del proto.metadata_props[:]
    for key, value in metadata.items():
        entry = proto.metadata_props.add()
        entry.key, entry.value = key, value
    onnx.checker.check_model(proto)
    onnx.save(proto, str(path))


def _session(path: Path) -> ort.InferenceSession:
    options = ort.SessionOptions()
    options.intra_op_num_threads = 1
    return ort.InferenceSession(str(path), options, providers=["CPUExecutionProvider"])


def _write_golden(golden_dir: Path, name: str, model_path: Path, cases: list[dict]) -> Path:
    golden_dir.mkdir(parents=True, exist_ok=True)
    out = golden_dir / f"{name}.json"
    payload = {
        # Chemin du modèle relatif au fichier doré, pour le test Rust.
        "model": os.path.relpath(model_path, golden_dir).replace("\\", "/"),
        "feature_version": FEATURE_VERSION,
        "cases": cases,
    }
    out.write_text(json.dumps(payload), encoding="utf-8")
    return out


def _common_metadata(labels: list[str], dataset_hash: str) -> dict[str, str]:
    return {
        "labels": json.dumps(labels),
        "feature_version": FEATURE_VERSION,
        "created_at": datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "dataset_hash": dataset_hash,
    }


def export_static(
    model: torch.nn.Module,
    labels: list[str],
    path: Path,
    golden_dir: Path,
    dataset_hash: str,
    samples: tuple[np.ndarray, np.ndarray],
) -> float:
    """Exporte le modèle statique et renvoie l'écart maximal ONNX Runtime / PyTorch."""
    world, handedness = samples
    example = (torch.from_numpy(world[:1]), torch.tensor([[float(handedness[0])]]))
    _export(model, example, path, ["world", "handedness"])
    _add_metadata(path, _common_metadata(labels, dataset_hash))

    session = _session(path)
    worst = 0.0
    cases = []
    for w, h in zip(world, handedness, strict=True):
        feed = {"world": w[None].astype(np.float32), "handedness": np.array([[h]], dtype=np.float32)}
        got = session.run(["probs"], feed)[0][0]
        with torch.no_grad():
            expected = model(torch.from_numpy(feed["world"]), torch.from_numpy(feed["handedness"]))[0].numpy()
        worst = max(worst, float(np.abs(got - expected).max()))
        cases.append({"world": w.tolist(), "handedness": float(h), "probs": got.tolist()})
    if worst > TOLERANCE:
        raise RuntimeError(f"{path.name} : écart ONNX Runtime / PyTorch de {worst:.2e} (> {TOLERANCE:.0e})")
    _write_golden(golden_dir, "static_golden", path, cases)
    return worst


def export_dynamic(
    model: torch.nn.Module,
    labels: list[str],
    path: Path,
    golden_dir: Path,
    dataset_hash: str,
    samples: list,
    window: int,
    sample_rate_hz: float,
) -> float:
    """Exporte le modèle dynamique et renvoie l'écart maximal ONNX Runtime / PyTorch."""
    first = samples[0]
    example = (
        torch.from_numpy(first.image[None]),
        torch.from_numpy(first.world[None]),
        torch.tensor([[first.handedness]], dtype=torch.float32),
    )
    _export(model, example, path, ["image", "world", "handedness"])
    metadata = _common_metadata(labels, dataset_hash)
    metadata["window_T"] = str(window)
    metadata["sample_rate_hz"] = str(int(sample_rate_hz))
    _add_metadata(path, metadata)

    session = _session(path)
    worst = 0.0
    cases = []
    for sample in samples:
        feed = {
            "image": sample.image[None].astype(np.float32),
            "world": sample.world[None].astype(np.float32),
            "handedness": np.array([[sample.handedness]], dtype=np.float32),
        }
        got = session.run(["probs"], feed)[0][0]
        with torch.no_grad():
            expected = model(*(torch.from_numpy(feed[k]) for k in ("image", "world", "handedness")))[
                0
            ].numpy()
        worst = max(worst, float(np.abs(got - expected).max()))
        cases.append(
            {
                "image": sample.image.tolist(),
                "world": sample.world.tolist(),
                "handedness": float(sample.handedness),
                "probs": got.tolist(),
            }
        )
    if worst > TOLERANCE:
        raise RuntimeError(f"{path.name} : écart ONNX Runtime / PyTorch de {worst:.2e} (> {TOLERANCE:.0e})")
    _write_golden(golden_dir, "dynamic_golden", path, cases)
    return worst
