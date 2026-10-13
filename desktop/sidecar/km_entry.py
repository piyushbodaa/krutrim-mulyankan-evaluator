"""PyInstaller entry for the single-file `km` command.

Build this as a console program and add the `suites/` directory as data.
A windowed build is wrong here: `km` is a command, not a window.
"""

from kritrim_mulyankan.cli import app

if __name__ == "__main__":
    app()
