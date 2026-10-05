"""Check that pagination preserves every measurement and character without invoking cloud APIs."""
import importlib.util
import json
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("eval_mcp_client", Path(__file__).with_name("eval-mcp-client.py"))
client = importlib.util.module_from_spec(spec)
spec.loader.exec_module(client)


class PaginationTests(unittest.TestCase):
    def test_large_nested_json_is_complete_and_input_stays_unchanged(self):
        payload = {"observations": [{"entity_ref": f"battery-{i}", "value": -i * 1000,
                                     "unit": "milliwatts", "source": "fixture/électricité",
                                     "window": {"duration_ms": i + 1}}
                                    for i in range(4096)]}
        response = {"jsonrpc": "2.0", "id": 3, "result": {"isError": False, "content": [
            {"type": "text", "text": json.dumps(payload)}]}}
        original = json.dumps(response)
        pages = client.response_pages(response)
        self.assertGreater(len(pages), 10)
        self.assertTrue(all(len(page) <= client.PAGE_CHARACTERS for page in pages))
        restored = json.loads("".join(pages))
        self.assertEqual(restored["result"]["content"][0]["decoded_json"], payload)
        self.assertEqual(restored["jsonrpc"], "2.0")
        self.assertEqual(restored["id"], 3)
        self.assertFalse(restored["result"]["isError"])
        self.assertEqual(json.dumps(response), original)

    def test_long_strings_unicode_and_non_json_tool_text_are_preserved(self):
        for text in ["測定 −15 W\n" * 4000, '{"not valid JSON', "[]"]:
            response = {"id": 5, "result": {"content": [{"type": "text", "text": text}]}}
            restored = json.loads("".join(client.response_pages(response)))
            content = restored["result"]["content"][0]
            if text == "[]":
                self.assertEqual(content["decoded_json"], [])
            else:
                self.assertEqual(content["text"], text)

    def test_rpc_errors_and_definitions_round_trip_without_schema_changes(self):
        for response in [
            {"id": 1, "error": {"code": -32602, "message": "unknown argument"}},
            {"id": 2, "result": {"tools": [{"name": "record_activity", "inputSchema": {
                "type": "object", "additionalProperties": False, "required": ["duration_seconds"]}}]}},
        ]:
            self.assertEqual(json.loads("".join(client.response_pages(response))), response)


if __name__ == "__main__":
    unittest.main()
