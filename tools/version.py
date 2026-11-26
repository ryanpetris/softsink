"""Read the build version."""
import os
import re
import sys


def version():
    value = os.environ.get("SOFTSINK_VERSION", "0.0.0")
    if not re.fullmatch(r"(0|[1-9][0-9]*)(\.(0|[1-9][0-9]*)){2}", value):
        sys.exit("SOFTSINK_VERSION must be MAJOR.MINOR.PATCH without a v prefix")
    return value


if __name__ == "__main__":
    print(version())
