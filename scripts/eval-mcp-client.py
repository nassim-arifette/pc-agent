"""Page complete local MCP replies for clients with small visible-output limits.

Only JSON presentation changes: nested JSON text is decoded once and every character is
delivered in bounded pages. The MCP server still captures the original replay replies.
"""
import argparse
from collections import deque
import json
from pathlib import Path
import subprocess
import sys

PAGE_CHARACTERS = 6000


def response_pages(response):
    response = json.loads(json.dumps(response, allow_nan=False))
    content = response.get("result", {}).get("content", [])
    if len(content) == 1 and content[0].get("type") == "text":
        try:
            decoded = json.loads(content[0]["text"])
            response["result"]["content"] = [{"type": "text", "decoded_json": decoded}]
        except (KeyError, ValueError):
            pass
    encoded = json.dumps(response, ensure_ascii=True, indent=1, allow_nan=False)
    # Concatenating all pages reconstructs the complete JSON presentation exactly.
    return [encoded[i:i + PAGE_CHARACTERS]
            for i in range(0, len(encoded), PAGE_CHARACTERS)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--scenario", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--allow-cloud", action="store_true")
    parser.add_argument("--allow-recording", action="store_true")
    parser.add_argument("--anonymize", action="store_true")
    args = parser.parse_args()
    if not args.allow_cloud or not args.allow_recording:
        parser.error("MCP replay requires explicit --allow-cloud and --allow-recording.")
    sys.stdin.reconfigure(encoding="utf-8")
    sys.stdout.reconfigure(encoding="utf-8", line_buffering=True)
    command = [args.binary, "mcp", "--scenario", args.scenario, "--allow-cloud",
               "--allow-recording", "--out", args.out]
    if args.anonymize:
        command.append("--anonymize")
    child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True, encoding="utf-8", bufsize=1)
    pages = deque()
    responses = 0
    served = 0
    finish_requested = False
    failed = False

    def show_page():
        nonlocal served
        if not pages:
            print("No pending MCP response pages.", flush=True)
            return
        number, total, payload = pages.popleft()
        served += 1
        print(f"MCP RESPONSE PAGE {number}/{total} ({len(pages)} remaining)", flush=True)
        print(payload, flush=True)

    try:
        for line in sys.stdin:
            message = json.loads(line)
            if message == {"bridge_next": True}:
                show_page()
                continue
            if message == {"bridge_finish": True}:
                if pages:
                    print("Read pending pages before finishing.", flush=True)
                    continue
                finish_requested = True
                break
            if pages:
                print("Read all pending pages before sending another RPC.", flush=True)
                continue
            child.stdin.write(json.dumps(message, ensure_ascii=True) + "\n")
            child.stdin.flush()
            if "id" in message:
                raw = child.stdout.readline()
                if not raw:
                    raise RuntimeError("MCP closed without a reply")
                chunks = response_pages(json.loads(raw))
                responses += 1
                pages.extend((i + 1, len(chunks), chunk) for i, chunk in enumerate(chunks))
                show_page()
            else:
                print("MCP notification forwarded.", flush=True)
    except Exception:
        failed = True
        raise
    finally:
        child.stdin.close()
        try:
            result = child.wait(timeout=15)
        except subprocess.TimeoutExpired:
            child.terminate()
            result = child.wait(timeout=5)
        error = child.stderr.read()
        if error:
            print(error[:4096], file=sys.stderr)
        visibility = {
            "schema_version": 1, "responses": responses, "pages_served": served,
            "pending_pages": len(pages), "finish_requested": finish_requested,
            "child_exit_code": result,
            "client_visibility_complete": not failed and finish_requested and not pages and result == 0,
        }
        Path(args.out + ".client.json").write_text(
            json.dumps(visibility, indent=2) + "\n", encoding="utf-8")
        print(f"MCP CLOSED exit_code={result}", flush=True)
    return result


if __name__ == "__main__":
    sys.exit(main())
