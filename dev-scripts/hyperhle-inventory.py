#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
"""Reproducible three-way source inventory; declarations are NOT verified APIs.

Run from the repository root. Does not checkout, execute or modify reference
code. Generated outputs retain every differing path, including non-Rust code.
"""
import argparse
import collections
import json
import pathlib
import re
import subprocess


def git(*args):
    return subprocess.check_output(['git', *args])


def tree(ref):
    result = {}
    for entry in git('ls-tree', '-r', '-z', ref).split(b'\0'):
        if entry:
            meta, name = entry.split(b'\t', 1)
            mode, kind, sha = meta.decode().split()
            result[name.decode()] = (mode, kind, sha)
    return result


def blobs(shas):
    shas = sorted(shas)
    process = subprocess.run(['git', 'cat-file', '--batch'],
                             input=('\n'.join(shas) + '\n').encode(),
                             stdout=subprocess.PIPE, check=True)
    data, offset, result = process.stdout, 0, {}
    for sha in shas:
        end = data.index(b'\n', offset)
        size = int(data[offset:end].split()[2])
        offset = end + 1
        result[sha] = data[offset:offset + size].decode('utf-8', errors='replace')
        offset += size + 1
    return result


def declarations(text):
    return set(re.findall(r'export_c_func!\(\s*(\w+)', text)) | {
        'class:' + name for name in re.findall(r'@implementation\s+(\w+)', text)
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--uf', default='d94c48f37bb5316aa2b87692b3aa6448f1fb20d6')
    parser.add_argument('--hyper', default='ff8eb8274130560efbafc56704e64ee7c5279572')
    parser.add_argument('--out', default='dev-docs/hyperhle')
    args = parser.parse_args()
    uf = git('rev-parse', args.uf).decode().strip()
    hyper = git('rev-parse', args.hyper).decode().strip()
    base = git('merge-base', uf, hyper).decode().strip()
    trees = [tree(ref) for ref in (base, uf, hyper)]
    all_paths = sorted(set().union(*trees))
    selected = [p for p in all_paths if trees[1].get(p) != trees[2].get(p)]
    source_shas = {value[2] for p in all_paths if p.endswith('.rs')
                   for t in trees for value in [t.get(p)] if value and value[1] == 'blob'}
    contents = blobs(source_shas)
    rows = []
    for path in selected:
        b, u, h = [t.get(path) for t in trees]
        category = ('both-modified' if u != b and h != b else
                    'uf-modified' if u != b else 'hyper-modified')
        if b is None:
            category = 'both-added' if u and h else 'uf-added' if u else 'hyper-added'
        def text(value):
            return contents.get(value[2], '') if value else ''
        us, hs = declarations(text(u)), declarations(text(h))
        rows.append(dict(path=path, category=category, base=b, uf=u, hyper=h,
                         candidate_hyper_exports=sorted(hs-us),
                         candidate_uf_exports=sorted(us-hs),
                         hyper_dependencies=sorted(set(re.findall(
                             r'(?:use|mod)\s+([\w:]+)', text(h)))),
                         review='pending-behavior-and-call-chain-review'))
    output = pathlib.Path(args.out)
    output.mkdir(parents=True, exist_ok=True)
    counts = dict(collections.Counter(row['category'] for row in rows))
    payload = dict(uf=uf, hyper=hyper, common_ancestor=base, counts=counts, files=rows)
    (output/'inventory.json').write_text(json.dumps(payload, ensure_ascii=False, indent=2)+'\n')
    # Compare declarations globally as well: moves between files must not be
    # mistaken for new functionality. Still requires runtime registration QA.
    maps = []
    for source_tree in trees[1:]:
        mapping = collections.defaultdict(list)
        for path, value in source_tree.items():
            if path.endswith('.rs') and value[1] == 'blob':
                for symbol in declarations(contents[value[2]]):
                    mapping[symbol].append(path)
        maps.append(mapping)
    unique = {name: {symbol: mapping[symbol] for symbol in sorted(mapping)
                     if symbol not in other}
              for name, mapping, other in [('hyper', maps[1], maps[0]),
                                           ('uf', maps[0], maps[1])]}
    (output/'unique-declarations.json').write_text(
        json.dumps(unique, ensure_ascii=False, indent=2)+'\n')
    lines = ['# Three-way inventory (generated)', '',
             'This is a coverage queue, not a correctness verdict. Export candidates may',
             'be relocated, stubbed, unregistered, or cfg-disabled. Inspect call chains.', '',
             f'UF `{uf}`; Hyper `{hyper}`; base `{base}`.', '',
             '| Path | Origin/difference | Hyper-only declaration candidates | Review |',
             '| -- | -- | -- | -- |']
    for row in rows:
        symbols = ', '.join(row['candidate_hyper_exports']) or '—'
        lines.append(f"| `{row['path']}` | {row['category']} | {symbols} | pending |")
    (output/'INVENTORY.md').write_text('\n'.join(lines)+'\n')
    # Keep complete non-merge history for provenance, not just recent subjects.
    for name, ref in [('uf', uf), ('hyper', hyper)]:
        (output/f'{name}-history.txt').write_bytes(git(
            'log', '--no-merges', '--format=%H %aI %an <%ae>%n%s', f'{base}..{ref}'))
    print(json.dumps(dict(uf=uf, hyper=hyper, base=base, counts=counts,
                          differing_paths=len(rows)), indent=2))


if __name__ == '__main__':
    main()
