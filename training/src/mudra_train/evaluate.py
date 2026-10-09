"""Évaluation : matrice de confusion, F1 par classe, faux positifs par heure (§8.5)."""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np
from sklearn.metrics import confusion_matrix, f1_score, precision_recall_fscore_support

from . import NONE_LABEL


@dataclass
class Report:
    labels: list[str]
    confusion: np.ndarray
    f1_per_class: np.ndarray
    precision: np.ndarray
    recall: np.ndarray
    macro_f1: float
    accuracy: float
    samples: int

    def to_markdown(self, title: str) -> str:
        lines = [f"## {title}", ""]
        lines.append(f"- Échantillons de validation : {self.samples}")
        lines.append(f"- Exactitude : {self.accuracy * 100:.2f} %")
        lines.append(f"- Macro-F1 : {self.macro_f1 * 100:.2f} %")
        lines += ["", "| Classe | Précision | Rappel | F1 |", "|---|---|---|---|"]
        for i, label in enumerate(self.labels):
            lines.append(
                f"| {label} | {self.precision[i] * 100:.1f} % | {self.recall[i] * 100:.1f} % "
                f"| {self.f1_per_class[i] * 100:.1f} % |"
            )
        lines += ["", "Matrice de confusion (lignes : vérité, colonnes : prédiction) :", ""]
        lines.append("| | " + " | ".join(self.labels) + " |")
        lines.append("|---|" + "---|" * len(self.labels))
        for i, label in enumerate(self.labels):
            lines.append(f"| **{label}** | " + " | ".join(str(int(v)) for v in self.confusion[i]) + " |")
        return "\n".join(lines) + "\n"


def evaluate(labels: list[str], truth: np.ndarray, predicted: np.ndarray) -> Report:
    ids = list(range(len(labels)))
    precision, recall, f1, _ = precision_recall_fscore_support(truth, predicted, labels=ids, zero_division=0)
    present = sorted(set(truth.tolist()))
    return Report(
        labels=labels,
        confusion=confusion_matrix(truth, predicted, labels=ids),
        f1_per_class=f1,
        precision=precision,
        recall=recall,
        # Macro-F1 sur les classes réellement présentes en validation.
        macro_f1=float(f1_score(truth, predicted, labels=present, average="macro", zero_division=0)),
        accuracy=float((truth == predicted).mean()) if len(truth) else 0.0,
        samples=int(len(truth)),
    )


def false_positives_per_hour(
    labels: list[str],
    probabilities: np.ndarray,
    seconds: float,
    threshold: float,
    hold_frames: int,
) -> float:
    """Déclenchements qu'aurait produits une séquence « none » (ENF-12).

    Compte les fronts montants d'une classe autre que `none` tenue au-dessus du
    seuil pendant `hold_frames` trames consécutives, comme le fait le moteur.
    """
    if seconds <= 0 or len(probabilities) == 0:
        return 0.0
    none = labels.index(NONE_LABEL)
    best = probabilities.argmax(axis=1)
    confident = (best != none) & (probabilities.max(axis=1) >= threshold)
    fired = 0
    run = 0
    previous = -1
    for cls, ok in zip(best, confident, strict=True):
        run = run + 1 if ok and cls == previous else (1 if ok else 0)
        previous = cls if ok else -1
        if run == hold_frames:
            fired += 1
    return fired * 3600.0 / seconds
