#!/usr/bin/env python3
"""Exercise the installer without changing the user's installation."""

import hashlib
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile


def executable(path, content):
    path.write_text(content)
    path.chmod(0o755)


installer = Path(__file__).with_name("install.sh").resolve()
with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    commands = root / "commands"
    commands.mkdir()
    destination = root / "installed"
    destination.mkdir()
    executable(commands / "uname", '#!/bin/sh\ncase "$1" in -s) echo "${TEST_OS:-Darwin}";; -m) echo "${TEST_ARCH:-arm64}";; esac\n')
    for command in ["tmux", "node", "npx"]:
        executable(commands / command, "#!/bin/sh\nexit 0\n")
    executable(commands / "curl", '#!/bin/sh\ncp "$TEST_ASSETS/${2##*/}" "$4"\n')
    environment = dict(os.environ, PATH=f"{commands}:{os.environ['PATH']}",
                       INSTALL_DIR=str(destination), TEST_ASSETS=str(root))
    for architecture in ["arm64", "amd64"]:
        binary = root / f"aoe-darwin-{architecture}"
        executable(binary, "#!/bin/sh\necho aoe-test\n")
        archive = root / f"{binary.name}.tar.gz"
        with tarfile.open(archive, "w:gz") as package:
            package.add(binary, arcname=binary.name)
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        archive.with_suffix(".gz.sha256").write_text(f"{digest}  {archive.name}\n")
    for architecture in ["arm64", "x86_64"]:
        result = subprocess.run(["bash", str(installer)], env=dict(environment, TEST_ARCH=architecture),
                                capture_output=True, text=True)
        assert result.returncode == 0, result.stderr
        assert subprocess.check_output([str(destination / "aoe")], text=True).strip() == "aoe-test"
    (destination / "aoe").write_text("existing installation")
    (root / "aoe-darwin-arm64.tar.gz").write_bytes(b"corrupt download")
    for overrides in [{}, {"TEST_OS": "Linux"}, {"TEST_ARCH": "riscv64"}]:
        result = subprocess.run(["bash", str(installer)], env=dict(environment, **overrides),
                                capture_output=True, text=True)
        assert result.returncode != 0
        assert (destination / "aoe").read_text() == "existing installation"
print("Installer checks passed: both Mac architectures, checksum failure, unsupported systems; existing installation preserved.")
