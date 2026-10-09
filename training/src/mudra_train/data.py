"""Lecture du dataset JSONL (EF-62) et découpage par session (§8.5)."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from . import DATASET_FORMAT_VERSION, NONE_LABEL

#: Étiquettes directionnelles échangées par le miroir (§8.4).
MIRROR_LABELS = {
    "swipe_left": "swipe_right",
    "swipe_right": "swipe_left",
    "rotate_cw": "rotate_ccw",
    "rotate_ccw": "rotate_cw",
}


@dataclass
class Session:
    """Une session d'enregistrement : un fichier `.jsonl`."""

    path: Path
    kind: str  # "static" | "dynamic"
    label: str
    width: int
    height: int
    fps: float
    t: np.ndarray  # (N,) microsecondes
    image: np.ndarray  # (N, 21, 3) repère isotrope : x/W, y/W, z/W
    world: np.ndarray  # (N, 21, 3) mètres
    handedness: np.ndarray  # (N,) 0 = gauche, 1 = droite
    track: np.ndarray  # (N,)
    clip: np.ndarray  # (N,) numéro de répétition, -1 hors mode dynamique
    meta: dict = field(default_factory=dict)

    def __len__(self) -> int:
        return len(self.t)


def read_session(path: Path) -> Session:
    with path.open(encoding="utf-8") as f:
        lines = [line for line in f if line.strip()]
    if not lines:
        raise ValueError(f"{path}: fichier vide")
    meta = json.loads(lines[0])
    if meta.get("format_version") != DATASET_FORMAT_VERSION:
        raise ValueError(f"{path}: format_version {meta.get('format_version')} non pris en charge")
    frames = [json.loads(line) for line in lines[1:]]
    n = len(frames)
    image = np.zeros((n, 21, 3), dtype=np.float32)
    world = np.zeros((n, 21, 3), dtype=np.float32)
    t = np.zeros(n, dtype=np.int64)
    handedness = np.zeros(n, dtype=np.float32)
    track = np.zeros(n, dtype=np.int64)
    clip = np.full(n, -1, dtype=np.int64)
    for i, frame in enumerate(frames):
        t[i] = frame["t"]
        image[i] = frame["image"]
        world[i] = frame["world"]
        handedness[i] = 1.0 if frame["hand"] == "right" else 0.0
        track[i] = frame["track"]
        clip[i] = frame.get("clip", -1) if frame.get("clip") is not None else -1
    # Le fichier stocke x/largeur et y/hauteur : on repasse en repère isotrope
    # (tout divisé par la largeur), celui que l'application fournit aux modèles.
    width, height = int(meta["width"]), int(meta["height"])
    image[:, :, 1] *= height / max(width, 1)
    return Session(
        path=path,
        kind=meta["kind"],
        label=meta["label"],
        width=width,
        height=height,
        fps=float(meta.get("fps", 60.0)),
        t=t,
        image=image,
        world=world,
        handedness=handedness,
        track=track,
        clip=clip,
        meta=meta,
    )


def load_sessions(root: Path, kind: str) -> list[Session]:
    """Charge `root/<kind>/<label>/*.jsonl`, triées pour un résultat reproductible."""
    sessions = []
    for path in sorted((root / kind).glob("*/*.jsonl")):
        session = read_session(path)
        if len(session) == 0:
            print(f"  (session vide ignorée : {path})")
            continue
        sessions.append(session)
    return sessions


def dataset_hash(sessions: list[Session]) -> str:
    """Empreinte du dataset, stockée dans les métadonnées du modèle (D-10)."""
    digest = hashlib.sha256()
    for session in sorted(sessions, key=lambda s: str(s.path)):
        digest.update(session.path.name.encode())
        digest.update(session.path.read_bytes())
    return digest.hexdigest()[:16]


def labels_of(sessions: list[Session]) -> list[str]:
    """Liste des classes, `none` toujours en tête."""
    labels = sorted({s.label for s in sessions} - {NONE_LABEL})
    return [NONE_LABEL, *labels]


def split_by_session(
    sessions: list[Session], seed: int = 0
) -> tuple[list[Session], list[Session], list[str]]:
    """Découpage entraînement / validation par session, jamais par trame (§8.5).

    Pour chaque label, une session sur quatre (au moins une) part en validation.
    Un label qui n'a qu'une session reste entièrement en entraînement : c'est
    signalé, car son score de validation n'existe pas.
    """
    rng = np.random.default_rng(seed)
    train: list[Session] = []
    val: list[Session] = []
    warnings: list[str] = []
    by_label: dict[str, list[Session]] = {}
    for session in sessions:
        by_label.setdefault(session.label, []).append(session)
    for label, group in sorted(by_label.items()):
        if len(group) < 2:
            warnings.append(
                f"« {label} » n'a qu'une session : pas de validation possible. Enregistre au moins 3 sessions."  # noqa: E501
            )
            train.extend(group)
            continue
        order = rng.permutation(len(group))
        n_val = max(1, len(group) // 4)
        val.extend(group[i] for i in order[:n_val])
        train.extend(group[i] for i in order[n_val:])
    return train, val, warnings


def resample(session: Session, indices: np.ndarray, rate_hz: float) -> tuple[np.ndarray, np.ndarray]:
    """Ré-échantillonne des trames à fréquence fixe par interpolation linéaire des horodatages (EF-21)."""
    t = session.t[indices].astype(np.float64)
    if len(t) < 2:
        return session.image[indices], session.world[indices]
    grid = np.arange(t[0], t[-1] + 1, 1e6 / rate_hz)
    out = []
    for array in (session.image[indices], session.world[indices]):
        flat = array.reshape(len(t), -1)
        columns = [np.interp(grid, t, flat[:, c]) for c in range(flat.shape[1])]
        out.append(np.stack(columns, axis=1).reshape(len(grid), 21, 3).astype(np.float32))
    return out[0], out[1]


def segments(session: Session, max_gap_us: int = 150_000) -> list[np.ndarray]:
    """Découpe une session en segments continus (même piste, même clip, sans trou)."""
    if len(session) == 0:
        return []
    breaks = (
        (np.diff(session.t) > max_gap_us)
        | (np.diff(session.track) != 0)
        | (np.diff(session.clip) != 0)
        | (np.diff(session.t) <= 0)
    )
    starts = np.concatenate([[0], np.nonzero(breaks)[0] + 1, [len(session)]])
    return [np.arange(a, b) for a, b in zip(starts[:-1], starts[1:], strict=True) if b - a >= 2]
