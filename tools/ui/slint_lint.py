#!/usr/bin/env python3
"""A cheap static check of a directory of .slint files. It is NOT a compiler; it catches the typos
that are easy to make without one:

  * unbalanced (), [] and {}
  * `import { A } from "f.slint"` where f.slint is missing or does not export A
  * `Global.name` where the global has no such property, callback or function
  * `Name {` where Name is neither a built-in element nor defined or imported in the file
  * .slint files that no import chain from the root reaches

    python3 tools/ui/slint_lint.py ui-next/ui/lib.slint
"""
import re
import sys
from pathlib import Path

BUILTINS = {
    "Rectangle", "Text", "Image", "Path", "TouchArea", "Flickable", "HorizontalLayout",
    "VerticalLayout", "GridLayout", "Timer", "Window", "FocusScope", "TextInput", "Dialog",
    "PopupWindow", "Row", "MenuBar", "DragArea", "DropArea", "SwipeGestureHandler",
    "ScaleRotateGestureHandler", "ComponentContainer", "Clip", "Empty", "PathLineTo", "PathMoveTo",
    "PathArcTo", "PathCubicTo", "PathQuadraticTo", "PathClose", "MoveTo", "LineTo", "ArcTo",
    "CubicTo", "QuadraticTo", "Close",
}
KEYWORD_CTX = re.compile(r"(?:component|global|struct|enum)\s+$")


def strip(text):
    """Blank out comments and string contents, keeping everything else (and line numbers)."""
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if text.startswith("//", i):
            while i < n and text[i] != "\n":
                i += 1
        elif text.startswith("/*", i):
            j = text.find("*/", i + 2)
            j = n if j < 0 else j + 2
            out.append(re.sub(r"[^\n]", " ", text[i:j]))
            i = j
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            out.append('"' + " " * (j - i - 1) + '"')
            i = j + 1
        else:
            out.append(c)
            i += 1
    return "".join(out)


def raw_strings(text):
    return re.findall(r'"((?:[^"\\]|\\.)*)"', text)


class File:
    def __init__(self, path):
        self.path = path
        self.raw = path.read_text(encoding="utf-8").replace("\r", "")
        self.src = strip(self.raw)
        self.defined = set(re.findall(r"\b(?:component|global|struct|enum)\s+([A-Za-z][\w-]*)", self.src))
        self.exported = set(
            re.findall(r"\bexport\s+(?:component|global|struct|enum)\s+([A-Za-z][\w-]*)", self.src)
        )
        for names in re.findall(r"\bexport\s*\{([^}]*)\}", self.src):
            self.exported |= {x.strip().split(" as ")[-1].strip() for x in names.split(",") if x.strip()}
        self.imports = []  # (names, file)
        for m in re.finditer(r"\bimport\s*\{([^}]*)\}\s*from\s*\"([^\"]+)\"", self.raw):
            names = [x.strip().split(" as ")[-1].strip() for x in m.group(1).split(",") if x.strip()]
            self.imports.append((names, m.group(2)))
        self.reexports = re.findall(r"\bexport\s*\{[^}]*\}\s*from\s*\"([^\"]+)\"", self.raw)
        self.globals = {}
        for m in re.finditer(r"\bglobal\s+([A-Za-z][\w-]*)\s*\{", self.src):
            body, depth, i = [], 1, m.end()
            while i < len(self.src) and depth:
                depth += {"{": 1, "}": -1}.get(self.src[i], 0)
                i += 1
            members = set()
            text = self.src[m.end() : i - 1]
            members |= set(re.findall(r"\bproperty\s*<[^>]*>\s*([\w-]+)", text))
            members |= set(re.findall(r"\bcallback\s+([\w-]+)", text))
            members |= set(re.findall(r"\bfunction\s+([\w-]+)", text))
            self.globals[m.group(1)] = members


def main(root_arg):
    root = Path(root_arg).resolve()
    base = root.parent
    files, problems = {}, []

    def load(p):
        p = p.resolve()
        if p in files:
            return files[p]
        if not p.exists():
            return None
        files[p] = File(p)
        for _, rel in files[p].imports:
            load(p.parent / rel)
        for rel in files[p].reexports:
            load(p.parent / rel)
        return files[p]

    load(root)
    for p in sorted(base.rglob("*.slint")):
        if p.resolve() not in files:
            problems.append(f"{p.relative_to(base)}: not reachable from {root.name}")

    all_globals = {}
    for f in files.values():
        all_globals.update(f.globals)

    for p, f in files.items():
        rel = p.relative_to(base)
        for a, b in (("(", ")"), ("[", "]"), ("{", "}")):
            if f.src.count(a) != f.src.count(b):
                problems.append(f"{rel}: unbalanced {a}{b} ({f.src.count(a)} vs {f.src.count(b)})")
        visible = set(f.defined) | BUILTINS
        for names, relp in f.imports:
            if relp == "std-widgets.slint":
                # the app's Slint build has no styles (and so none of their SVG assets): a stock
                # widget compiles in Slint but fails in rustc, as ListView did
                problems.append(f"{rel}: std-widgets.slint cannot be used in this build")
                continue
            target = files.get((p.parent / relp).resolve())
            if target is None:
                problems.append(f"{rel}: imports missing file {relp}")
                continue
            for n in names:
                if n not in target.exported:
                    problems.append(f"{rel}: {relp} does not export {n}")
                visible.add(n)
        for m in re.finditer(r"(?<![\w.-])([A-Z][A-Za-z0-9]*)\s*\{", f.src):
            name = m.group(1)
            if name not in visible and not KEYWORD_CTX.search(f.src[: m.start()]):
                line = f.src.count("\n", 0, m.start()) + 1
                problems.append(f"{rel}:{line}: unknown element {name}")
        for m in re.finditer(r"\binherits\s+([A-Z][A-Za-z0-9]*)", f.src):
            if m.group(1) not in visible:
                problems.append(f"{rel}: inherits unknown {m.group(1)}")
        for g, members in all_globals.items():
            if g not in visible:
                continue
            for m in re.finditer(rf"(?<![\w.-]){g}\.([a-z][\w-]*)", f.src):
                member = m.group(1).rstrip("-")
                if member not in members:
                    line = f.src.count("\n", 0, m.start()) + 1
                    problems.append(f"{rel}:{line}: {g}.{member} is not declared")

    print(f"checked {len(files)} files from {root.name}")
    for p in problems:
        print("  PROBLEM", p)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
