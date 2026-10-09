"""Chaîne d'apprentissage de Mudra (§8 du cahier des charges).

Python ne sert qu'hors ligne : lecture du dataset JSONL, entraînement des
classifieurs statique et dynamique, export ONNX avec métadonnées et vecteurs
dorés. L'application, elle, n'exécute aucun Python (D-01).
"""

#: Version du format d'entrée des modèles ; doit correspondre à
#: `mudra_gestures::onnx::FEATURE_VERSION` côté Rust.
FEATURE_VERSION = "1"

#: Version du format de dataset lue par ce paquet (`mudra-dataset::FORMAT_VERSION`).
DATASET_FORMAT_VERSION = 1

NONE_LABEL = "none"
