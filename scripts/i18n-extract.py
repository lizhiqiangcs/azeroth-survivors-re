#!/usr/bin/env python3
"""List the survivors UI's English strings that the Chinese catalog lacks.

usage: python scripts/i18n-extract.py [--all]

Reads every string literal in crates/benilla-app/src/survivors/*.rs and loading_screen.rs that
reads like display text (log lines, keys and paths skipped), turns `format!` placeholders into
`{}`, and prints the ones missing from crates/benilla-app/src/survivors/i18n/zh_CN.tsv, one per
line, ready to paste into it with a tab and the translation. `--all` prints every one.
"""
import io
import os
import re
import sys

ROOT = os.path.join(os.path.dirname(__file__), '..', 'crates', 'benilla-app', 'src')
SRC = [os.path.join(ROOT, 'survivors', f) for f in sorted(os.listdir(os.path.join(ROOT, 'survivors')))
       if f.endswith('.rs') and f not in ('auto.rs', 'i18n.rs')]
SRC.append(os.path.join(ROOT, 'loading_screen.rs'))
CATALOG = os.path.join(ROOT, 'survivors', 'i18n', 'zh_CN.tsv')

SKIP_CALL = re.compile(r'\b(info|warn|error|debug|trace|eprintln|println|panic|assert\w*|expect|unreachable|todo)!?\s*\($')
LIT = re.compile(r'"((?:[^"\\]|\\.)*)"', re.S)


def strings(path):
    text = io.open(path, encoding='utf-8').read()
    # Comments out, and the test modules.
    # Char literals first, since a '"' would open a string; then comments, though not a `://`.
    text = re.sub(r"'(?:\\.|[^'\\\n])'", "' '", text)
    text = re.sub(r'(?<![:"])//[^\n]*', '', text)
    cut = text.find('#[cfg(test)]')
    if cut >= 0:
        text = text[:cut]
    for m in LIT.finditer(text):
        before = text[max(0, m.start() - 60):m.start()].rstrip()
        if SKIP_CALL.search(before) or before.endswith('load(') or before.endswith('include_str!('):
            continue
        s = m.group(1)
        s = re.sub(r'\\\n\s*', '', s)
        s = s.replace('\\"', '"').replace('\\n', '\n').replace('\\\\', '\\')
        fmt = re.search(r'format!\s*\(\s*$|write!\s*\([^()]*,\s*$|writeln!\s*\([^()]*,\s*$', before)
        if fmt or re.search(r'\{[^{}]*\}', s):
            s = re.sub(r'\{\{', '\x01', s)
            s = re.sub(r'\}\}', '\x02', s)
            s = re.sub(r'\{[^{}]*\}', '{}', s)
            s = s.replace('\x01', '{').replace('\x02', '}')
        for line in s.split('\n'):
            line = line.strip()
            if keep(line):
                yield line


def keep(s):
    if not re.search(r'[A-Za-z]{2}', s):
        return False
    if re.search(r'[　-鿿]', s):          # already Chinese
        return False
    if re.fullmatch(r'[a-z0-9_:]+', s):       # keys
        return False
    if '\\' in s:                           # texture paths
        return False
    if '://' in s or re.search(r'\.(png|ttf|blp|m2|wav|mp3|txt|tsv|csv|md|wmo|adt)\b', s, re.I):
        return False
    if re.fullmatch(r'[A-Z_0-9]+', s) or s.startswith('#'):
        return False
    if re.fullmatch(r'\{\}|[{} ]+', s):
        return False
    return True


def catalog():
    if not os.path.exists(CATALOG):
        return set()
    out = set()
    for line in io.open(CATALOG, encoding='utf-8'):
        if '\t' in line and not line.startswith('#'):
            out.add(line.split('\t', 1)[0])
    return out


def main():
    have = catalog()
    seen, out = set(), []
    for p in SRC:
        for s in strings(p):
            if s not in seen:
                seen.add(s)
                if '--all' in sys.argv or s not in have:
                    out.append(s)
    sys.stdout.reconfigure(encoding='utf-8')
    for s in out:
        print(s)
    print(f'# {len(out)} strings', file=sys.stderr)


main()
