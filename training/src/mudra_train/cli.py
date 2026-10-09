"""`uv run mudra-train all` : entraîne, évalue et exporte les classifieurs (EF-63)."""

from __future__ import annotations

import argparse
import sys
from datetime import datetime
from pathlib import Path

from .data import dataset_hash, load_sessions
from .export import export_dynamic, export_static
from .synth import generate
from .train_dynamic import SAMPLE_RATE_HZ, WINDOW, train_dynamic
from .train_static import train_static

ROOT = Path(__file__).resolve().parents[3]


def _report_header(title: str) -> str:
    return f"# {title}\n\nGénéré le {datetime.now():%Y-%m-%d %H:%M} par `mudra-train`.\n\n"


def run_static(args: argparse.Namespace) -> bool:
    sessions = load_sessions(args.dataset, "static")
    if len({s.label for s in sessions}) < 2 or not any(s.label == "none" for s in sessions):
        print(
            f"Poses statiques : il faut au moins un geste et la classe « none » dans {args.dataset / 'static'}.\n"  # noqa: E501
            "  mudra record --label thumbs_up --static --duration 20s\n"
            "  mudra record --label none --static --duration 300s"
        )
        return False
    outcome = train_static(sessions, epochs=args.epochs, seed=args.seed)
    path = args.models / "static_gestures.onnx"
    gap = export_static(
        outcome.model, outcome.labels, path, args.golden, dataset_hash(sessions), outcome.samples
    )
    result = outcome.result
    print(
        f"  → {path} · {outcome.parameters} paramètres · {result.epochs} époques en {result.seconds:.0f} s · "
        f"écart ONNX/PyTorch {gap:.1e}"
    )
    report = _report_header("Classifieur de poses statiques")
    report += f"- Classes : {', '.join(outcome.labels)}\n- Paramètres : {outcome.parameters}\n"
    report += f"- Température de calibration : {result.temperature:.2f}\n"
    for warning in outcome.warnings:
        report += f"- ⚠ {warning}\n"
    if outcome.none_false_positives_per_hour is not None:
        report += (
            f"- Faux positifs sur les sessions « none » de validation : "
            f"{outcome.none_false_positives_per_hour:.1f} par heure (cible ENF-12 : ≤ 1)\n"
        )
    if outcome.report:
        report += "\n" + outcome.report.to_markdown("Validation (sessions jamais vues à l'entraînement)")
        verdict = "atteinte" if outcome.report.macro_f1 >= 0.97 else "NON atteinte"
        print(
            f"  macro-F1 de validation : {outcome.report.macro_f1 * 100:.2f} % (cible ENF-10 ≥ 97 % : {verdict})"  # noqa: E501
        )
    args.reports.mkdir(parents=True, exist_ok=True)
    (args.reports / "static.md").write_text(report, encoding="utf-8")
    return True


def run_dynamic(args: argparse.Namespace) -> bool:
    dynamic = load_sessions(args.dataset, "dynamic")
    static = load_sessions(args.dataset, "static")
    if not any(s.label != "none" for s in dynamic):
        print(
            f"Gestes dynamiques : aucun geste dans {args.dataset / 'dynamic'}.\n"
            "  mudra record --label swipe_left --dynamic --reps 15"
        )
        return False
    outcome = train_dynamic(dynamic, static, epochs=args.epochs, seed=args.seed)
    if not outcome.samples:
        print("Gestes dynamiques : aucun clip exploitable (clips trop courts ?).")
        return False
    path = args.models / "dynamic_gestures.onnx"
    gap = export_dynamic(
        outcome.model,
        outcome.labels,
        path,
        args.golden,
        dataset_hash(dynamic),
        outcome.samples,
        WINDOW,
        SAMPLE_RATE_HZ,
    )
    result = outcome.result
    print(
        f"  → {path} · {outcome.parameters} paramètres · {result.epochs} époques en {result.seconds:.0f} s · "
        f"écart ONNX/PyTorch {gap:.1e}"
    )
    report = _report_header("Classifieur de gestes dynamiques")
    report += f"- Classes : {', '.join(outcome.labels)}\n- Paramètres : {outcome.parameters}\n"
    report += f"- Fenêtre : {WINDOW} trames à {SAMPLE_RATE_HZ:.0f} Hz\n"
    for warning in outcome.warnings:
        report += f"- ⚠ {warning}\n"
    if outcome.report:
        report += "\n" + outcome.report.to_markdown("Validation (sessions jamais vues à l'entraînement)")
        print(f"  macro-F1 de validation : {outcome.report.macro_f1 * 100:.2f} %")
    args.reports.mkdir(parents=True, exist_ok=True)
    (args.reports / "dynamic.md").write_text(report, encoding="utf-8")
    return True


def main(argv: list[str] | None = None) -> int:
    # La console Windows n'est pas toujours en UTF-8.
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    parser = argparse.ArgumentParser(
        prog="mudra-train", description="Entraînement des classifieurs de gestes de Mudra"
    )
    parser.add_argument("command", choices=["all", "static", "dynamic", "synth"])
    parser.add_argument(
        "--dataset", type=Path, default=ROOT / "dataset", help="racine du dataset (défaut : ../dataset)"
    )
    parser.add_argument("--models", type=Path, default=ROOT / "models", help="dossier de sortie des .onnx")
    parser.add_argument(
        "--golden", type=Path, default=ROOT / "tests" / "golden", help="dossier des vecteurs dorés"
    )
    parser.add_argument("--reports", type=Path, default=ROOT / "training" / "reports")
    parser.add_argument("--epochs", type=int, default=80)
    parser.add_argument("--seed", type=int, default=0)
    args = parser.parse_args(argv)

    if args.command == "synth":
        generate(args.dataset, seed=args.seed)
        print(f"Dataset synthétique écrit dans {args.dataset}")
        return 0
    done = []
    if args.command in ("all", "static"):
        done.append(run_static(args))
    if args.command in ("all", "dynamic"):
        done.append(run_dynamic(args))
    if not any(done):
        return 1
    print(
        f"Rapports : {args.reports}. L'application recharge les modèles dès que les fichiers .onnx changent."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
