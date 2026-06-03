from __future__ import annotations

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import configure_forgejo
import label_intake
import write_interaction_bindings

DOGFOOD_DIR = Path(__file__).resolve().parents[1]
PROFILE_SPEC = DOGFOOD_DIR / "config" / "interaction-profiles" / "product-manager.json"


def load_profile() -> dict:
    spec = json.loads(PROFILE_SPEC.read_text())
    return spec["profiles"][0]


class DogfoodInteractionProfileTests(unittest.TestCase):
    def test_example_profile_encodes_product_chat_safety_policy(self) -> None:
        profile = load_profile()

        self.assertEqual("product-manager", profile["id"])
        self.assertEqual("product-manager-responder", profile["responder"])
        self.assertEqual("product-manager", profile["participants"]["agent"]["display_name"])
        self.assertEqual("human", profile["participants"]["human"]["display_name"])
        self.assertEqual(["product"], profile["transcript"]["labels"])
        self.assertEqual("exact", profile["transcript"]["label_policy"])
        self.assertEqual("product-chat", profile["transcript"]["marker_namespace"])
        self.assertEqual(["/file"], profile["commands"][0]["aliases"])

        effect = profile["acceptance_actions"][0]["effects"][0]
        self.assertEqual("create_issue", effect["kind"])
        self.assertEqual(["untriaged"], effect["labels"])
        self.assertEqual("product-chat", effect["marker_namespace"])
        self.assertEqual("file", effect["marker_key"])
        self.assertIn("Transcript: ${conversation.transcript_url}", effect["body_template"])
        self.assertIn("${effect.marker}", effect["body_template"])

    def test_config_and_labeler_align_with_example_transcript_label(self) -> None:
        labels = set(load_profile()["transcript"]["labels"])

        self.assertEqual({configure_forgejo.PRODUCT_LABEL_NAME}, labels)
        self.assertLessEqual(labels, label_intake.NON_WORKFLOW_SKIP_LABELS)

    def test_deployment_bindings_use_generic_temper_token_env_names(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "bindings.json"
            argv = [
                "write_interaction_bindings.py",
                "--out",
                str(out),
                "--base-url",
                "https://git.example.test",
                "--repo",
                "ai/temper",
                "--profile-id",
                "product-manager",
                "--responder-id",
                "product-manager-responder",
                "--responder-command",
                "/tmp/smith-product-manager-responder",
                "--responder-args-json",
                '["--auth","chatgpt-oauth"]',
                "--responder-env-allowlist",
                "OPENAI_API_KEY,SMITH_AUTH",
                "--responder-timeout-secs",
                "30",
            ]
            with mock.patch.object(sys, "argv", argv):
                with contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(0, write_interaction_bindings.main())

            data = json.loads(out.read_text())
            binding = data["profiles"]["product-manager"]
            self.assertEqual("TEMPER_INTERACTION_HUMAN_TOKEN", binding["human_token_env"])
            self.assertEqual("TEMPER_INTERACTION_AGENT_TOKEN", binding["agent_token_env"])
            responder = data["responders"]["product-manager-responder"]
            self.assertEqual(["--auth", "chatgpt-oauth"], responder["args"])
            self.assertEqual(["OPENAI_API_KEY", "SMITH_AUTH"], responder["env_allowlist"])
            self.assertEqual(30, responder["timeout_secs"])
            self.assertNotIn("TEMPER_PRODUCT_CHAT", out.read_text())


if __name__ == "__main__":
    unittest.main()
