"""Codex trust bookkeeping must not hide material benchmark configuration drift."""

import copy
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from config_fingerprint import codex_config_fingerprint, configurations_match
from test_campaign import campaign


class ConfigFingerprintTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.config = {"model": "gpt-6-astra", "model_reasoning_effort": "xhigh",
                       "mcp_servers": {"codebase-memory-mcp": {"enabled": True}}}

    def fingerprint(self, config, layout="paired"):
        return codex_config_fingerprint(config, self.root, 5, layout=layout)

    def test_only_exact_generated_trusted_registrations_are_ignored(self):
        original = self.fingerprint(self.config)
        config = copy.deepcopy(self.config)
        config["projects"] = {str(self.root / f"pairs/{n:03}/codex/repo"):
                              {"trust_level": "trusted"} for n in range(1, 6)}
        snapshot = copy.deepcopy(config)
        self.assertEqual(self.fingerprint(config), original)
        self.assertEqual(config, snapshot)

        for path, value in [
            ("codex/repo", {"trust_level": "trusted"}),
            ("pairs/006/codex/repo", {"trust_level": "trusted"}),
            ("pairs/001/temper/repo", {"trust_level": "trusted"}),
            ("pairs/001/codex/elsewhere", {"trust_level": "trusted"}),
            ("unrelated", {"trust_level": "trusted"}),
            ("pairs/001/codex/repo", {"trust_level": "untrusted"}),
            ("pairs/001/codex/repo", {"trust_level": "trusted", "extra": True}),
        ]:
            with self.subTest(path=path, value=value):
                changed = copy.deepcopy(config)
                changed["projects"][str(self.root / path)] = value
                self.assertNotEqual(self.fingerprint(changed), original)

    def test_diagnostic_layouts_never_ignore_another_layouts_checkout(self):
        for layout in ["paired", "single", "none"]:
            original = self.fingerprint(self.config, layout)
            for path, allowed_layout in [("codex/repo", "single"),
                                         ("pairs/001/codex/repo", "paired"),
                                         ("pairs/005/codex/repo", "paired")]:
                with self.subTest(layout=layout, path=path):
                    changed = copy.deepcopy(self.config)
                    changed["projects"] = {str(self.root / path): {"trust_level": "trusted"}}
                    self.assertEqual(self.fingerprint(changed, layout) == original,
                                     layout == allowed_layout)

    def test_unknown_layout_is_rejected_even_without_project_registrations(self):
        with self.assertRaisesRegex(ValueError, "checkout layout"):
            self.fingerprint(self.config, "unknown")

    def test_model_provider_tier_mcp_and_instruction_changes_remain_material(self):
        for key, value in [
            ("model", "different-model"), ("model_reasoning_effort", "low"),
            ("model_provider", "other"), ("service_tier", "priority"),
            ("developer_instructions", "changed"),
            ("mcp_servers", {"codebase-memory-mcp": {"enabled": False}}),
        ]:
            with self.subTest(key=key):
                self.assertNotEqual(self.fingerprint(dict(self.config, **{key: value})),
                                    self.fingerprint(self.config))

    def test_configuration_evidence_retains_raw_hashes_and_rejects_other_drift(self):
        frozen = {"codex_config_sha256": "before", "codex_effective_config_sha256": "effective",
                  "codex_config_fingerprint_layout": "paired",
                  "binaries": {"codex": "frozen-binary"}}
        observed = dict(frozen, codex_config_sha256="after-trust-registration")
        with patch.object(campaign, "preflight", return_value=observed):
            evidence = campaign.check_configuration(None, frozen)
        self.assertTrue(evidence["matches_frozen"])
        self.assertEqual(evidence["observed"]["codex_config_sha256"], "after-trust-registration")
        self.assertEqual(frozen["codex_config_sha256"], "before")
        for change in [{"binaries": {"codex": "changed"}},
                       {"codex_config_fingerprint_layout": "single"},
                       {"codex_config_fingerprint_layout": "none"},
                       {"codex_effective_config_sha256": "changed"}]:
            with self.subTest(change=change):
                self.assertFalse(configurations_match(dict(observed, **change), frozen))
        self.assertFalse(configurations_match({"codex_config_sha256": "a"},
                                              {"codex_config_sha256": "b"}))


if __name__ == "__main__":
    unittest.main()
