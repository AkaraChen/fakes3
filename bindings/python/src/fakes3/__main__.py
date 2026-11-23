"""``fakes3`` on the command line: runs the bundled binary with the given arguments."""

import os
import subprocess
import sys

from ._server import binary_path


def main() -> None:
    binary = binary_path()
    argv = [binary, *sys.argv[1:]]
    if os.name == "nt":
        try:
            sys.exit(subprocess.call(argv))
        except KeyboardInterrupt:
            sys.exit(130)
    os.execv(binary, argv)


if __name__ == "__main__":
    main()
