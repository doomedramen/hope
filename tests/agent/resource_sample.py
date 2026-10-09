#!/usr/bin/env python3
"""Measure an unchanged Linux agent process; includes waited collector child CPU."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import time

parser = argparse.ArgumentParser()
parser.add_argument('--pid', type=int, required=True)
parser.add_argument('--binary', required=True)
parser.add_argument('--output', required=True)
parser.add_argument('--seconds', type=int, default=1800)
parser.add_argument('--warmup', type=int, default=60)
args = parser.parse_args()
ticks = os.sysconf('SC_CLK_TCK')

def read():
    stat = Path(f'/proc/{args.pid}/stat').read_text()
    fields = stat[stat.rfind(')') + 2:].split()
    status = Path(f'/proc/{args.pid}/status').read_text().splitlines()
    rss = next(int(line.split()[1]) * 1024 for line in status if line.startswith('VmRSS:'))
    return {'start_tick': fields[19], 'cpu_seconds': (int(fields[11]) + int(fields[12])) / ticks,
            'child_cpu_seconds': (int(fields[13]) + int(fields[14])) / ticks, 'rss_bytes': rss}

initial = read()
time.sleep(args.warmup)
baseline = read()
assert initial['start_tick'] == baseline['start_tick'], 'Agent restarted during warmup'
started = time.monotonic()
samples = []
while True:
    sample = read()
    assert sample['start_tick'] == baseline['start_tick'], 'Agent process changed'
    sample['elapsed_seconds'] = time.monotonic() - started
    samples.append(sample)
    if sample['elapsed_seconds'] >= args.seconds:
        break
    time.sleep(min(1, args.seconds - sample['elapsed_seconds']))
elapsed = samples[-1]['elapsed_seconds']
agent_cpu = samples[-1]['cpu_seconds'] - baseline['cpu_seconds']
child_cpu = samples[-1]['child_cpu_seconds'] - baseline['child_cpu_seconds']
cpu_model = next((line.split(':', 1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')), 'unavailable')
result = {'platform': 'linux', 'architecture': os.uname().machine, 'kernel': os.uname().release,
          'cpu_model': cpu_model, 'available_cpus': len(os.sched_getaffinity(0)),
          'os_release': Path('/etc/os-release').read_text(), 'memory_total_kib': next(line for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemTotal:')),
          'binary_sha256': hashlib.sha256(Path(args.binary).read_bytes()).hexdigest(),
          'warmup_seconds': args.warmup, 'elapsed_seconds': elapsed, 'agent_cpu_seconds': agent_cpu,
          'collector_child_cpu_seconds': child_cpu, 'cpu_percent_one_core_including_children': (agent_cpu + child_cpu) / elapsed * 100,
          'rss_median_bytes': statistics.median(sample['rss_bytes'] for sample in samples),
          'rss_peak_bytes': max(sample['rss_bytes'] for sample in samples), 'samples': samples}
output = Path(args.output)
output.parent.mkdir(parents=True, exist_ok=True)
temporary = output.with_suffix('.tmp')
temporary.write_text(json.dumps(result, indent=2) + '\n')
os.chmod(temporary, 0o600)
temporary.replace(output)
