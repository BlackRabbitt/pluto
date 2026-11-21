---
name: test-notes
description: Create, list, search, and read sample notes in the Pluto test notes database. Use for requests about test notes.
---

Run the CLI with the terminal tool from the Pluto project root. Set `command` to `python3` and pass the script path, subcommand, and each value as separate entries in `args`.

- Create a note with `["tests/test-notes/notes.py", "add", "--", TITLE, BODY]`.
- List note IDs and titles with `["tests/test-notes/notes.py", "list"]`.
- Search titles and bodies with `["tests/test-notes/notes.py", "search", "--", QUERY]`.
- Read a note with `["tests/test-notes/notes.py", "show", ID]`. Pass the ID as a string.

On success, the terminal tool's `stdout` contains JSON. Use the returned ID to read a note. If the command fails, report its `stderr`.
