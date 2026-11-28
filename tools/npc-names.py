#!/usr/bin/env python3
"""Generate crates/axidps-core/src/npc_names.rs from EVTC logs.

EVTC logs name an NPC agent by its species (the agent's `prof` field); the capture does not read
NPC names, so the core names them from this table. A species keeps its most common name, and only
when at least 90% of the logs that name it agree: a name chosen by a player (a renamed pet, say)
never wins. Placeholder names (`ch<species>-<instance>`, written when a name could not be read)
are skipped. Counts are per log, not per agent.

Usage: python3 -I tools/npc-names.py LOG_DIR... > crates/axidps-core/src/npc_names.rs
"""
import collections, json, os, re, struct, sys, zipfile

PLACEHOLDER = re.compile(r'^ch\d+-\d+$')

def npcs(path):
    with zipfile.ZipFile(path) as z, z.open(z.namelist()[0]) as f:
        n, = struct.unpack_from('<I', f.read(20), 16)
        b = f.read(n * 96)
    for i in range(n):
        o = i * 96
        _addr, prof, elite = struct.unpack_from('<QII', b, o)
        if elite == 0xFFFFFFFF and prof >> 16 != 0xFFFF:
            yield prof, b[o + 28:o + 92].split(b'\0')[0].decode('utf8', 'replace')

def main():
    names = collections.defaultdict(collections.Counter)
    logs = 0
    for d in sys.argv[1:]:
        for root, _, files in os.walk(d, followlinks=True):
            for f in files:
                # Logs axidps wrote itself are left out: the table is what other logs carry.
                if not f.endswith('.zevtc') or f.endswith('_axidps.zevtc'):
                    continue
                try:
                    found = {(s, n) for s, n in npcs(os.path.join(root, f))
                             if n and not PLACEHOLDER.match(n) and not n.startswith("axidps:")}
                except (OSError, zipfile.BadZipFile, struct.error):
                    continue
                for s, n in found:
                    names[s][n] += 1
                logs += 1
    print(f'// Generated from {logs} EVTC logs by tools/npc-names.py. Do not edit.')
    print('//! NPC names by species id, as EVTC logs carry them.')
    print()
    print('/// `(species id, name)`, sorted by id.')
    print('pub const NPC_NAMES: &[(u32, &str)] = &[')
    for s in sorted(names):
        name, top = names[s].most_common(1)[0]
        if top * 10 < sum(names[s].values()) * 9:
            continue
        # JSON string syntax is valid Rust here: UTF-8 kept as is, quotes and backslashes escaped.
        print(f'    ({s}, {json.dumps(name, ensure_ascii=False)}),')
    print('];')

if __name__ == '__main__':
    main()
