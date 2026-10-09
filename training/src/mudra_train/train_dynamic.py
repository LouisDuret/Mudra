"""Entraînement du classifieur de gestes dynamiques (§7.6, §8.5)."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass

import numpy as np
import torch

from . import NONE_LABEL
from .augment import augment_window
from .common import TrainResult, predict_logits, train
from .data import Session, labels_of, resample, segments, split_by_session
from .evaluate import Report, evaluate
from .models import DynamicTCN, count_parameters
from .train_static import mirror_map

WINDOW = 32
SAMPLE_RATE_HZ = 60.0
#: Dans un clip de 1,0 s centré sur le geste, la fenêtre positive se termine
#: peu après la fin du mouvement (trame ~48 sur 60), à ±4 trames près (§8.4).
POSITIVE_END = 48
SHIFT = 4


@dataclass
class Window:
    image: np.ndarray  # [T, 21, 3]
    world: np.ndarray
    handedness: float
    target: int


@dataclass
class DynamicOutcome:
    model: DynamicTCN
    labels: list[str]
    result: TrainResult
    report: Report | None
    warnings: list[str]
    samples: list[Window]
    parameters: int


def _slice(image: np.ndarray, world: np.ndarray, end: int) -> tuple[np.ndarray, np.ndarray] | None:
    start = end - WINDOW
    if start < 0 or end > len(image):
        return None
    return image[start:end], world[start:end]


def windows_from(
    dynamic: list[Session],
    static: list[Session],
    labels: list[str],
    rng: np.random.Generator,
    *,
    jitter: bool,
) -> list[Window]:
    index = {label: i for i, label in enumerate(labels)}
    none = index[NONE_LABEL]
    out: list[Window] = []
    for session in dynamic:
        for ids in segments(session):
            image, world = resample(session, ids, SAMPLE_RATE_HZ)
            handedness = float(session.handedness[ids[0]])
            if session.label == NONE_LABEL:
                # Sessions « none » : toutes les fenêtres sont négatives.
                for end in range(WINDOW, len(image) + 1, 8):
                    piece = _slice(image, world, end)
                    if piece:
                        out.append(Window(*piece, handedness, none))
                continue
            shifts = rng.integers(-SHIFT, SHIFT + 1, size=3) if jitter else [0]
            for shift in shifts:
                piece = _slice(image, world, min(POSITIVE_END + int(shift), len(image)))
                if piece:
                    out.append(Window(*piece, handedness, index[session.label]))
            # Le début du clip, avant que le geste soit terminé, est un négatif.
            piece = _slice(image, world, WINDOW)
            if piece:
                out.append(Window(*piece, handedness, none))
    # Les sessions de poses statiques sont autant de « non-gestes » dynamiques.
    for session in static:
        for ids in segments(session):
            image, world = resample(session, ids, SAMPLE_RATE_HZ)
            for end in range(WINDOW, len(image) + 1, 24):
                piece = _slice(image, world, end)
                if piece:
                    out.append(Window(*piece, float(session.handedness[ids[0]]), none))
    return out


def _tensors(windows: list[Window]):
    if not windows:
        empty = torch.zeros((0, WINDOW, 21, 3))
        return (empty, empty.clone(), torch.zeros((0, 1))), torch.zeros((0,), dtype=torch.long)
    image = torch.from_numpy(np.stack([w.image for w in windows]))
    world = torch.from_numpy(np.stack([w.world for w in windows]))
    handedness = torch.tensor([[w.handedness] for w in windows], dtype=torch.float32)
    targets = torch.tensor([w.target for w in windows], dtype=torch.long)
    return (image, world, handedness), targets


def train_dynamic(
    dynamic: list[Session],
    static: list[Session],
    *,
    epochs: int = 80,
    seed: int = 0,
    log: Callable[[str], None] = print,
) -> DynamicOutcome:
    labels = labels_of(dynamic)
    rng = np.random.default_rng(seed)
    train_sessions, val_sessions, warnings = split_by_session(dynamic, seed)
    static_train, static_val, _ = split_by_session(static, seed)
    train_windows = windows_from(train_sessions, static_train, labels, rng, jitter=True)
    val_windows = windows_from(val_sessions, static_val, labels, rng, jitter=False)
    positives = sum(w.target != 0 for w in train_windows)
    log(
        f"Gestes dynamiques : {len(labels)} classes, {len(train_windows)} fenêtres d'entraînement "
        f"dont {positives} positives, {len(val_windows)} de validation"
    )
    for warning in warnings:
        log(f"  ⚠ {warning}")
    mapping = mirror_map(labels)

    def make_epoch(epoch_rng: np.random.Generator):
        # Les négatifs sont nombreux : on en garde au plus 3 par positif à chaque époque.
        negatives = [w for w in train_windows if w.target == 0]
        keep = min(len(negatives), max(3 * positives, 64))
        chosen = [w for w in train_windows if w.target != 0]
        chosen += (
            [negatives[i] for i in epoch_rng.choice(len(negatives), keep, replace=False)] if negatives else []
        )
        augmented = [
            Window(*augment_window(w.image, w.world, w.handedness, w.target, mapping, epoch_rng))
            for w in chosen
        ]
        return _tensors(augmented)

    model = DynamicTCN(len(labels))
    result = train(
        model,
        len(labels),
        make_epoch,
        _tensors(val_windows),
        epochs=epochs,
        batch_size=64,
        seed=seed,
        log=log,
    )

    report = None
    if val_windows:
        inputs, targets = _tensors(val_windows)
        report = evaluate(labels, targets.numpy(), predict_logits(model, inputs).argmax(dim=1).numpy())

    source = val_windows or train_windows
    picks = rng.choice(len(source), size=min(8, len(source)), replace=False) if source else []
    return DynamicOutcome(
        model=model,
        labels=labels,
        result=result,
        report=report,
        warnings=warnings,
        samples=[source[i] for i in picks],
        parameters=count_parameters(model),
    )
