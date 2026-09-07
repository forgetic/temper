# Worker versus Codex benchmark delivery declaration

Deliver `benchmark-contract.json` according to `report_contract.py`. This small
artifact declares the boundary of a Jig delivery scenario: it contains no
measured model time, real provider usage, or performance-parity claim.

Run `python3 -I -B report_contract.py benchmark-contract.json` and
`python3 -B -m unittest discover -s tests -v` before submission. Keep the
validator, tests, workflow, and this README unchanged. The required host
assertion separately runs the real benchmark accounting, transport-proxy, and
campaign tests from the exact Temper feature checkout after convergence.
