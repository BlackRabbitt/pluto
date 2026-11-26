---
name: test-sort
description: Sort the lines of a text file and write sort-report.txt in the terminal working directory.
---

# Sort a file into a report

Use this skill when asked to sort a plain-text file. Sort whole lines in ascending byte order, keeping duplicates and blank lines. Ask for an input filename if the request does not identify one.

Use the `terminal` tool with the following command and arguments. Replace `./items.txt` with the input file’s path relative to the terminal working directory. Keep the `./` prefix and pass the path as its own argument.

```json
{
  "command": "/bin/sh",
  "args": [
    "-c",
    "if [ ! -f \"$1\" ] || [ ! -r \"$1\" ]; then printf 'Input file is missing or unreadable\\n' >&2; exit 1; fi; set -C; LC_ALL=C sort \"$1\" > ./sort-report.txt",
    "sort-report",
    "./items.txt"
  ]
}
```

Report success only if the terminal tool returns exit code `0`. Tell the user that `sort-report.txt` was written in the current working directory. In Pluto VM, the default directory is `/var/lib/pluto-workspace`. If the report already exists, leave it in place and explain the error.
