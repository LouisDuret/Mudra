"""Convertit le cahier des charges .docx en Markdown (§0 du cahier), sans pandoc.

    python scripts/docx_to_md.py Cahier_des_charges_Mudra.docx docs/cahier-des-charges.md

Le nom de travail « Gestura » du document d'origine est remplacé par « Mudra »,
le nom retenu pour l'application. Les figures sont extraites dans docs/images/.
"""

from __future__ import annotations

import re
import sys
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET

W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"
FIGURES = ["figure-1-pipeline.png", "figure-2-etats.png"]


def rename(text: str) -> str:
    text = text.replace("Gestura (nom de travail)", "Mudra")
    return text.replace("Gestura", "Mudra").replace("gestura", "mudra").replace("GESTURA", "MUDRA")


def run_text(paragraph: ET.Element) -> str:
    parts = []
    for node in paragraph.iter():
        if node.tag == f"{W}t":
            parts.append(node.text or "")
        elif node.tag == f"{W}tab":
            parts.append(" ")
        elif node.tag == f"{W}br":
            parts.append("\n")
    return rename("".join(parts))


def style_of(paragraph: ET.Element) -> str:
    style = paragraph.find(f"{W}pPr/{W}pStyle")
    return style.get(f"{W}val", "") if style is not None else ""


def is_code(paragraph: ET.Element) -> bool:
    """Vrai si tous les passages de texte du paragraphe sont en police à chasse fixe."""
    runs = [r for r in paragraph.iter(f"{W}r") if (r.findtext(f"{W}t") or "").strip()]
    if not runs:
        return False
    for run in runs:
        fonts = run.find(f"{W}rPr/{W}rFonts")
        name = fonts.get(f"{W}ascii", "") if fonts is not None else ""
        if not any(mono in name for mono in ("Consolas", "Courier", "Mono")):
            return False
    return True


def cell_text(cell: ET.Element) -> str:
    lines = [run_text(p).strip() for p in cell.iter(f"{W}p")]
    return "<br>".join(line.replace("\n", "<br>") for line in lines if line).replace("|", "\\|")


def table_md(table: ET.Element) -> list[str]:
    rows = [[cell_text(c) for c in row.findall(f"{W}tc")] for row in table.findall(f"{W}tr")]
    rows = [r for r in rows if r]
    if not rows:
        return []
    width = max(len(r) for r in rows)
    rows = [r + [""] * (width - len(r)) for r in rows]
    # Un tableau d'une seule cellule est un encadré.
    if width == 1:
        return [f"> {line}" for r in rows for line in r[0].split("<br>")] + [""]
    out = ["| " + " | ".join(rows[0]) + " |", "|" + "---|" * width]
    out += ["| " + " | ".join(r) + " |" for r in rows[1:]]
    return out + [""]


def convert(docx: Path, out: Path) -> None:
    images_dir = out.parent / "images"
    with zipfile.ZipFile(docx) as archive:
        body = ET.fromstring(archive.read("word/document.xml")).find(f"{W}body")
        rels = archive.read("word/_rels/document.xml.rels").decode("utf-8")
        targets = dict(re.findall(r'Id="([^"]+)"[^>]*Target="(media/[^"]+)"', rels))
        targets.update({i: t for t, i in re.findall(r'Target="(media/[^"]+)"[^>]*Id="([^"]+)"', rels)})
        lines: list[str] = []
        code: list[str] = []
        figure = 0

        def flush_code() -> None:
            if code:
                lines.extend(["```", *code, "```", ""])
                code.clear()

        for element in body:
            if element.tag == f"{W}tbl":
                flush_code()
                lines.extend(table_md(element))
                continue
            if element.tag != f"{W}p":
                continue
            blip = next((n for n in element.iter() if n.tag.endswith("}blip")), None)
            if blip is not None and figure < len(FIGURES):
                flush_code()
                rel = next((v for k, v in blip.attrib.items() if k.endswith("}embed")), "")
                if rel in targets:
                    images_dir.mkdir(parents=True, exist_ok=True)
                    (images_dir / FIGURES[figure]).write_bytes(archive.read("word/" + targets[rel]))
                    lines.extend([f"![Figure {figure + 1}](images/{FIGURES[figure]})", ""])
                    figure += 1
                continue
            text = run_text(element)
            if not text.strip():
                flush_code()
                continue
            style = style_of(element)
            if is_code(element) and not style.startswith("Heading"):
                code.extend(text.split("\n"))
                continue
            flush_code()
            if style == "Heading1":
                lines.extend([f"## {text.strip()}", ""])
            elif style == "Heading2":
                lines.extend([f"### {text.strip()}", ""])
            elif style == "Heading3":
                lines.extend([f"#### {text.strip()}", ""])
            elif style == "Title":
                lines.extend([f"# {text.strip()}", ""])
            elif style == "ListParagraph":
                lines.append(f"- {text.strip()}")
            else:
                if lines and lines[-1].startswith("- "):
                    lines.append("")
                lines.extend([text.strip().replace("\n", "  \n"), ""])
        flush_code()

    text = "\n".join(lines)
    text = re.sub(r"\n{3,}", "\n\n", text)
    # Les listes se terminent par une ligne vide avant le paragraphe suivant.
    text = re.sub(r"(\n- [^\n]*)\n(?!- |\n)", r"\1\n\n", text)
    note = (
        "<!-- Converti depuis Cahier_des_charges_Mudra.docx par scripts/docx_to_md.py. "
        "Le nom de travail « Gestura » y est remplacé par « Mudra ». -->\n\n"
    )
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(note + text.strip() + "\n", encoding="utf-8")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    convert(Path(sys.argv[1]), Path(sys.argv[2]))
    print(f"Écrit : {sys.argv[2]}")
