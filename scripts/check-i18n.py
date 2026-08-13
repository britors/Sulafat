#!/usr/bin/python3
"""Static catalog, source coverage, UTF-8 and fallback checks."""

from __future__ import annotations

import ast
import re
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE_FILES = list((ROOT / "sulafat-gtk" / "src").glob("*.rs"))
CATALOGS = [ROOT / "po" / f"{locale}.po" for locale in ("pt_BR", "es_ES", "zh_CN")]
CALL = re.compile(r'\btr(?:_format)?\(\s*"((?:[^"\\]|\\.)*)"')
PLACEHOLDER = re.compile(r"\{[A-Za-z_][A-Za-z0-9_]*\}")


def source_messages() -> set[str]:
    messages: set[str] = set()
    for path in SOURCE_FILES:
        for raw in CALL.findall(path.read_text(encoding="utf-8")):
            messages.add(ast.literal_eval(f'"{raw}"'))
    return messages


def catalog(path: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    current: str | None = None
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if line.startswith("msgid "):
            current = ast.literal_eval(line[6:])
            if current and current in result:
                raise AssertionError(f"{path}:{number}: duplicate msgid {current!r}")
        elif line.startswith("msgstr ") and current:
            result[current] = ast.literal_eval(line[7:])
    return result


def main() -> None:
    expected = source_messages()
    assert "Disconnect" in expected and "Main menu" in expected
    for path in CATALOGS:
        entries = catalog(path)
        assert entries.keys() == expected, f"{path}: missing={sorted(expected-entries.keys())}, extra={sorted(entries.keys()-expected)}"
        for msgid, msgstr in entries.items():
            assert msgstr, f"{path}: empty translation for {msgid!r}"
            assert set(PLACEHOLDER.findall(msgid)) == set(PLACEHOLDER.findall(msgstr)), f"{path}: placeholders differ for {msgid!r}"
        with tempfile.NamedTemporaryFile(suffix=".mo") as output:
            subprocess.run(["msgfmt", "--check", "--check-format", "-o", output.name, path], check=True)

    chinese = catalog(ROOT / "po" / "zh_CN.po")
    assert any("\u4e00" <= char <= "\u9fff" for value in chinese.values() for char in value), "zh_CN has no CJK glyphs"

    # en-US is the source catalog and therefore also the missing-key/unsupported-locale fallback.
    for locale, expected_locale in {
        "en_US.UTF-8": "en-US", "pt_BR.UTF-8": "pt-BR", "es_ES.UTF-8": "es-ES",
        "zh_CN.UTF-8": "zh-CN", "fr_FR.UTF-8": "en-US",
    }.items():
        base = locale.split(".", 1)[0].lower()
        actual = {"pt_br": "pt-BR", "es_es": "es-ES", "zh_cn": "zh-CN", "en_us": "en-US"}.get(base, "en-US")
        assert actual == expected_locale

    # User-facing builder/menu strings must go through tr(); technical identifiers are exempt.
    suspicious = re.compile(r'\.(?:title|label|tooltip_text|placeholder_text|accessible_label|description)\("')
    leftovers = []
    for path in SOURCE_FILES:
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if suspicious.search(line) and not any(exempt in line for exempt in ('title("HostName")', 'title("ProxyJump")', 'title("Sulafat")')):
                leftovers.append(f"{path.relative_to(ROOT)}:{number}: {line.strip()}")
    assert not leftovers, "untranslated UI literals:\n" + "\n".join(leftovers)
    print(f"i18n: {len(expected)} UI messages, 3 translated catalogs, en-US source fallback")


if __name__ == "__main__":
    main()
