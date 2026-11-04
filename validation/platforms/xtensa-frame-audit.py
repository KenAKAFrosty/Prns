#!/usr/bin/env python3
"""Inspect actual ELF function ranges, excluding literal pools decoded as code.

This checks local immediate MOVSP patterns; it is not a whole-program proof.
No ELF or repository modifications are performed.
"""
import argparse
import bisect
import hashlib
import os
import collections
import json
import pathlib
import re
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('elf')
parser.add_argument('report', type=pathlib.Path)
parser.add_argument('--objdump', default=os.environ.get('XTENSA_OBJDUMP', 'xtensa-esp32s3-elf-objdump'))
args = parser.parse_args()
OBJDUMP = args.objdump
elf = args.elf
output = args.report
symbols = []
for line in subprocess.check_output([OBJDUMP, '-t', '-C', elf], text=True).splitlines():
    match = re.match(r'^([0-9a-f]+)\s+.*?\bF\s+\S+\s+([0-9a-f]+)\s+(.+)$', line)
    if match:
        address, size, name = match.groups()
        symbols.append((int(address, 16), int(size, 16), name))
symbols.sort()
starts = [s[0] for s in symbols]
instructions = collections.defaultdict(list)
for line in subprocess.check_output([OBJDUMP, '-d', '-C', elf], text=True).splitlines():
    match = re.match(r'^\s*([0-9a-f]+):\s+[0-9a-f]+\s+(.+)$', line)
    if not match:
        continue
    address = int(match[1], 16)
    index = bisect.bisect_right(starts, address) - 1
    if index < 0:
        continue
    start, size, _ = symbols[index]
    if address < start + size:
        instructions[index].append((address, re.sub(r',\s*', ',', match[2])))

adjustments = []
direct = []
for index, items in instructions.items():
    function = symbols[index][2]
    for position, (address, instruction) in enumerate(items):
        if re.match(r'^(?:addi(?:\.n)?|addmi|add(?:\.n)?|sub|and|or|mov(?:\.n)?)\s+a1,', instruction):
            direct.append({'function': function, 'address': hex(address), 'instruction': instruction})
        match = re.match(r'^movsp\s+a1,(a\d+)\s*$', instruction)
        if not match:
            continue
        register = match[1]
        before = items[max(0, position - 6):position]
        adjustment = None
        for _, candidate in reversed(before):
            immediate = re.match(r'^addi(?:\.n)?\s+' + register + r',a1,(-?\d+)\s*$', candidate)
            if immediate:
                adjustment = int(immediate[1])
                break
            if re.match(r'^\S+\s+' + register + ',', candidate):
                break
        adjustments.append({'function': function, 'address': hex(address), 'immediate': adjustment,
                            'context': [(hex(a), i) for a, i in before + [(address, instruction)]]})
bad = [a for a in adjustments if a['immediate'] is not None and a['immediate'] % 16]
unknown = [a for a in adjustments if a['immediate'] is None]
report = {'elf': elf, 'sha256': hashlib.sha256(pathlib.Path(elf).read_bytes()).hexdigest(), 'functions': len(symbols), 'instruction_count': sum(len(items) for items in instructions.values()), 'movsp_count': len(adjustments),
          'misaligned_immediate_count': len(bad), 'unknown_adjustment_count': len(unknown),
          'bad': bad, 'unknown': unknown, 'direct_sp_changes': direct,
          'all_adjustments': adjustments}
output.write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps({k: report[k] for k in ['elf', 'sha256', 'functions', 'instruction_count', 'movsp_count', 'misaligned_immediate_count', 'unknown_adjustment_count']}))
print('detailed_report=' + str(output))
for item in bad[:12]:
    print(item['address'], 'delta=' + str(item['immediate']), item['function'][:160])
