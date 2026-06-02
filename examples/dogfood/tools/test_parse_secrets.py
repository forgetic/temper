from __future__ import annotations

import contextlib
import io
import shlex
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import parse_secrets


REQUIRED_NOTE = """\
architect:architect-pw
api token: architect-token
engineer:engineer-pw
api token: engineer-token
reviewer:reviewer-pw
api token: reviewer-token
owner:owner-pw
api token: owner-token
bot:bot-pw
api token: bot-token
"""

PRODUCT_MANAGER_NOTE = """\
product-manager:pm-pw
api token: pm-token
"""

PRODUCT_MANAGER_SOURCE_NOTE = """\
pm-bot:pm-pw
api token: pm-token
"""


def load_env(path: Path) -> dict[str, str]:
    env: dict[str, str] = {}
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = shlex.split(line, posix=True)
        if len(parts) == 1 and "=" in parts[0]:
            key, value = parts[0].split("=", 1)
            env[key] = value
    return env


class ParseSecretsTests(unittest.TestCase):
    def run_parser(
        self,
        note: str,
        product_manager_user: str | None = None,
        product_chat_human_user: str | None = None,
    ) -> dict[str, str]:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            source = tmp_path / "note.txt"
            out = tmp_path / "roles.env"
            source.write_text(note)
            argv = [
                "parse_secrets.py",
                "--source",
                str(source),
                "--out",
                str(out),
                "--human-user",
                "bot",
                "--mechanical-user",
                "bot",
            ]
            if product_manager_user is not None:
                argv.extend(["--product-manager-user", product_manager_user])
            if product_chat_human_user is not None:
                argv.extend(["--product-chat-human-user", product_chat_human_user])
            with mock.patch.object(sys, "argv", argv):
                with contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(0, parse_secrets.main())
            return load_env(out)

    def test_emits_product_manager_and_permission_user_when_present(self) -> None:
        env = self.run_parser(REQUIRED_NOTE + PRODUCT_MANAGER_NOTE)

        self.assertEqual("product-manager", env["TEMPER_FORGEJO_USER_PRODUCT_MANAGER"])
        self.assertEqual("pm-token", env["TEMPER_FORGEJO_TOKEN_PRODUCT_MANAGER"])
        self.assertEqual("pm-pw", env["TEMPER_FORGEJO_PASSWORD_PRODUCT_MANAGER"])
        self.assertIn("product-manager", env["DOGFOOD_PERMISSION_USERS"].split())

    def test_aliases_product_manager_from_configured_source_user(self) -> None:
        env = self.run_parser(REQUIRED_NOTE + PRODUCT_MANAGER_SOURCE_NOTE, product_manager_user="pm-bot")

        self.assertEqual("pm-bot", env["TEMPER_FORGEJO_USER_PRODUCT_MANAGER"])
        self.assertEqual("pm-token", env["TEMPER_FORGEJO_TOKEN_PRODUCT_MANAGER"])
        self.assertIn("pm-bot", env["DOGFOOD_PERMISSION_USERS"].split())

    def test_aliases_product_chat_human_separately_from_workflow_human(self) -> None:
        env = self.run_parser(
            REQUIRED_NOTE + "free:free-pw\napi token: free-token\n",
            product_chat_human_user="free",
        )

        self.assertEqual("bot", env["TEMPER_FORGEJO_USER_HUMAN"])
        self.assertEqual("bot-token", env["TEMPER_FORGEJO_TOKEN_HUMAN"])
        self.assertEqual("free", env["TEMPER_FORGEJO_USER_PRODUCT_CHAT_HUMAN"])
        self.assertEqual("free-token", env["TEMPER_FORGEJO_TOKEN_PRODUCT_CHAT_HUMAN"])
        self.assertIn("free", env["DOGFOOD_PERMISSION_USERS"].split())

    def test_uses_admin_token_only_when_product_chat_human_is_admin_user(self) -> None:
        env = self.run_parser(
            "admin user: free\npw: free-pw\ngitnex api token: free-admin-token\n"
            + REQUIRED_NOTE,
            product_chat_human_user="free",
        )

        self.assertEqual("free", env["TEMPER_FORGEJO_USER_PRODUCT_CHAT_HUMAN"])
        self.assertEqual(
            "free-admin-token", env["TEMPER_FORGEJO_TOKEN_PRODUCT_CHAT_HUMAN"]
        )
        self.assertEqual("free-pw", env["TEMPER_FORGEJO_PASSWORD_PRODUCT_CHAT_HUMAN"])

        mismatch = self.run_parser(
            "admin user: root\npw: root-pw\ngitnex api token: root-admin-token\n"
            + REQUIRED_NOTE,
            product_chat_human_user="free",
        )
        self.assertNotIn("TEMPER_FORGEJO_TOKEN_PRODUCT_CHAT_HUMAN", mismatch)

    def test_succeeds_without_product_manager_credentials(self) -> None:
        env = self.run_parser(REQUIRED_NOTE)

        self.assertNotIn("TEMPER_FORGEJO_USER_PRODUCT_MANAGER", env)
        self.assertNotIn("TEMPER_FORGEJO_TOKEN_PRODUCT_MANAGER", env)
        self.assertNotIn("TEMPER_FORGEJO_USER_PRODUCT_CHAT_HUMAN", env)
        self.assertNotIn("product-manager", env["DOGFOOD_PERMISSION_USERS"].split())


if __name__ == "__main__":
    unittest.main()
