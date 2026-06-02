#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from html import unescape
from html.parser import HTMLParser
from pathlib import Path


def _normalize_company(s: str) -> str:
    # Decode HTML entities and normalize whitespace.
    s = unescape(s)
    s = " ".join(s.split())
    return s.strip()


def _normalize_vendor_hex(s: str) -> str:
    s = s.strip().upper()
    # Keep only hex digits.
    s = "".join(ch for ch in s if ch in "0123456789ABCDEF")
    if not s:
        return ""
    return s.zfill(8)


@dataclass
class _RowState:
    vendor_id: str | None = None
    company: str | None = None

    def reset(self) -> None:
        self.vendor_id = None
        self.company = None

    def complete(self) -> bool:
        return bool(self.vendor_id and self.company)


class _VendorHtmlParser(HTMLParser):
    """
    Extracts pairs of:
      - <td role="vendor-id" data-search="XXXXXXXX">
      - <span role="company">Company name</span>
    within the same <tr>.
    """

    def __init__(self) -> None:
        super().__init__(convert_charrefs=False)
        self._in_tr = False
        self._in_company_span = False
        self._row = _RowState()
        self.rows: list[tuple[str, str]] = []

    @staticmethod
    def _attrs(attrs: list[tuple[str, str | None]]) -> dict[str, str]:
        return {k: (v or "") for (k, v) in attrs}

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        a = self._attrs(attrs)

        if tag == "tr":
            self._in_tr = True
            self._row.reset()
            return

        if not self._in_tr:
            return

        if tag == "td" and a.get("role") == "vendor-id":
            vendor = _normalize_vendor_hex(a.get("data-search", ""))
            if vendor:
                self._row.vendor_id = vendor
            return

        if tag == "span" and a.get("role") == "company":
            self._in_company_span = True
            return

    def handle_endtag(self, tag: str) -> None:
        if tag == "span" and self._in_company_span:
            self._in_company_span = False
            return

        if tag == "tr" and self._in_tr:
            if self._row.complete():
                self.rows.append((self._row.vendor_id or "", self._row.company or ""))
            self._in_tr = False
            self._in_company_span = False
            self._row.reset()
            return

    def handle_data(self, data: str) -> None:
        if not (self._in_tr and self._in_company_span):
            return
        text = _normalize_company(data)
        if not text:
            return
        # Company name is typically a single text node, but be safe.
        if self._row.company:
            self._row.company = _normalize_company(f"{self._row.company} {text}")
        else:
            self._row.company = text


def extract_vendor_map(html_text: str) -> dict[str, str]:
    parser = _VendorHtmlParser()
    parser.feed(html_text)
    parser.close()

    out: dict[str, str] = {}
    conflicts: list[tuple[str, str, str]] = []

    for vendor_id, company in parser.rows:
        vendor_id = _normalize_vendor_hex(vendor_id)
        company = _normalize_company(company)
        if not vendor_id or not company:
            continue

        if vendor_id in out:
            if out[vendor_id] != company:
                conflicts.append((vendor_id, out[vendor_id], company))
            continue
        out[vendor_id] = company

    for vendor_id, a, b in conflicts:
        print(
            f"WARNING: vendor {vendor_id} has conflicting companies: {a!r} vs {b!r} (kept first)",
            file=sys.stderr,
        )

    return out


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Extract CANopen Vendor ID -> Company mapping from raw HTML.")
    ap.add_argument("input", type=Path, help="Input raw HTML file (e.g. raw.txt)")
    ap.add_argument("output", type=Path, help="Output JSON file (e.g. data/canopen_vendor_ids.json)")
    ap.add_argument("--pretty", action="store_true", default=True, help="Pretty-print JSON (default)")
    args = ap.parse_args(argv)

    text = args.input.read_text(encoding="utf-8", errors="replace")
    vendor_map = extract_vendor_map(text)

    args.output.parent.mkdir(parents=True, exist_ok=True)
    if args.pretty:
        args.output.write_text(json.dumps(vendor_map, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    else:
        args.output.write_text(json.dumps(vendor_map, separators=(",", ":"), sort_keys=True) + "\n", encoding="utf-8")

    print(f"Extracted {len(vendor_map)} vendors -> {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

