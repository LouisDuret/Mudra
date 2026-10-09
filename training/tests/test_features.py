import numpy as np
import torch

from mudra_train.augment import augment_poses, augment_window
from mudra_train.features import DYNAMIC_FEATURES, STATIC_FEATURES, DynamicFeatures, StaticFeatures
from mudra_train.synth import POSES, hand_world
from mudra_train.train_static import mirror_map


def pose(label: str) -> torch.Tensor:
    fingers, thumb, pinch, roll = POSES[label]
    return torch.tensor(hand_world(fingers, thumb, pinch, roll), dtype=torch.float32)[None]


def test_static_feature_shape_and_finiteness():
    features = StaticFeatures()(pose("open_palm"), torch.ones(1, 1))
    assert features.shape == (1, STATIC_FEATURES) == (1, 91)
    assert torch.isfinite(features).all()


def test_left_hand_is_mirrored_onto_right_hand():
    extractor = StaticFeatures()
    right = pose("v_sign")
    left = right.clone()
    left[..., 0] *= -1.0
    a = extractor(right, torch.ones(1, 1))
    b = extractor(left, torch.zeros(1, 1))
    assert torch.allclose(a, b, atol=1e-5)


def test_features_are_scale_and_translation_invariant():
    extractor = StaticFeatures()
    world = pose("rock")
    moved = world * 1.7 + torch.tensor([0.3, -0.2, 0.1])
    assert torch.allclose(extractor(world, torch.ones(1, 1)), extractor(moved, torch.ones(1, 1)), atol=1e-4)


def test_flexion_angles_separate_open_hand_from_fist():
    extractor = StaticFeatures()
    angles = slice(63, 78)
    open_hand = extractor(pose("open_palm"), torch.ones(1, 1))[0, angles]
    fist = extractor(pose("fist"), torch.ones(1, 1))[0, angles]
    # Angles des doigts longs (hors pouce) : presque nuls main ouverte, grands poing fermé.
    assert open_hand[3:].max() < 0.1
    assert fist[3:].mean() > 0.4


def test_dynamic_features_keep_screen_direction_for_both_hands():
    extractor = DynamicFeatures()
    world = pose("open_palm").repeat(32, 1, 1)[None]
    image = world.clone() * 100.0
    image[0, :, :, 0] += torch.linspace(0.0, 30.0, 32)[:, None]  # déplacement vers la droite
    right = extractor(image, world, torch.ones(1, 1))
    left = extractor(image, world, torch.zeros(1, 1))
    assert right.shape == (1, 32, DYNAMIC_FEATURES)
    trajectory = slice(STATIC_FEATURES, STATIC_FEATURES + 2)
    # La trajectoire ne dépend pas de la latéralité ; elle part de la gauche (x négatif) vers 0.
    assert torch.allclose(right[..., trajectory], left[..., trajectory])
    assert right[0, 0, STATIC_FEATURES] < -1.0 and abs(float(right[0, -1, STATIC_FEATURES])) < 1e-5


def test_pose_augmentation_mirrors_labels_and_handedness():
    labels = ["none", "swipe_left", "swipe_right", "v_sign"]
    mapping = mirror_map(labels)
    assert mapping.tolist() == [0, 2, 1, 3]
    rng = np.random.default_rng(0)
    world = np.repeat(pose("v_sign").numpy(), 400, axis=0)
    out, handedness, targets = augment_poses(world, np.ones(400, np.float32), np.full(400, 1), mapping, rng)
    assert out.shape == world.shape and np.isfinite(out).all()
    mirrored = handedness == 0.0
    assert 120 < mirrored.sum() < 280
    assert (targets[mirrored] == 2).all() and (targets[~mirrored] == 1).all()
    # L'échelle reste dans les bornes de l'augmentation.
    palm = np.linalg.norm(out[:, 9] - out[:, 0], axis=-1) / np.linalg.norm(world[0, 9] - world[0, 0])
    assert 0.8 < palm.min() and palm.max() < 1.2


def test_window_augmentation_preserves_shape():
    rng = np.random.default_rng(1)
    world = np.repeat(pose("open_palm").numpy(), 32, axis=0)
    image = world * 100.0
    mapping = np.array([0, 2, 1])
    for _ in range(20):
        a, b, handedness, target = augment_window(image, world, 1.0, 1, mapping, rng)
        assert a.shape == b.shape == (32, 21, 3)
        assert (handedness, target) in {(1.0, 1), (0.0, 2)}
