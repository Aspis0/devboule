from __future__ import annotations

import sys


def main() -> int:
    print(
        "OpenRouter smoke was retired from Devboule. Use model_bakeoff.py with --remote-provider scaleway|infomaniak|mistral.",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
