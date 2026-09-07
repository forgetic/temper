#!/usr/bin/env bash
set -euo pipefail
python3 - "$1" <<'PY'
import json
from pathlib import Path
import sys
context = json.loads(Path(sys.argv[1]).read_text())
# Manual runs expose the aggregate's original name; retained feature validation
# copies that same registered log to the common provider-artifact filename.
names = {'shared-lifecycle-aggregate.jsonl', 'fake-codebase-memory-mcp.jsonl'}
paths = [Path(value) for value in context['run_evidence']['artifacts']['log_paths'] if Path(value).name in names]
assert len(paths) == 1, 'one retained lifecycle aggregate is required'
path = paths[0]
rows = [json.loads(line) for line in path.read_text().splitlines()]
expected = ['cold_parent_bootstrap', 'continuous_discovery_serving_admission', 'two_distinct_overlapping_attempts', 'real_forgejo_ownership_revoked', 'a_production_containment_complete', 'b_same_attempt_daemon_work_survived', 'b_exact_source_consumed_after_cleanup', 'late_a_has_no_authority', 'b_host_ci_merged_closed', 'last_session_natural_cleanup_before_teardown']
assert [row['checkpoint'] for row in rows] == expected
assert all(set(row) == {'checkpoint', 'passed', 'sequence'} and row['passed'] is True and row['sequence'] == index + 1 for index, row in enumerate(rows))
# CI retains assertion stdout even when temporary runtime logs are discarded.
# Publish only the already-validated closed facts, never private identities.
for row in rows:
    print(json.dumps(row, sort_keys=True))
print('Ten correlated lifecycle checkpoints passed; native index coalescing is validated separately.')
PY
