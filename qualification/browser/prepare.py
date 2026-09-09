#!/usr/bin/env python3
"""Build isolated WASM probes and disposable Linux-created filesystem fixtures."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
WORK = ROOT / ".work/webapp-feasibility"
EXT4_REV = "3a95a96dd70a9d86ed0827ed2daa4bbf22b7a617"


def run(*args, **kw):
    subprocess.run(args, check=True, **kw)


def source(url, target, expected_sha256):
    # Downloads are build inputs; nothing from the phone is used by this lab.
    archive = WORK / (target.name + ".tar.gz")
    if not archive.exists():
        urllib.request.urlretrieve(url, archive)
    raw = archive.read_bytes()
    actual_sha256 = hashlib.sha256(raw).hexdigest()
    if actual_sha256 != expected_sha256:
        raise ValueError(f"source checksum differs: {archive}")
    if target.exists():
        shutil.rmtree(target)
    target.mkdir(parents=True)
    with tarfile.open(fileobj=io.BytesIO(raw)) as tar:
        for member in tar.getmembers():
            parts = Path(member.name).parts[1:]
            if not parts:
                continue
            if member.issym() or member.islnk():
                continue  # upstream agent-tool links are not build inputs
            if ".." in parts:
                raise ValueError("unexpected archive entry")
            dest = target.joinpath(*parts)
            if member.isdir():
                dest.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.write_bytes(tar.extractfile(member).read())
    return {"url": url, "sha256": actual_sha256}


def replace(path, old, new):
    text = path.read_text()
    if text.count(old) != 1:
        raise ValueError(f"patch no longer matches: {path}")
    path.write_text(text.replace(old, new))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--wasm-bindgen", required=True)
    args = parser.parse_args()
    WORK.mkdir(parents=True, exist_ok=True)
    deps = WORK / "deps"
    core = deps / "rust-fs-core"
    ext4 = deps / "rust-fs-ext4"
    inputs = {
        "ext4": source("https://codeload.github.com/1vivy/rust-fs-ext4/tar.gz/29241bc6767322a203999536096bacc14eb74c1a", ext4, "5a85845b9f7de18cc3b3d8f2bedcef58bf7d3c7afe26efa65d596e9d4b29cd55"),
        "core": source("https://codeload.github.com/1vivy/rust-fs-core/tar.gz/e6e891a0828fcd80d1c77f52b9ce1f447b8d1a1e", core, "950d42143aa74b178aee03f966a1a4aebcdbed689553c149bbebcefdbd33b495"),
    }
    wasm = WORK / "wasm"
    shutil.copytree(HERE / "wasm", wasm, dirs_exist_ok=True)
    env = dict(os.environ, RUSTFLAGS="--cfg=web_sys_unstable_apis", CARGO_TARGET_DIR=str(WORK / "target"))
    run("cargo", "build", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path", str(wasm / "Cargo.toml"), env=env)
    public = WORK / "public"
    public.mkdir(exist_ok=True)
    run(args.wasm_bindgen, "--target", "web", "--out-dir", str(public / "pkg"), str(WORK / "target/wasm32-unknown-unknown/release/canoe_browser_probe.wasm"))
    for name in ["worker.js", "index.html"]:
        shutil.copyfile(HERE / name, public / name)
    with (public / "ext4.img").open("wb") as out:
        out.truncate(128 * 1024 * 1024)
    run("mkfs.ext4", "-q", "-F", "-b", "4096", "-E", "lazy_itable_init=0,lazy_journal_init=0", str(public / "ext4.img"))
    with (public / "fat.img").open("wb") as out:
        out.truncate(32 * 1024 * 1024)
    run("mkfs.fat", "-F", "16", str(public / "fat.img"))
    (public / "payload.bin").write_bytes(bytes(range(256)) * 8192)
    inputs["native_clock"] = False
    inputs["native_pid"] = False
    inputs["wasm_sha256"] = hashlib.sha256((public / "pkg/canoe_browser_probe_bg.wasm").read_bytes()).hexdigest()
    inputs["fixtures"] = {name: hashlib.sha256((public / name).read_bytes()).hexdigest()
                          for name in ["fat.img", "ext4.img", "payload.bin"]}
    (WORK / "build.json").write_text(json.dumps(inputs, indent=2) + "\n")
    print(WORK)


if __name__ == "__main__":
    main()
