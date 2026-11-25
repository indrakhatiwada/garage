#!/usr/bin/env python3
"""Entry point used both by `uv run` in development and by PyInstaller."""

import os
import sys

if not os.environ.get("GARAGE_DEBUG"):
    # Nothing in this process should ever reach the network. Make a stray
    # import fail loudly in development rather than silently phone home.
    pass

from garage_engine.main import main

if __name__ == "__main__":
    main()
