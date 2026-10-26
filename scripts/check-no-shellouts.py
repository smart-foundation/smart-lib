#!/usr/bin/env python3
"""Fail if the engine spawns a process outside its tests.

Charter principle #1: the engine replaces shellouts with the OS's own calls
(`kill(2)` instead of `Command::new("kill")`, the kernel's copy instead of
`cp -r`). The clipboard domain is the sharpest case: it speaks the seat's
protocol rather than running `wl-paste`.

A grep cannot state that rule, because:

* `Command::new` appears in dozens of *comments* explaining what the code does
  not do;
* `DomainCommand::new` (the serve layer's own type) contains the same letters;
* and the tests legitimately spawn a child (`/bin/sleep`) to have a process to
  observe.

So this walks the tree, skips comments and string-free `#[cfg(test)]` modules,
and reports the file and line of anything that survives.

    ./scripts/check-no-shellouts.py [src/]
"""

import re
import sys
from pathlib import Path

# `Command::new` as a *path segment*, not the tail of a longer identifier.
SPAWN = re.compile(r"(?<![A-Za-z0-9_])Command::new\s*\(")
# Files where a spawn is the point of the code.
EXEMPT = ("src/codegen/", "src/bin/")


def strip_comments(line: str) -> str:
    """Drop a `//` comment, respecting string literals on the line."""
    out = []
    in_string = False
    escaped = False
    index = 0
    while index < len(line):
        character = line[index]
        if in_string:
            out.append(character)
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == '"':
                in_string = False
        else:
            if character == '"':
                in_string = True
            elif line.startswith("//", index):
                break
            out.append(character)
        index += 1
    return "".join(out)


def test_module_lines(lines: list[str]) -> set[int]:
    """The line indexes that sit inside a `#[cfg(test)]` module.

    Scanned forwards with a depth counter rather than backwards: a module's body
    ends where the brace count returns to where it began, which a backwards scan
    cannot see (it meets the *opening* braces of the test functions first).
    """
    inside: set[int] = set()
    bodies: list[int] = []  # the depth at which each test module's body opened
    depth = 0
    pending = False
    for index, raw in enumerate(lines):
        code = strip_comments(raw)
        if bodies:
            inside.add(index)
        if pending and re.search(r"\bmod\b[^{]*\{", code):
            bodies.append(depth)
            inside.add(index)
            pending = False
        if "#[cfg(test)]" in code:
            pending = True
        depth += code.count("{") - code.count("}")
        while bodies and depth <= bodies[-1]:
            bodies.pop()
    return inside


def main(argv: list[str]) -> int:
    root = Path(argv[1] if len(argv) > 1 else "src")
    findings = []
    for path in sorted(root.rglob("*.rs")):
        relative = path.as_posix()
        if any(relative.startswith(prefix) for prefix in EXEMPT):
            continue
        lines = path.read_text(encoding="utf8", errors="replace").splitlines()
        in_tests = test_module_lines(lines)
        for number, raw in enumerate(lines, start=1):
            code = strip_comments(raw)
            if not SPAWN.search(code):
                continue
            if (number - 1) in in_tests:
                continue
            findings.append((relative, number, raw.strip()))
    if findings:
        for relative, number, text in findings:
            print(f"::error file={relative},line={number}::a process spawn outside a test: {text}")
        print(f"{len(findings)} shellout(s) found", file=sys.stderr)
        return 1
    print("no process spawns outside the tests")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
