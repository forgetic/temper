from __future__ import annotations

import contextlib
import io
import unittest
from typing import Any

import configure_forgejo


class FakeForgejo:
    def __init__(self, labels: list[dict[str, Any]]) -> None:
        self.labels = labels
        self.calls: list[tuple[str, str, dict[str, Any] | None]] = []

    def api(self, method: str, path: str, data: dict[str, Any] | None = None) -> Any:
        self.calls.append((method, path, data))
        if method == "GET" and path.endswith("/labels?page=1&limit=50"):
            return list(self.labels)
        if method == "POST" and path.endswith("/labels") and data is not None:
            self.labels.append({"id": 1, **data})
            return {"id": 1, **data}
        if method == "PATCH" and "/labels/" in path and data is not None:
            self.labels[0].update(data)
            return dict(self.labels[0])
        raise AssertionError(f"unexpected API call: {method} {path}")


class ConfigureForgejoTests(unittest.TestCase):
    def test_ci_workflow_requires_explicit_install_flag(self) -> None:
        with self.assertRaises(SystemExit):
            configure_forgejo.validate_ci_args(False, "ci.yml")
        with self.assertRaises(SystemExit):
            configure_forgejo.validate_ci_args(True, None)
        configure_forgejo.validate_ci_args(True, "ci.yml")
        configure_forgejo.validate_ci_args(False, None)

    def test_ensure_product_label_creates_missing_label(self) -> None:
        forgejo = FakeForgejo([])

        with contextlib.redirect_stdout(io.StringIO()):
            configure_forgejo.ensure_repo_label(
                forgejo,
                "ai",
                "temper",
                configure_forgejo.PRODUCT_LABEL_NAME,
                configure_forgejo.PRODUCT_LABEL_COLOR,
                configure_forgejo.PRODUCT_LABEL_DESCRIPTION,
            )

        self.assertEqual("POST", forgejo.calls[-1][0])
        self.assertEqual(configure_forgejo.PRODUCT_LABEL_NAME, forgejo.calls[-1][2]["name"])

    def test_ensure_product_label_patches_missing_description(self) -> None:
        forgejo = FakeForgejo([{"id": 7, "name": "product", "color": "5319e7", "description": ""}])

        with contextlib.redirect_stdout(io.StringIO()):
            configure_forgejo.ensure_repo_label(
                forgejo,
                "ai",
                "temper",
                configure_forgejo.PRODUCT_LABEL_NAME,
                configure_forgejo.PRODUCT_LABEL_COLOR,
                configure_forgejo.PRODUCT_LABEL_DESCRIPTION,
            )

        self.assertEqual("PATCH", forgejo.calls[-1][0])
        self.assertTrue(forgejo.calls[-1][1].endswith("/labels/7"))
        self.assertEqual(configure_forgejo.PRODUCT_LABEL_DESCRIPTION, forgejo.calls[-1][2]["description"])


if __name__ == "__main__":
    unittest.main()
