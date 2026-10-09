"""Dataset synthétique : mains schématiques au format du dataset de Mudra.

Il ne remplace pas de vraies données ; il sert aux tests et à l'intégration
continue (§10.3), pour vérifier la chaîne lecture → entraînement → export ONNX
→ chargement par l'application sans caméra. Même géométrie que
`mudra_gestures::builtin::synthetic` côté Rust.
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np

from . import DATASET_FORMAT_VERSION

WIDTH, HEIGHT, FPS = 1280, 720, 60.0

# (flexion index..auriculaire, pouce écarté, pincement, rotation dans le plan en degrés)
POSES: dict[str, tuple[tuple[float, float, float, float], float, bool, float]] = {
    "open_palm": ((0.0, 0.0, 0.0, 0.0), 1.0, False, 0.0),
    "fist": ((1.0, 1.0, 1.0, 1.0), 0.0, False, 0.0),
    "thumbs_up": ((1.0, 1.0, 1.0, 1.0), 1.0, False, 51.0),
    "thumbs_down": ((1.0, 1.0, 1.0, 1.0), 1.0, False, -129.0),
    "v_sign": ((0.0, 0.0, 1.0, 1.0), 0.0, False, 0.0),
    "rock": ((0.0, 1.0, 1.0, 0.0), 0.0, False, 0.0),
    "three_fingers": ((0.0, 0.0, 0.0, 1.0), 0.0, False, 0.0),
    "pointing": ((0.0, 1.0, 1.0, 1.0), 0.0, False, 0.0),
    "pinch": ((0.45, 0.1, 0.1, 0.1), 1.0, True, 0.0),
}
SWIPES = {
    "swipe_left": (-1.0, 0.0),
    "swipe_right": (1.0, 0.0),
    "swipe_up": (0.0, -1.0),
    "swipe_down": (0.0, 1.0),
}

_BASES = np.array([[-0.025, -0.085], [-0.004, -0.090], [0.016, -0.085], [0.034, -0.075]])
_LENGTHS = np.array(
    [[0.040, 0.025, 0.020], [0.045, 0.028, 0.022], [0.041, 0.026, 0.021], [0.032, 0.019, 0.018]]
)
_SPLAY = np.deg2rad([-8.0, 0.0, 7.0, 15.0])


def hand_world(fingers, thumb: float, pinch: bool, roll_deg: float) -> np.ndarray:
    """Main droite schématique `[21, 3]`, en mètres, poignet à l'origine, doigts vers le haut (y négatif)."""
    lm = np.zeros((21, 3), dtype=np.float64)
    for f in range(4):
        point = np.array([_BASES[f][0], _BASES[f][1], 0.0])
        lm[5 + 4 * f] = point
        angle = 0.0
        for j, bend in enumerate((80.0, 100.0, 70.0)):
            angle += np.deg2rad(fingers[f] * bend)
            length = _LENGTHS[f][j]
            point = point + length * np.array(
                [np.cos(angle) * np.sin(_SPLAY[f]), -np.cos(angle) * np.cos(_SPLAY[f]), -np.sin(angle)]
            )
            lm[6 + 4 * f + j] = point
    # Pouce : écarté sur le côté, ou au repos contre le flanc de l'index.
    rest_ip = lm[5] + [-0.016, 0.012, -0.006]
    rest_tip = lm[6] + [-0.012, 0.008, -0.004]
    lm[1] = [-0.025, -0.020, -0.005]
    lm[2] = np.array([-0.036, -0.048, -0.012]) * (1 - thumb) + np.array([-0.050, -0.040, -0.008]) * thumb
    lm[3] = rest_ip * (1 - thumb) + np.array([-0.072, -0.058, -0.010]) * thumb
    lm[4] = rest_tip * (1 - thumb) + np.array([-0.092, -0.074, -0.012]) * thumb
    if pinch:
        lm[3] = lm[8] + [-0.012, 0.028, 0.004]
        lm[4] = lm[8] + [-0.004, 0.006, -0.002]
    c, s = np.cos(np.deg2rad(roll_deg)), np.sin(np.deg2rad(roll_deg))
    rotated = lm.copy()
    rotated[:, 0] = lm[:, 0] * c - lm[:, 1] * s
    rotated[:, 1] = lm[:, 0] * s + lm[:, 1] * c
    return rotated


