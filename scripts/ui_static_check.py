#!/usr/bin/env python3
"""Static UI checks for modeld C5.

This is intentionally conservative rather than pretending to be a full taint
engine. It enforces the invariants that caused real problems in the audit:

  1. the bearer token key exists in exactly one module (session.js);
  2. untagged template literals assigned to innerHTML never interpolate bare
     server-derived fields -- those must go through escapeHtml()/safeHtml;
  3. generated form controls have an explicit accessible name;
  4. index.html has the document basics Lighthouse checks (lang, viewport,
     one main landmark, labelled dialog, no positive tabindex, img alt).

Run from anywhere:  python3 scripts/ui_static_check.py
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
UI = ROOT / "crates" / "modeld-webui" / "ui"

JS_FILES = sorted(UI.glob("*.js")) + sorted((UI / "pages").glob("*.js"))

TOKEN_LITERAL = "modeld_token"
# Bare interpolations of fields that come from API responses or config.
# Wrapped forms (escapeHtml(m.name)) do not match because the `}` is not
# directly after the member chain.
BARE_UNTRUSTED = re.compile(
    r"\$\{\s*(?:m|d|g|a|s|cfg|e|item|model|file|entry)\."
    r"[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*"
    r"(?:\s*\|\|\s*[^{}]*)?\s*\}"
)
INLINE_EVENT_WITH_INTERPOLATION = re.compile(
    r"\bon(?:click|change|input|submit)\s*=\s*\"[^\"]*\$\{"
)
FORM_TAG = re.compile(r"<(input|select|textarea)\b([^>]*)>", re.DOTALL)
BUTTON_TAG = re.compile(r"<button\b([^>]*)>(.*?)</button>", re.DOTALL)
TAGGED_TEMPLATE = re.compile(r"safeHtml\s*$")


def iter_templates(text: str):
    """Yield (body, tagged, line) for every top-level JS template literal."""
    i = 0
    n = len(text)
    while i < n:
        if text[i] != "`":
            i += 1
            continue
        start = i + 1
        tagged = bool(TAGGED_TEMPLATE.search(text[max(0, i - 16):i]))
        line = text.count("\n", 0, i) + 1

        stack = [{"kind": "t"}]
        j = start
        end = None
        while j < n:
            ch = text[j]
            if ch == "\\":
                j += 2
                continue
            state = stack[-1]
            if state["kind"] == "t":
                if ch == "`":
                    stack.pop()
                    if not stack:
                        end = j
                        break
                    j += 1
                    continue
                if ch == "$" and j + 1 < n and text[j + 1] == "{":
                    stack.append({"kind": "e", "depth": 1})
                    j += 2
                    continue
                j += 1
                continue

            if ch in ("'", '"'):
                quote = ch
                j += 1
                while j < n and text[j] != quote:
                    j += 2 if text[j] == "\\" else 1
                j += 1
                continue
            if ch == "`":
                stack.append({"kind": "t"})
                j += 1
                continue
            if ch == "{":
                state["depth"] += 1
                j += 1
                continue
            if ch == "}":
                state["depth"] -= 1
                if state["depth"] == 0:
                    stack.pop()
                j += 1
                continue
            j += 1

        if end is None:
            yield text[start:], tagged, line
            break
        yield text[start:end], tagged, line
        i = end + 1


def check_token_source(errors: list[str]) -> list[tuple[str, int]]:
    hits = []
    for path in JS_FILES:
        text = path.read_text(encoding="utf-8")
        for number, line in enumerate(text.splitlines(), 1):
            if TOKEN_LITERAL in line and not line.lstrip().startswith("//"):
                hits.append((path.name, number))
    if len(hits) != 1 or hits[0][0] != "session.js":
        errors.append(
            f"token key must be declared only in session.js, found at {hits}"
        )
    return hits


def check_templates(errors: list[str]) -> tuple[int, int]:
    templates = 0
    controls = 0
    for path in JS_FILES:
        text = path.read_text(encoding="utf-8")
        for body, tagged, line in iter_templates(text):
            templates += 1
            if "<" not in body:
                continue

            if not tagged:
                for match in BARE_UNTRUSTED.finditer(body):
                    offset = body.count("\n", 0, match.start())
                    errors.append(
                        f"{path.name}:{line + offset}: raw template interpolates "
                        f"untrusted field {match.group(0)}; wrap with escapeHtml"
                    )
                if INLINE_EVENT_WITH_INTERPOLATION.search(body):
                    errors.append(
                        f"{path.name}:{line}: inline event handler contains an "
                        "interpolation; use data-* plus addEventListener"
                    )

            for match in FORM_TAG.finditer(body):
                attrs = match.group(2)
                if re.search(r'type\s*=\s*["\']hidden["\']', attrs):
                    continue
                controls += 1
                if "aria-label=" not in attrs and "aria-labelledby=" not in attrs:
                    tag = match.group(1)
                    errors.append(
                        f"{path.name}:{line}: generated <{tag}> has no accessible "
                        "name (add aria-label)"
                    )

            for match in BUTTON_TAG.finditer(body):
                attrs, content = match.group(1), match.group(2)
                label = re.sub(r"\s+", " ", content).strip()
                has_name = (
                    "aria-label=" in attrs
                    or "title=" in attrs
                    or bool(label)
                )
                if not has_name:
                    errors.append(
                        f"{path.name}:{line}: generated <button> has no accessible name"
                    )
    return templates, controls


def check_index(errors: list[str]) -> None:
    path = UI / "index.html"
    html = path.read_text(encoding="utf-8")

    if not re.search(r"<html[^>]*\blang\s*=\s*[\"'][A-Za-z-]+[\"']", html):
        errors.append("index.html: <html> has no valid lang attribute")
    if 'name="viewport"' not in html:
        errors.append("index.html: missing viewport meta tag")
    if len(re.findall(r"<main\b", html)) != 1:
        errors.append("index.html: expected exactly one <main> landmark")
    if re.search(r"tabindex\s*=\s*[\"'][1-9]\d*[\"']", html):
        errors.append("index.html: positive tabindex breaks natural focus order")
    for match in re.finditer(r"<img\b([^>]*)>", html, re.DOTALL):
        if not re.search(r"\balt\s*=", match.group(1)):
            errors.append("index.html: <img> is missing alt")
    dialog = re.search(r"<dialog\b([^>]*)>", html, re.DOTALL)
    if not dialog or "aria-labelledby=" not in dialog.group(1):
        errors.append("index.html: <dialog> has no aria-labelledby")


def main() -> int:
    errors: list[str] = []
    token_hits = check_token_source(errors)
    templates, controls = check_templates(errors)
    check_index(errors)

    if errors:
        print("UI static check: FAILED")
        for error in errors:
            print(f"  - {error}")
        return 1

    print(
        "UI static check: OK "
        f"({len(JS_FILES)} JS files, {len(token_hits)} token declaration, "
        f"{templates} templates, {controls} form controls checked)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
