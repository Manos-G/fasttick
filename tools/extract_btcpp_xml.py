#!/usr/bin/env python3
"""Extract the XML test data of BehaviorTree.CPP into tests/fixtures/btcpp/.

BehaviorTree.CPP (BT.CPP) is MIT-licensed. The FastTick XML tests use its
XML files and the XML text in its C++ examples and tests as round-trip
fixtures.

The script writes:

(a) each *.xml file under examples/, tests/ and fuzzing/corpus/bt_corpus/,
    and the ROS package manifest package.xml, at the same relative path;
(b) each C++ raw string literal (R"(...)" or R"delim(...)delim") in the
    C++ files under examples/, tests/ and sample_nodes/ that contains
    "<root", as inline/<source file stem>_<n>.xml. The number <n> counts
    these literals in each source file, from 1.

The script copies the bytes exactly. It uses only the Python standard
library.

Usage:
    tools/extract_btcpp_xml.py [--btcpp PATH] [--out PATH]
"""

from __future__ import annotations

import argparse
import pathlib
import shutil
import sys

XML_DIRS = ("examples", "tests", "fuzzing/corpus/bt_corpus")
XML_FILES = ("package.xml",)
CPP_DIRS = ("examples", "tests", "sample_nodes")
CPP_SUFFIXES = {".cpp", ".cc", ".cxx", ".h", ".hpp", ".hxx"}
RAW_PREFIXES = {"R", "u8R", "uR", "UR", "LR"}
# Characters that a raw-string delimiter cannot contain ([lex.string]).
BAD_DELIMITER_CHARS = set(" ()\\\t\v\f\r\n")

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_BTCPP = pathlib.Path("/home/user/ref/BehaviorTree.CPP")
DEFAULT_OUT = REPO_ROOT / "tests" / "fixtures" / "btcpp"


class ScanError(Exception):
    """The C++ scanner found a malformed raw string literal."""


def _is_ident_char(ch: str) -> bool:
    return ch.isalnum() or ch == "_"


def _in_number(src: str, i: int) -> bool:
    """Return True if the quote at src[i] is a digit separator (1'000)."""
    k = i
    while k > 0 and (_is_ident_char(src[k - 1]) or src[k - 1] in ".'"):
        k -= 1
    return k < i and src[k].isdigit()


def _skip_quoted(src: str, i: int, quote: str) -> int:
    """Skip an ordinary string or character literal that starts at src[i]."""
    j = i + 1
    n = len(src)
    while j < n:
        ch = src[j]
        if ch == "\\":
            j += 2
            continue
        if ch == quote or ch == "\n":
            return j + 1
        j += 1
    return n


def find_raw_strings(src: str) -> list[str]:
    """Return the contents of all raw string literals in C++ source text.

    The scanner skips comments, ordinary string literals and character
    literals, so that a raw string in a comment does not count.
    """
    found: list[str] = []
    i = 0
    n = len(src)
    while i < n:
        if src.startswith("//", i):
            j = src.find("\n", i)
            i = n if j < 0 else j + 1
            continue
        if src.startswith("/*", i):
            j = src.find("*/", i + 2)
            i = n if j < 0 else j + 2
            continue
        ch = src[i]
        if ch == '"':
            k = i
            while k > 0 and _is_ident_char(src[k - 1]):
                k -= 1
            if src[k:i] in RAW_PREFIXES:
                open_paren = src.find("(", i + 1)
                if open_paren < 0:
                    raise ScanError(f"raw string without '(' at offset {i}")
                delimiter = src[i + 1 : open_paren]
                if len(delimiter) > 16 or BAD_DELIMITER_CHARS & set(delimiter):
                    raise ScanError(f"bad raw string delimiter at offset {i}")
                end_marker = ")" + delimiter + '"'
                end = src.find(end_marker, open_paren + 1)
                if end < 0:
                    raise ScanError(f"unterminated raw string at offset {i}")
                found.append(src[open_paren + 1 : end])
                i = end + len(end_marker)
                continue
            i = _skip_quoted(src, i, '"')
            continue
        if ch == "'":
            if _in_number(src, i):
                i += 1
                continue
            i = _skip_quoted(src, i, "'")
            continue
        i += 1
    return found


def copy_xml_files(btcpp: pathlib.Path, out: pathlib.Path) -> list[pathlib.Path]:
    written = []
    for rel_dir in XML_DIRS:
        for path in sorted((btcpp / rel_dir).rglob("*.xml")):
            if not path.is_file():
                continue
            rel = path.relative_to(btcpp)
            target = out / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, target)
            written.append(target)
    for rel in XML_FILES:
        path = btcpp / rel
        if path.is_file():
            target = out / rel
            shutil.copyfile(path, target)
            written.append(target)
    return written


def extract_inline_xml(btcpp: pathlib.Path, out: pathlib.Path) -> list[pathlib.Path]:
    sources = []
    for rel_dir in CPP_DIRS:
        for path in sorted((btcpp / rel_dir).rglob("*")):
            if path.is_file() and path.suffix in CPP_SUFFIXES:
                sources.append(path)
    stems: dict[str, int] = {}
    for path in sources:
        stems[path.stem] = stems.get(path.stem, 0) + 1

    written = []
    inline_dir = out / "inline"
    for path in sources:
        text = path.read_bytes().decode("utf-8")
        try:
            literals = find_raw_strings(text)
        except ScanError as err:
            raise SystemExit(f"{path}: {err}") from err
        documents = [lit for lit in literals if "<root" in lit]
        if not documents:
            continue
        stem = path.stem
        if stems[stem] > 1:
            # Two source files have the same stem: add the directory.
            stem = "_".join(path.relative_to(btcpp).with_suffix("").parts)
        inline_dir.mkdir(parents=True, exist_ok=True)
        for number, document in enumerate(documents, start=1):
            target = inline_dir / f"{stem}_{number}.xml"
            target.write_bytes(document.encode("utf-8"))
            written.append(target)
    return written


def clean(out: pathlib.Path) -> None:
    """Remove the files of an earlier run. Keep README.md."""
    if not out.exists():
        return
    for path in sorted(out.rglob("*"), reverse=True):
        if path.is_file() and path.suffix == ".xml":
            path.unlink()
        elif path.is_dir() and not any(path.iterdir()):
            path.rmdir()


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--btcpp", type=pathlib.Path, default=DEFAULT_BTCPP,
                        help="path of the BehaviorTree.CPP source tree")
    parser.add_argument("--out", type=pathlib.Path, default=DEFAULT_OUT,
                        help="output directory")
    args = parser.parse_args(argv)
    btcpp = args.btcpp.resolve()
    out = args.out.resolve()
    if not (btcpp / "include" / "behaviortree_cpp").is_dir():
        print(f"error: {btcpp} is not a BehaviorTree.CPP source tree", file=sys.stderr)
        return 1
    clean(out)
    out.mkdir(parents=True, exist_ok=True)
    files = copy_xml_files(btcpp, out)
    inline = extract_inline_xml(btcpp, out)
    print(f"copied {len(files)} XML files and extracted {len(inline)} inline documents to {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
