"""Extraction des features, intégrée au graphe ONNX exporté (D-09).

Source de vérité unique : ce module est exporté avec chaque modèle, de sorte
que Rust ne fournit que les points-clés lissés et la latéralité. Seules des
opérations exportables sont utilisées (pas d'atan2 : acos borné).
"""

from __future__ import annotations

import torch
from torch import nn

#: Triplets (a, b, c) : flexion à l'articulation b. 3 par doigt, 15 au total.
ANGLE_TRIPLETS = [
    (0, 1, 2), (1, 2, 3), (2, 3, 4),  # pouce
    (0, 5, 6), (5, 6, 7), (6, 7, 8),  # index
    (0, 9, 10), (9, 10, 11), (10, 11, 12),  # majeur
    (0, 13, 14), (13, 14, 15), (14, 15, 16),  # annulaire
    (0, 17, 18), (17, 18, 19), (18, 19, 20),  # auriculaire
]  # fmt: skip
TIPS = [4, 8, 12, 16, 20]

#: 63 coordonnées + 15 angles + 4 distances pouce-bouts + 3 distances entre
#: bouts voisins + normale de paume (3) + axe poignet → majeur (3).
STATIC_FEATURES = 63 + 15 + 4 + 3 + 3 + 3
#: Features statiques + position (2) et vitesse (2) du centre de la main.
DYNAMIC_FEATURES = STATIC_FEATURES + 4

_EPS = 1e-6


def _normalize(v: torch.Tensor) -> torch.Tensor:
    return v / (v.norm(dim=-1, keepdim=True) + _EPS)


class StaticFeatures(nn.Module):
    """Features d'une pose : `world [..., 21, 3]`, `handedness [..., 1]` → `[..., 91]`."""

    def __init__(self) -> None:
        super().__init__()
        a, b, c = zip(*ANGLE_TRIPLETS, strict=True)
        self.register_buffer("a", torch.tensor(a), persistent=False)
        self.register_buffer("b", torch.tensor(b), persistent=False)
        self.register_buffer("c", torch.tensor(c), persistent=False)
        self.register_buffer("tips", torch.tensor(TIPS), persistent=False)

    def forward(self, world: torch.Tensor, handedness: torch.Tensor) -> torch.Tensor:
        # Miroir de la forme pour la main gauche : un seul modèle pour les deux mains.
        sign = handedness * 2.0 - 1.0  # gauche → -1, droite → +1
        flip = torch.cat([sign, torch.ones_like(sign), torch.ones_like(sign)], dim=-1)
        points = world * flip.unsqueeze(-2)

        # Translation au poignet, normalisation par la distance poignet → MCP du majeur.
        points = points - points[..., 0:1, :]
        scale = points[..., 9, :].norm(dim=-1, keepdim=True).unsqueeze(-1) + _EPS
        points = points / scale

        # 15 angles de flexion (acos borné, divisé par pi).
        u = _normalize(points.index_select(-2, self.b) - points.index_select(-2, self.a))
        v = _normalize(points.index_select(-2, self.c) - points.index_select(-2, self.b))
        cosine = (u * v).sum(dim=-1).clamp(-1.0 + 1e-4, 1.0 - 1e-4)
        angles = torch.acos(cosine) / torch.pi

        # Distances bout du pouce ↔ autres bouts, et entre bouts voisins.
        tips = points.index_select(-2, self.tips)
        thumb_to_tips = (tips[..., 1:, :] - tips[..., 0:1, :]).norm(dim=-1)
        neighbours = (tips[..., 2:, :] - tips[..., 1:-1, :]).norm(dim=-1)

        # Orientation de la paume : normale et axe poignet → majeur.
        # Produit vectoriel écrit à la main : exportable quel que soit l'opset.
        p, q = points[..., 5, :], points[..., 17, :]
        cross = torch.stack(
            [
                p[..., 1] * q[..., 2] - p[..., 2] * q[..., 1],
                p[..., 2] * q[..., 0] - p[..., 0] * q[..., 2],
                p[..., 0] * q[..., 1] - p[..., 1] * q[..., 0],
            ],
            dim=-1,
        )
        normal = _normalize(cross)
        axis = _normalize(points[..., 9, :])

        return torch.cat(
            [points.flatten(start_dim=-2), angles, thumb_to_tips, neighbours, normal, axis],
            dim=-1,
        )


class DynamicFeatures(nn.Module):
    """Features d'une fenêtre : `image [B, T, 21, 3]`, `world [B, T, 21, 3]`,
    `handedness [B, 1]` → `[B, T, 95]`.
    """

    PALM_IDS = (0, 5, 9, 13, 17)

    def __init__(self) -> None:
        super().__init__()
        self.static = StaticFeatures()
        self.register_buffer("palm", torch.tensor(self.PALM_IDS), persistent=False)

    def forward(self, image: torch.Tensor, world: torch.Tensor, handedness: torch.Tensor) -> torch.Tensor:
        frames = image.shape[1]
        # Forme de la main à chaque trame (le miroir gauche → droite s'y applique).
        shape = self.static(world, handedness.unsqueeze(1).expand(-1, frames, -1))

        # Trajectoire du centre de la main dans l'image, relative à la dernière
        # trame, normalisée par la taille de paume moyenne. Le miroir ne s'y
        # applique jamais : gauche et droite gardent leur sens à l'écran.
        xy = image[..., :2]
        center = xy.index_select(-2, self.palm).mean(dim=-2)  # [B, T, 2]
        palm_size = (xy[..., 9, :] - xy[..., 0, :]).norm(dim=-1).mean(dim=1, keepdim=True).unsqueeze(
            -1
        ) + _EPS
        trajectory = (center - center[:, -1:, :]) / palm_size
        # Vitesses par différences finies (la première vaut zéro).
        velocity = torch.cat(
            [torch.zeros_like(trajectory[:, :1]), trajectory[:, 1:] - trajectory[:, :-1]], dim=1
        )
        return torch.cat([shape, trajectory, velocity * 8.0], dim=-1)
