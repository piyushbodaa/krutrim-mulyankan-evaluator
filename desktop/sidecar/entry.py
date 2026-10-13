"""PyInstaller entry. A console subsystem so the shell can speak JSON on stdin/stdout.

The desktop window spawns this with no console window. Do not build it as a windowed app:
that would drop the pipes the shell uses.
"""

from kritrim_mulyankan.desktop_bridge import main

if __name__ == "__main__":
    main()
