"""Entraînement du classifieur de poses statiques (§7.6, §8.5)."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass

import numpy as np
import torch

from .augment import augment_poses
from .common import TrainResult, predict_logits, train
from .data import MIRROR_LABELS, Session, labels_of, split_by_session
from .evaluate import Report, evaluate, false_positives_per_hour
from .models import StaticMLP, count_parameters

#: Nombre maximal d'échantillons tirés par classe à chaque époque : les trames
#: voisines d'une session à 60 fps sont presque identiques.
PER_CLASS = 4000


@dataclass
class StaticOutcome:
    model: StaticMLP
    labels: list[str]
    result: TrainResult
    report: Report | None
    warnings: list[str]
    none_false_positives_per_hour: float | None
    samples: tuple[np.ndarray, np.ndarray]  # (world, handedness) pour les vecteurs dorés
    parameters: int


def mirror_map(labels: list[str]) -> np.ndarray:
    index = {label: i for i, label in enumerate(labels)}
    return np.array([index.get(MIRROR_LABELS.get(label, label), i) for i, label in enumerate(labels)])


def to_arrays(sessions: list[Session], labels: list[str]) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    index = {label: i for i, label in enumerate(labels)}
    if not sessions:
        return np.zeros((0, 21, 3), np.float32), np.zeros((0,), np.float32), np.zeros((0,), np.int64)
    world = np.concatenate([s.world for s in sessions])
    handedness = np.concatenate([s.handedness for s in sessions])
    targets = np.concatenate([np.full(len(s), index[s.label], np.int64) for s in sessions])
    return world, handedness, targets


def _tensors(world: np.ndarray, handedness: np.ndarray, targets: np.ndarray):
    return (torch.from_numpy(world), torch.from_numpy(handedness[:, None])), torch.from_numpy(targets)


def train_static(
    sessions: list[Session],
    *,
    epochs: int = 80,
    seed: int = 0,
    log: Callable[[str], None] = print,
) -> StaticOutcome:
    labels = labels_of(sessions)
    train_sessions, val_sessions, warnings = split_by_session(sessions, seed)
    train_world, train_hand, train_targets = to_arrays(train_sessions, labels)
    val_world, val_hand, val_targets = to_arrays(val_sessions, labels)
    log(
        f"Poses statiques : {len(labels)} classes, {len(train_targets)} trames d'entraînement "
        f"({len(train_sessions)} sessions), {len(val_targets)} de validation ({len(val_sessions)} sessions)"
    )
    for warning in warnings:
        log(f"  ⚠ {warning}")
    mapping = mirror_map(labels)

    def make_epoch(rng: np.random.Generator):
        picks = []
        for cls in range(len(labels)):
            ids = np.nonzero(train_targets == cls)[0]
            if len(ids) > PER_CLASS:
                ids = rng.choice(ids, PER_CLASS, replace=False)
            picks.append(ids)
        ids = np.concatenate(picks)
        world, hand, targets = augment_poses(
            train_world[ids], train_hand[ids], train_targets[ids], mapping, rng
        )
        return _tensors(world, hand, targets)

    model = StaticMLP(len(labels))
    result = train(
        model,
        len(labels),
        make_epoch,
        _tensors(val_world, val_hand, val_targets),
        epochs=epochs,
        seed=seed,
        log=log,
    )

    report = None
    fp_per_hour = None
    if len(val_targets):
        inputs, _ = _tensors(val_world, val_hand, val_targets)
        probabilities = torch.softmax(predict_logits(model, inputs) / result.temperature, dim=1).numpy()
        report = evaluate(labels, val_targets, probabilities.argmax(axis=1))
        # Faux positifs par heure sur les sessions « none » de validation (ENF-12).
        none_sessions = [s for s in val_sessions if s.label == "none"]
        if none_sessions:
            seconds = sum((s.t[-1] - s.t[0]) * 1e-6 for s in none_sessions)
            world, hand, targets = to_arrays(none_sessions, labels)
            inputs, _ = _tensors(world, hand, targets)
            none_probabilities = torch.softmax(
                predict_logits(model, inputs) / result.temperature, dim=1
            ).numpy()
            fp_per_hour = false_positives_per_hour(labels, none_probabilities, seconds, 0.85, 18)

    # Quelques échantillons réels, un par classe si possible, pour les vecteurs dorés.
    source_world, source_hand, source_targets = (
        (val_world, val_hand, val_targets) if len(val_targets) else (train_world, train_hand, train_targets)
    )
    rng = np.random.default_rng(seed)
    golden = [
        rng.choice(np.nonzero(source_targets == c)[0])
        for c in range(len(labels))
        if (source_targets == c).any()
    ]
    golden += list(rng.choice(len(source_targets), size=min(8, len(source_targets)), replace=False))
    golden = np.array(golden)

    return StaticOutcome(
        model=model,
        labels=labels,
        result=result,
        report=report,
        warnings=warnings,
        none_false_positives_per_hour=fp_per_hour,
        samples=(source_world[golden], source_hand[golden]),
        parameters=count_parameters(model),
    )
