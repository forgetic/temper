#!/usr/bin/env bash
set -euo pipefail
python3 - "$1" <<'PYTHON'
import json
from pathlib import Path
import sys
context = json.loads(Path(sys.argv[1]).read_text())
paths = [Path(value) for value in context["run_evidence"]["artifacts"]["log_paths"]
         if Path(value).name == "repo-populate.log"]
assert len(paths) == 1, "one retained repository verification log is required"
prefix = "patch-creation-fact "
rows = [json.loads(line[len(prefix):]) for line in paths[0].read_text().splitlines()
        if line.startswith(prefix)]
assert rows == [{"checkpoint": "absent-seed-file-matches-merged-bytes", "passed": True}], \
    "exact created bytes must be verified against the actual merged commit"
print(json.dumps(rows[0], sort_keys=True))
PYTHON
