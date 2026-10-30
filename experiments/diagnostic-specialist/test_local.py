"""Protocol tests use a fake runtime, never download weights or call a provider."""
from contextlib import contextmanager, closing
import http.client
import json
import threading
import unittest

from local import LocalServer, check_state, MAX_BODY


def state():
    return {"symptom": "my PC is slow", "symptom_active_now": None,
            "observations": [], "completed_calls": [], "data_origin": "synthetic test"}


class FakeRuntime:
    identity = {"protocol": 1, "model": "test fake; not the published model"}
    def __init__(self):
        self.calls = []
    def decide(self, value, variant):
        self.calls.append((value, variant))
        return {"identity": self.identity, "variant": variant, "decision": None, "raw": "invalid output is not repaired"}


@contextmanager
def serving():
    runtime = FakeRuntime()
    with LocalServer(runtime, 0) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield server
        finally:
            server.shutdown()
            thread.join()


class LocalTests(unittest.TestCase):
    def request(self, server, path="/health", body=None, headers=None, method=None):
        with closing(http.client.HTTPConnection("127.0.0.1", server.server_port, timeout=2)) as connection:
            headers = headers or {}
            if body is not None:
                body = json.dumps(body)
                headers.setdefault("Content-Type", "application/json")
            connection.request(method or ("POST" if body is not None else "GET"), path, body, headers)
            response = connection.getresponse()
            return response.status, json.loads(response.read())

    def test_health_never_invokes_inference(self):
        with serving() as server:
            code, result = self.request(server)
            self.assertEqual(code, 200)
            self.assertEqual(result, server.runtime.identity)
            self.assertEqual(server.runtime.calls, [])

    def test_browser_cross_origin_and_rebinding_are_rejected(self):
        with serving() as server:
            for headers in [{"Host": "attacker.example"}, {"Origin": "http://localhost"}, {"Sec-Fetch-Site": "same-origin"}]:
                self.assertEqual(self.request(server, headers=headers)[0], 403)
                self.assertEqual(self.request(server, "/decide", {"state": state()}, headers)[0], 403)
            self.assertEqual(server.runtime.calls, [])

    def test_fake_runtime_is_called_only_with_valid_states_and_variant(self):
        with serving() as server:
            code, result = self.request(server, "/decide", {"state": state(), "variant": "base"})
            self.assertEqual(code, 200)
            self.assertIsNone(result["decision"])
            self.assertEqual(server.runtime.calls, [(state(), "base")])
            for request in [{"state": {}}, {"state": state(), "variant": "cloud"}, {"state": state(), "execute": True}]:
                self.assertEqual(self.request(server, "/decide", request)[0], 400)

    def test_no_queue_and_body_limit(self):
        with serving() as server:
            with server.inference_lock:
                self.assertEqual(self.request(server, "/decide", {"state": state()})[0], 409)
            self.assertEqual(self.request(server, "/decide", {"state": state()}, {"Content-Length": str(MAX_BODY + 1)})[0], 413)
            self.assertEqual(server.runtime.calls, [])

    def test_state_does_not_invent_unknown_timing_or_citations(self):
        check_state(state())
        bad = state()
        bad["symptom_active_now"] = "yes"
        with self.assertRaises(ValueError):
            check_state(bad)
        bad = state()
        bad["observations"] = [{"id": "O1"}, {"id": "O1"}]
        with self.assertRaises(ValueError):
            check_state(bad)


if __name__ == "__main__":
    unittest.main()
