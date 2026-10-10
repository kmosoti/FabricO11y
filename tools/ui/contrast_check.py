#!/usr/bin/env python3
"""Check selected foreground/background pairs in the Fabric UI palette.

This is a deterministic sRGB relative-luminance contrast check for text. It is
not a complete WCAG conformance or accessibility audit.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CSS = ROOT / "crates/fabric-ui/style.css"
THRESHOLD = 4.5


def luminance(hex_color: str) -> float:
    match = re.fullmatch(r"#([0-9a-fA-F]{6})", hex_color.strip())
    if not match:
        raise ValueError(f"expected six-digit sRGB hex color, got {hex_color!r}")
    channels = [int(match.group(1)[i : i + 2], 16) / 255 for i in (0, 2, 4)]
    linear = [v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4 for v in channels]
    return sum(c * value for c, value in zip((0.2126, 0.7152, 0.0722), linear))


def contrast(foreground: str, background: str) -> float:
    high, low = sorted((luminance(foreground), luminance(background)), reverse=True)
    return (high + 0.05) / (low + 0.05)


def declarations(css: str, selector: str) -> dict[str, str]:
    match = re.search(re.escape(selector) + r"\s*\{([^}]*)\}", css, re.DOTALL)
    if not match:
        raise ValueError(f"missing CSS selector {selector}")
    found = re.findall(r"(--[\w-]+)\s*:\s*(#[0-9a-fA-F]{3,8})\s*;", match.group(1))
    return {name: expand_hex(value) for name, value in found}


def css_property(css: str, selector: str, prop: str) -> str:
    match = re.search(re.escape(selector) + r"\s*\{([^}]*)\}", css, re.DOTALL)
    if not match:
        raise ValueError(f"missing CSS selector {selector}")
    value = re.search(r"\b" + re.escape(prop) + r"\s*:\s*(#[0-9a-fA-F]{3,8})", match.group(1))
    if not value:
        raise ValueError(f"could not read {prop} from {selector}")
    return expand_hex(value.group(1))


def property_pair(css: str, selector: str, foreground: str, background: str) -> tuple[str, str]:
    return css_property(css, selector, foreground), css_property(css, selector, background)


def expand_hex(value: str) -> str:
    value = value.strip()
    if re.fullmatch(r"#[0-9a-fA-F]{3}", value):
        value = "#" + "".join(ch * 2 for ch in value[1:])
    if not re.fullmatch(r"#[0-9a-fA-F]{6}", value):
        raise ValueError(f"unsupported non-opaque sRGB color {value!r}")
    return value.lower()


def result(name: str, foreground: str, background: str, *, threshold: float = THRESHOLD) -> dict:
    ratio = contrast(foreground, background)
    return {
        "name": name,
        "foreground": foreground.lower(),
        "background": background.lower(),
        "ratio": round(ratio, 3),
        "threshold": threshold,
        "pass": ratio >= threshold,
    }


def controls() -> list[dict]:
    checks = [
        result("control.black_on_white", "#000000", "#ffffff", threshold=21.0),
        result("control.white_on_black", "#ffffff", "#000000", threshold=21.0),
        result("control.initial_muted_pair_rejected", "#637987", "#f6f8fa"),
        result("control.initial_cyan_tint_pair_rejected", "#087f93", "#e9f8fa"),
    ]
    if not all(check["pass"] for check in checks[:2]):
        raise AssertionError("black/white contrast controls must equal 21:1")
    if any(check["pass"] for check in checks[2:]):
        raise AssertionError("known sub-4.5:1 negative controls must be rejected")
    if checks[2]["ratio"] != 4.274 or checks[3]["ratio"] != 4.314:
        raise AssertionError("negative-control ratios changed unexpectedly")
    return checks


def palette_checks(css: str) -> list[dict]:
    light = declarations(css, ".console")
    dark = declarations(css, ".console.dark")
    checks = []
    for theme, tokens in (("light", light), ("dark", dark)):
        for foreground in ("--text", "--muted", "--cyan"):
            for background in ("--bg", "--surface"):
                checks.append(result(f"{theme}.{foreground}_on_{background}", tokens[foreground], tokens[background]))
        checks.append(result(f"{theme}.--cyan_on_--tint", tokens["--cyan"], tokens["--tint"]))

    for selector in (".badge.warning", ".dark .badge.warning"):
        fg, bg = property_pair(css, selector, "color", "background")
        checks.append(result(f"{selector}.text", fg, bg))
    fg, bg = property_pair(css, ".primary", "color", "background")
    checks.append(result(".primary.text", fg, bg))
    terminal_bg = css_property(css, ".tail-terminal", "background")
    checks.append(result(".tail-terminal.text", css_property(css, ".tail-terminal", "color"), terminal_bg))
    checks.append(result(".tail.timestamp", css_property(css, ".tail-line>span:first-child", "color"), terminal_bg))
    checks.append(result(".tail.highlight", css_property(css, ".tail-line b", "color"), terminal_bg))
    return checks


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--css", type=Path, default=DEFAULT_CSS)
    parser.add_argument("--output", type=Path, help="write JSON result to this path")
    args = parser.parse_args()
    css = args.css.read_text(encoding="utf-8")
    report = {
        "scope": "selected opaque sRGB text contrast pairs; not full accessibility conformance",
        "algorithm": "WCAG relative luminance and contrast ratio, sRGB",
        "threshold": THRESHOLD,
        "css": str(args.css.resolve()),
        "css_sha256": hashlib.sha256(css.encode("utf-8")).hexdigest(),
        "generated_utc": datetime.now(timezone.utc).isoformat(),
        "controls": controls(),
        "checks": palette_checks(css),
    }
    report["passed"] = all(check["pass"] for check in report["checks"])
    report["failures"] = [check for check in report["checks"] if not check["pass"]]
    encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded, encoding="utf-8")
    sys.stdout.write(encoded)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
