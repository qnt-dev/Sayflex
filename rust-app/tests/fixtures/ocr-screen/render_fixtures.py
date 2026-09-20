"""Render the immutable OCR benchmark corpus with the Windows text stack.

The checked-in PNG files are the benchmark input. This script exists only so
their origin can be audited and repeated on the release machine. It never runs
as part of the product build.
"""

from __future__ import annotations

import html
import json
import pathlib
import subprocess
import tempfile


ROOT = pathlib.Path(__file__).resolve().parent
MANIFEST = ROOT / "manifest.json"
EDGE_CANDIDATES = (
    pathlib.Path(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"),
    pathlib.Path(r"C:\Program Files\Microsoft\Edge\Application\msedge.exe"),
)


def edge_path() -> pathlib.Path:
    for candidate in EDGE_CANDIDATES:
        if candidate.is_file():
            return candidate
    raise SystemExit("Microsoft Edge was not found; corpus was not modified")


def fixture_html(fixture: dict[str, str]) -> tuple[str, int, int]:
    category = fixture["category"]
    dark = category == "dark"
    background = "#111317" if dark else "#f3f5f7"
    foreground = "#f5f6f8" if dark else "#111317"
    width, height, size = 1000, 180, 46
    direction = "rtl" if fixture["language"] == "ar" else "ltr"
    body = html.escape(fixture["expected"])

    if category == "small":
        width, height, size = 840, 120, 15
    elif category == "columns":
        width, height, size = 1100, 320, 36
        body = (
            '<div class="columns"><div>Left one<br>Left two</div>'
            '<div>Right one<br>Right two</div></div>'
        )
    elif category == "blank":
        width, height, size = 840, 180, 40
        body = ""

    document = f"""<!doctype html>
<meta charset="utf-8">
<style>
  * {{ box-sizing: border-box; }}
  html, body {{ width: {width}px; height: {height}px; margin: 0; overflow: hidden; }}
  body {{
    display: flex; align-items: center; justify-content: center;
    background: {background}; color: {foreground};
    font-family: "{fixture['font']}"; font-size: {size}px;
    font-weight: 400; line-height: 1.35; text-rendering: geometricPrecision;
  }}
  .text {{ white-space: pre; direction: {direction}; unicode-bidi: plaintext; }}
  .columns {{ display: grid; grid-template-columns: 1fr 1fr; gap: 160px; width: 82%; }}
  .columns > div {{ line-height: 1.75; }}
</style>
<div class="text" lang="{fixture['language']}">{body}</div>
"""
    return document, width, height


def main() -> None:
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    edge = edge_path()
    with tempfile.TemporaryDirectory(prefix="sayflex-ocr-fixtures-") as temp_dir:
        temp = pathlib.Path(temp_dir)
        for fixture in manifest["fixtures"]:
            document, width, height = fixture_html(fixture)
            source = temp / f"{fixture['file']}.html"
            source.write_text(document, encoding="utf-8")
            output = ROOT / fixture["file"]
            subprocess.run(
                [
                    str(edge),
                    "--headless=new",
                    "--hide-scrollbars",
                    "--disable-gpu",
                    "--force-device-scale-factor=1",
                    f"--window-size={width},{height}",
                    f"--screenshot={output}",
                    source.as_uri(),
                ],
                check=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            if not output.is_file():
                raise RuntimeError(f"Edge did not create {output}")


if __name__ == "__main__":
    main()
