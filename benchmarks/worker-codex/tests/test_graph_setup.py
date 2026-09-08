"""Namespace setup reads provider state without indexing or invoking a model."""

import hashlib
import json
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import codex_run
from graph_setup import developer_instructions, missing_status, prepare_namespace


SERVER = """
import json,sys,time
mode=sys.argv[1]
for line in sys.stdin:
 request=json.loads(line)
 if request['method']=='initialize':
  print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':'2024-11-05'}}),flush=True)
 elif request['method']=='tools/call':
  assert request['params']=={'name':'index_status','arguments':{'project':'fresh-namespace'}}
  if mode=='timeout':time.sleep(30)
  if mode=='malformed':print('{broken',flush=True);break
  if mode=='eof':break
  sys.stderr.write('diagnostic'*10000);sys.stderr.flush()
  reply={'jsonrpc':'2.0','id':request['id'],'result':{'isError':True,'structuredContent':{'error':'project not found or not indexed'}}}
  if mode=='wrong_id':reply['id']=999
  print(json.dumps(reply),flush=True)
"""


class GraphSetupTests(unittest.TestCase):
    def test_stdio_handshake_checks_only_exact_namespace_and_closes_cleanly(self):
        result = missing_status([sys.executable, "-u", "-c", SERVER, "success"], "fresh-namespace")
        self.assertTrue(result["isError"])
        self.assertEqual(result["structuredContent"]["error"], "project not found or not indexed")

    def test_malformed_unpaired_eof_and_timeout_status_cannot_prove_missing(self):
        for mode in ["malformed", "wrong_id", "eof", "timeout"]:
            with self.subTest(mode=mode):
                started = time.monotonic()
                with self.assertRaises((ValueError, TimeoutError)):
                    missing_status([sys.executable, "-u", "-c", SERVER, mode], "fresh-namespace",
                                   timeout_seconds=0.15)
                self.assertLess(time.monotonic() - started, 3)

    def test_setup_records_uuid_missing_status_and_exact_instruction_hash(self):
        missing = {"isError": True, "structuredContent": {"error": "project not found or not indexed"}}
        with patch("graph_setup.missing_status", return_value=missing) as status:
            proof = prepare_namespace(Path("provider"), Path("/fresh/checkout"))
        self.assertRegex(proof["namespace"], r"^worker-codex-[0-9a-f]{32}$")
        self.assertTrue(proof["complete"])
        self.assertTrue(proof["missing_before_start"])
        self.assertEqual(status.call_args.args[1], proof["namespace"])
        self.assertEqual(proof["instructions_sha256"], hashlib.sha256(proof["instructions"].encode()).hexdigest())
        for response in [{"structuredContent": {"status": "indexed"}},
                         {"isError": True, "structuredContent": {"error": "provider unavailable"}},
                         {"isError": False, "structuredContent": missing["structuredContent"]}]:
            with patch("graph_setup.missing_status", return_value=response):
                self.assertFalse(prepare_namespace(Path("provider"), Path("/fresh"))["complete"])

    def test_developer_instructions_preserve_user_context_and_reject_ambiguous_profiles(self):
        self.assertEqual(developer_instructions({}), "")
        self.assertEqual(developer_instructions({"developer_instructions": "User context"}), "User context")
        for config in [{"developer_instructions": []}, {"profile": "custom"}]:
            with self.assertRaises(ValueError):developer_instructions(config)

    def test_codex_receives_namespace_in_developer_instructions_without_changing_task(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fake = root / "fake-codex"
            fake.write_text("#!" + sys.executable + "\nimport json\n"
                            "print(json.dumps({'type':'thread.started','thread_id':'toy'}))\n"
                            "print(json.dumps({'type':'turn.completed','usage':{}}))\n")
            fake.chmod(0o700)
            instruction = "Graph infrastructure: fresh-namespace"
            proof = {"instructions": instruction, "instructions_sha256": hashlib.sha256(instruction.encode()).hexdigest()}
            with patch.object(codex_run, "current_developer_instructions", return_value='User context: "keep"'):
                result = codex_run.run_codex(root, "Exact frozen task", root / "session",
                                            executable=str(fake), graph_namespace=proof, timeout_seconds=5)
            self.assertEqual(result["exit_code"], 0)
            command = json.loads((root / "session/command.json").read_text())
            configured = next(arg for arg in command if arg.startswith("developer_instructions="))
            combined = json.loads(configured.split("=", 1)[1])
            self.assertEqual(combined, 'User context: "keep"\n\n' + instruction)
            metadata = json.loads((root / "session/developer-instructions.json").read_text())
            self.assertEqual(metadata["text"], combined)
            self.assertEqual(metadata["sha256"], hashlib.sha256(combined.encode()).hexdigest())
            prompt = (root / "session/prompt.txt").read_text()
            self.assertTrue(prompt.startswith("Exact frozen task\n"))
            self.assertNotIn(instruction, prompt)


if __name__ == "__main__":
    unittest.main()