def _frame(
    t_us: int, world: np.ndarray, centre: tuple[float, float], palm_px: float, right: bool, clip=None
) -> dict:
    """Projette la main dans l'image (pixels → x/largeur, y/hauteur) et forme une ligne du dataset."""
    world = world.copy()
    if not right:
        world[:, 0] *= -1.0
    scale = palm_px / (np.linalg.norm(world[9] - world[0]) + 1e-9)
    image = np.empty_like(world)
    image[:, 0] = (centre[0] + world[:, 0] * scale) / WIDTH
    image[:, 1] = (centre[1] + world[:, 1] * scale) / HEIGHT
    image[:, 2] = world[:, 2] * scale / WIDTH
    frame = {
        "t": int(t_us),
        "track": 1,
        "hand": "right" if right else "left",
        "hand_score": 0.98,
        "presence": 0.99,
        "image": np.round(image, 6).tolist(),
        "world": np.round(world, 6).tolist(),
    }
    if clip is not None:
        frame["clip"] = clip
    return frame


def _write(path: Path, kind: str, label: str, frames: list[dict], clips: int | None = None) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    header = {
        "format_version": DATASET_FORMAT_VERSION,
        "kind": kind,
        "label": label,
        "camera": "synthétique",
        "width": WIDTH,
        "height": HEIGHT,
        "fps": FPS,
        "mirror": True,
        "created_at": "2026-01-01T00:00:00Z",
    }
    if clips is not None:
        header["clips"] = clips
        header["clip_seconds"] = 1.0
    with path.open("w", encoding="utf-8") as f:
        f.write(json.dumps(header) + "\n")
        for frame in frames:
            f.write(json.dumps(frame) + "\n")


def _noisy_pose(rng: np.random.Generator, label: str) -> np.ndarray:
    if label == "none":
        # Main détendue ou en mouvement quelconque : doigts à demi fléchis, au hasard.
        fingers = rng.uniform(0.35, 0.65, 4)
        return hand_world(fingers, rng.uniform(0.0, 0.5), False, rng.uniform(-40.0, 40.0))
    fingers, thumb, pinch, roll = POSES[label]
    fingers = np.clip(np.array(fingers) + rng.normal(0.0, 0.05, 4), 0.0, 1.0)
    world = hand_world(
        fingers, float(np.clip(thumb + rng.normal(0.0, 0.05), 0.0, 1.0)), pinch, roll + rng.normal(0.0, 6.0)
    )
    return world + rng.normal(0.0, 0.0008, world.shape)


def generate(root: Path, *, sessions: int = 3, frames: int = 240, reps: int = 12, seed: int = 0) -> None:
    """Écrit un petit dataset synthétique complet sous `root`."""
    rng = np.random.default_rng(seed)
    period = int(1e6 / FPS)
    for label in [*POSES, "none"]:
        for s in range(sessions):
            right = (s % 2 == 0) or label in ("thumbs_up", "thumbs_down")
            rows = []
            centre = np.array([rng.uniform(400, 880), rng.uniform(250, 470)])
            palm = rng.uniform(70, 130)
            for i in range(frames):
                centre = centre + rng.normal(0.0, 1.0, 2)
                rows.append(_frame(i * period, _noisy_pose(rng, label), tuple(centre), palm, right))
            _write(root / "static" / label / f"synth-{s}.jsonl", "static", label, rows)

    open_pose = POSES["open_palm"]
    for label in [*SWIPES, "none"]:
        for s in range(sessions):
            rows = []
            t = 0
            for clip in range(1, reps + 1):
                right = clip % 2 == 0
                palm = rng.uniform(70, 130)
                start = np.array([rng.uniform(500, 780), rng.uniform(280, 440)])
                direction = np.array(SWIPES.get(label, (0.0, 0.0)))
                amplitude = rng.uniform(2.2, 3.2) * palm
                for i in range(60):
                    if label == "none":
                        offset = rng.normal(0.0, 0.15, 2) * palm * 0.05 * i**0.5
                        world = _noisy_pose(rng, "none")
                    else:
                        # Immobile, balayage franc entre 0,3 s et 0,7 s, puis immobile.
                        progress = np.clip((i - 18) / 24, 0.0, 1.0)
                        progress = progress * progress * (3 - 2 * progress)
                        offset = direction * amplitude * (progress - 0.5)
                        fingers = np.clip(np.array(open_pose[0]) + rng.normal(0.0, 0.04, 4), 0.0, 1.0)
                        world = hand_world(fingers, 1.0, False, rng.normal(0.0, 5.0))
                    rows.append(_frame(t + i * period, world, tuple(start + offset), palm, right, clip))
                t += 60 * period + 2_600_000
            _write(root / "dynamic" / label / f"synth-{s}.jsonl", "dynamic", label, rows, reps)
