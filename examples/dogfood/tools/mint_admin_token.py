#!/usr/bin/env python3
"""Mint a Forgejo token using basic auth. Prints only the new token."""

from __future__ import annotations

import argparse
import base64
import json
import time
import urllib.error
import urllib.request


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--user", required=True)
    parser.add_argument("--password", required=True)
    args = parser.parse_args()

    payload = json.dumps({
        "name": f"temper-dogfood-{int(time.time())}",
        "scopes": ["all"],
    }).encode()
    basic = base64.b64encode(f"{args.user}:{args.password}".encode()).decode()
    req = urllib.request.Request(
        args.base_url.rstrip("/") + f"/api/v1/users/{args.user}/tokens",
        data=payload,
        headers={
            "Authorization": f"Basic {basic}",
            "Content-Type": "application/json",
            "Accept": "application/json",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            data = json.loads(resp.read().decode())
    except urllib.error.HTTPError as exc:
        body = exc.read(300).decode(errors="replace")
        raise SystemExit(f"mint token failed ({exc.code}): {body}") from None
    token = data.get("sha1")
    if not token:
        raise SystemExit("mint token response did not contain sha1")
    print(token)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
