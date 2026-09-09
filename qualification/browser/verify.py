#!/usr/bin/env python3
"""Independent read-only filesystem check and extraction of browser results."""
import hashlib
import json
from pathlib import Path
import subprocess

work = Path(__file__).resolve().parents[2] / ".work/webapp-feasibility"
results = work / "results"
checks = []
browser = json.loads((results / "browser.json").read_text())
build = json.loads((work / "build.json").read_text())
if not browser["matchedExpectations"] or browser["build"] != build or build["native_clock"] or build["native_pid"]:
    raise RuntimeError("run the successful browser probes for the current build before verification")


def run(*command):
    p = subprocess.run(command, text=True, capture_output=True)
    checks.append({"command": command, "exit": p.returncode, "output": p.stdout + p.stderr})
    if p.returncode:
        raise RuntimeError(p.stdout + p.stderr)


run("e2fsck", "-fn", str(results / "ext4.img"))
run("fsck.fat", "-n", str(results / "fat.img"))
for name in ["extracted.fat", "extracted.bin"]:
    (results / name).unlink(missing_ok=True)
run("debugfs", "-R", f'dump /efisp.fat "{results}/extracted.fat"', str(results / "ext4.img"))
run("mcopy", "-i", str(results / "fat.img"), "::browser-result.bin", str(results / "extracted.bin"))
for result, source in [("extracted.fat", "fat.img"), ("extracted.bin", "payload.bin")]:
    actual = (results / result).read_bytes()
    expected = (work / "public" / source).read_bytes()
    if actual != expected:
        raise RuntimeError(f"independent extraction differs: {result}")
    checks.append({"result": result, "bytes": len(actual), "sha256": hashlib.sha256(actual).hexdigest(), "matches": True})
(results / "oracle.json").write_text(json.dumps(checks, indent=2) + "\n")
print("Both filesystems pass their native checkers; both extracted payloads match exactly.")
