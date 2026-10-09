"""Réseaux des classifieurs (§7.6). L'extraction des features et le softmax
font partie du module exporté.
"""

from __future__ import annotations

import torch
from torch import nn

from .features import DYNAMIC_FEATURES, STATIC_FEATURES, DynamicFeatures, StaticFeatures


class StaticMLP(nn.Module):
    """MLP 91 → 128 → 64 → K, ReLU, LayerNorm, dropout à l'entraînement (~20 k paramètres)."""

    def __init__(self, classes: int, dropout: float = 0.2) -> None:
        super().__init__()
        self.features = StaticFeatures()
        self.net = nn.Sequential(
            nn.Linear(STATIC_FEATURES, 128),
            nn.LayerNorm(128),
            nn.ReLU(),
            nn.Dropout(dropout),
            nn.Linear(128, 64),
            nn.LayerNorm(64),
            nn.ReLU(),
            nn.Dropout(dropout),
            nn.Linear(64, classes),
        )
        # Température de calibration, ajustée sur la validation (temperature scaling).
        self.register_buffer("temperature", torch.ones(()))

    def logits(self, world: torch.Tensor, handedness: torch.Tensor) -> torch.Tensor:
        return self.net(self.features(world, handedness))

    def forward(self, world: torch.Tensor, handedness: torch.Tensor) -> torch.Tensor:
        """`world [B, 21, 3]`, `handedness [B, 1]` → `probs [B, K]`."""
        return torch.softmax(self.logits(world, handedness) / self.temperature, dim=-1)


class DynamicTCN(nn.Module):
    """3 convolutions temporelles 1D (64 canaux, noyau 5, dilatations 1/2/4),
    pooling global, couche dense (~60 k paramètres).
    """

    def __init__(self, classes: int, dropout: float = 0.2) -> None:
        super().__init__()
        self.features = DynamicFeatures()
        layers: list[nn.Module] = []
        channels = DYNAMIC_FEATURES
        for dilation in (1, 2, 4):
            layers += [
                nn.Conv1d(channels, 64, kernel_size=5, dilation=dilation, padding=2 * dilation),
                nn.BatchNorm1d(64),
                nn.ReLU(),
                nn.Dropout(dropout),
            ]
            channels = 64
        self.tcn = nn.Sequential(*layers)
        self.head = nn.Linear(64, classes)
        self.register_buffer("temperature", torch.ones(()))

    def logits(self, image: torch.Tensor, world: torch.Tensor, handedness: torch.Tensor) -> torch.Tensor:
        x = self.features(image, world, handedness).transpose(1, 2)  # [B, C, T]
        x = self.tcn(x)
        # Pooling global : moyenne et dernier instant comptent tous deux pour
        # réagir dès la fin du mouvement.
        return self.head(0.5 * (x.mean(dim=-1) + x[..., -1]))

    def forward(self, image: torch.Tensor, world: torch.Tensor, handedness: torch.Tensor) -> torch.Tensor:
        """`image [B, T, 21, 3]`, `world [B, T, 21, 3]`, `handedness [B, 1]` → `probs [B, K]`."""
        return torch.softmax(self.logits(image, world, handedness) / self.temperature, dim=-1)


def count_parameters(model: nn.Module) -> int:
    return sum(p.numel() for p in model.parameters() if p.requires_grad)
