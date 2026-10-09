"""Boucle d'entraînement commune aux deux classifieurs (§8.5) : AdamW, lr 1e-3,
schedule cosinus, pondération des classes, early stopping sur le F1 de
validation, puis calibration par temperature scaling.
"""

from __future__ import annotations

import copy
import time
from collections.abc import Callable
from dataclasses import dataclass

import numpy as np
import torch
from sklearn.metrics import f1_score
from torch import nn

Batch = tuple[tuple[torch.Tensor, ...], torch.Tensor]


@dataclass
class TrainResult:
    model: nn.Module
    best_f1: float
    epochs: int
    seconds: float
    temperature: float


def class_weights(targets: np.ndarray, classes: int) -> torch.Tensor:
    counts = np.bincount(targets, minlength=classes).astype(np.float64)
    weights = np.where(counts > 0, counts.sum() / (np.maximum(counts, 1) * max((counts > 0).sum(), 1)), 0.0)
    return torch.tensor(weights, dtype=torch.float32)


@torch.no_grad()
def predict_logits(model: nn.Module, inputs: tuple[torch.Tensor, ...], batch: int = 2048) -> torch.Tensor:
    model.eval()
    out = [model.logits(*(x[i : i + batch] for x in inputs)) for i in range(0, len(inputs[0]), batch)]
    return torch.cat(out) if out else torch.zeros((0, 1))


def fit_temperature(logits: torch.Tensor, targets: torch.Tensor) -> float:
    """Temperature scaling : une seule valeur, ajustée sur la validation."""
    if len(logits) == 0:
        return 1.0
    log_t = torch.zeros((), requires_grad=True)
    optimizer = torch.optim.LBFGS([log_t], lr=0.1, max_iter=60)

    def closure() -> torch.Tensor:
        optimizer.zero_grad()
        loss = nn.functional.cross_entropy(logits / log_t.exp(), targets)
        loss.backward()
        return loss

    optimizer.step(closure)
    return float(log_t.detach().exp().clamp(0.5, 5.0))


def train(
    model: nn.Module,
    classes: int,
    make_epoch: Callable[[np.random.Generator], Batch],
    validation: Batch,
    *,
    epochs: int,
    batch_size: int = 256,
    patience: int = 12,
    seed: int = 0,
    log: Callable[[str], None] = print,
) -> TrainResult:
    """`make_epoch` renvoie les données d'une époque (augmentations comprises)."""
    torch.manual_seed(seed)
    rng = np.random.default_rng(seed)
    optimizer = torch.optim.AdamW(model.parameters(), lr=1e-3, weight_decay=1e-4)
    scheduler = torch.optim.lr_scheduler.CosineAnnealingLR(optimizer, T_max=epochs)
    val_inputs, val_targets = validation
    has_validation = len(val_targets) > 0

    best_f1, best_state, since_best = -1.0, copy.deepcopy(model.state_dict()), 0
    started = time.perf_counter()
    done = 0
    for epoch in range(1, epochs + 1):
        inputs, targets = make_epoch(rng)
        weights = class_weights(targets.numpy(), classes)
        model.train()
        order = torch.randperm(len(targets))
        total = 0.0
        for start in range(0, len(order), batch_size):
            index = order[start : start + batch_size]
            if len(index) < 2:
                continue
            optimizer.zero_grad()
            loss = nn.functional.cross_entropy(
                model.logits(*(x[index] for x in inputs)),
                targets[index],
                weight=weights,
                label_smoothing=0.05,
            )
            loss.backward()
            optimizer.step()
            total += loss.item() * len(index)
        scheduler.step()
        done = epoch

        if has_validation:
            predicted = predict_logits(model, val_inputs).argmax(dim=1)
            f1 = float(f1_score(val_targets.numpy(), predicted.numpy(), average="macro", zero_division=0))
        else:
            # Sans session de validation, on ne peut qu'observer la perte d'entraînement.
            f1 = -total / max(len(order), 1)
        if epoch == 1 or epoch % 10 == 0:
            log(
                f"  époque {epoch:>3} · perte {total / max(len(order), 1):.4f} · F1 validation {max(f1, 0) * 100:.1f} %"  # noqa: E501
            )
        if f1 > best_f1 + 1e-5:
            best_f1, best_state, since_best = f1, copy.deepcopy(model.state_dict()), 0
        else:
            since_best += 1
            if since_best >= patience:
                break

    model.load_state_dict(best_state)
    model.eval()
    temperature = 1.0
    if has_validation:
        temperature = fit_temperature(predict_logits(model, val_inputs), val_targets)
        model.temperature.fill_(temperature)
    return TrainResult(
        model=model,
        best_f1=max(best_f1, 0.0) if has_validation else float("nan"),
        epochs=done,
        seconds=time.perf_counter() - started,
        temperature=temperature,
    )
