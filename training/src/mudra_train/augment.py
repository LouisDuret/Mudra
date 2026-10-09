"""Augmentations sur les points bruts, avant l'extraction des features (§8.4)."""

from __future__ import annotations

import numpy as np


def _rotations(rng: np.random.Generator, n: int, plane_deg: float, tilt_deg: float) -> np.ndarray:
    """`n` matrices de rotation : dans le plan (±plane_deg) puis 3D légère (±tilt_deg)."""
    az = np.deg2rad(rng.uniform(-plane_deg, plane_deg, n))
    ax = np.deg2rad(rng.uniform(-tilt_deg, tilt_deg, n))
    ay = np.deg2rad(rng.uniform(-tilt_deg, tilt_deg, n))
    zeros, ones = np.zeros(n), np.ones(n)
    rz = np.stack([np.cos(az), -np.sin(az), zeros, np.sin(az), np.cos(az), zeros, zeros, zeros, ones], -1)
    rx = np.stack([ones, zeros, zeros, zeros, np.cos(ax), -np.sin(ax), zeros, np.sin(ax), np.cos(ax)], -1)
    ry = np.stack([np.cos(ay), zeros, np.sin(ay), zeros, ones, zeros, -np.sin(ay), zeros, np.cos(ay)], -1)
    shape = (n, 3, 3)
    return (rz.reshape(shape) @ rx.reshape(shape) @ ry.reshape(shape)).astype(np.float32)


def augment_poses(
    world: np.ndarray,
    handedness: np.ndarray,
    targets: np.ndarray,
    mirror_map: np.ndarray,
    rng: np.random.Generator,
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Augmente un lot de poses `[N, 21, 3]` : rotation dans le plan ±15°,
    rotation 3D ±10°, échelle 0,85 à 1,15, bruit gaussien (1 % de la paume),
    miroir avec inversion de la latéralité et des labels directionnels.
    """
    n = len(world)
    rotation = _rotations(rng, n, 15.0, 10.0)
    centre = world.mean(axis=1, keepdims=True)
    scale = rng.uniform(0.85, 1.15, (n, 1, 1)).astype(np.float32)
    out = np.einsum("nij,nkj->nki", rotation, world - centre) * scale + centre
    palm = np.linalg.norm(out[:, 9] - out[:, 0], axis=-1)[:, None, None] + 1e-6
    out = out + rng.normal(0.0, 0.01, out.shape).astype(np.float32) * palm
    mirror = rng.random(n) < 0.5
    out[mirror, :, 0] *= -1.0
    handedness = np.where(mirror, 1.0 - handedness, handedness)
    targets = np.where(mirror, mirror_map[targets], targets)
    return out.astype(np.float32), handedness.astype(np.float32), targets


def augment_window(
    image: np.ndarray,
    world: np.ndarray,
    handedness: float,
    target: int,
    mirror_map: np.ndarray,
    rng: np.random.Generator,
) -> tuple[np.ndarray, np.ndarray, float, int]:
    """Augmente une fenêtre `[T, 21, 3]` : même transformation à chaque trame,
    plus une déformation temporelle (vitesse 0,7 à 1,3×).
    """
    frames = image.shape[0]
    # Déformation temporelle autour de la dernière trame : le geste finit au même instant.
    speed = rng.uniform(0.7, 1.3)
    source = np.clip((frames - 1) - (frames - 1 - np.arange(frames)) * speed, 0, frames - 1)
    lo = np.floor(source).astype(int)
    hi = np.minimum(lo + 1, frames - 1)
    w = (source - lo).astype(np.float32)[:, None, None]
    image = image[lo] * (1 - w) + image[hi] * w
    world = world[lo] * (1 - w) + world[hi] * w

    # Rotation dans le plan (image et forme ensemble), échelle, bruit.
    rotation = _rotations(rng, 1, 12.0, 0.0)[0]
    pivot = image[-1].mean(axis=0, keepdims=True)
    image = (image - pivot) @ rotation.T * rng.uniform(0.85, 1.15) + pivot
    centre = world.mean(axis=1, keepdims=True)
    world = (world - centre) @ rotation.T + centre
    palm = np.linalg.norm(world[:, 9] - world[:, 0], axis=-1).mean() + 1e-6
    world = world + rng.normal(0.0, 0.01 * palm, world.shape)
    image_palm = np.linalg.norm(image[:, 9, :2] - image[:, 0, :2], axis=-1).mean() + 1e-6
    image = image + rng.normal(0.0, 0.01 * image_palm, image.shape)

    if rng.random() < 0.5:
        # Miroir horizontal de la scène : la trajectoire s'inverse, donc le label aussi.
        image[..., 0] = 2 * pivot[0, 0] - image[..., 0]
        world[..., 0] *= -1.0
        handedness = 1.0 - handedness
        target = int(mirror_map[target])
    return image.astype(np.float32), world.astype(np.float32), handedness, target
