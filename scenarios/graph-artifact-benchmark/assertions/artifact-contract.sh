#!/usr/bin/env bash
set -euo pipefail
python3 - "$1" <<'PY'
import json
from pathlib import Path
import sys
context = json.loads(Path(sys.argv[1]).read_text())
paths = [Path(value) for value in context['run_evidence']['artifacts']['log_paths']
         if Path(value).name == 'repo-populate.log']
assert len(paths) == 1, 'one retained repository verification log is required'
prefix = 'artifact-benchmark-fact '
rows = [json.loads(line[len(prefix):]) for line in paths[0].read_text().splitlines() if line.startswith(prefix)]
expected = ['exact-repository-harness-bytes', 'synthetic-evidence-cannot-authorize-adoption',
            'all-25-matched-samples-retained', 'all-four-failure-cases-recovered',
            'foreign-root-imports-refused-before-recovery',
            'source-symbol-call-coverage-and-deletion-checks-passed',
            'checksummed-manifest-and-sqlite-inspection', 'no-synthetic-timing-or-speedup',
            'closed-report-excludes-source-and-private-paths',
            'delivered-report-matches-repeated-harness', 'report-consumer-product-tests-passed',
            'report-verified-at-actual-merged-main']
assert [row['checkpoint'] for row in rows] == expected, 'all correlated merged report facts are required'
assert all(set(row) == {'checkpoint', 'passed'} and row['passed'] is True for row in rows)
for row in rows:
    print(json.dumps(row, sort_keys=True))
print('Twelve merged report checkpoints passed; native performance is measured separately.')
PY
