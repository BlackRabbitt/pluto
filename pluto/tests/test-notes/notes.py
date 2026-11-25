"""A small notes CLI for exercising Pluto."""

import argparse
from contextlib import closing
import json
from pathlib import Path
import sqlite3


DATABASE = Path(__file__).resolve().with_name(".notes.sqlite3")


def nonempty(value):
    if not value.strip():
        raise argparse.ArgumentTypeError("must contain text")
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)

    add = commands.add_parser("add", help="Create a note")
    add.add_argument("title", type=nonempty)
    add.add_argument("body", type=nonempty)

    commands.add_parser("list", help="List note IDs and titles")

    show = commands.add_parser("show", help="Read a note by ID")
    show.add_argument("id", type=int)

    search = commands.add_parser("search", help="Find text in titles and bodies")
    search.add_argument("query", type=nonempty)

    args = parser.parse_args()
    try:
        with closing(sqlite3.connect(DATABASE)) as db, db:
            db.row_factory = sqlite3.Row
            db.create_function("casefold", 1, str.casefold)
            db.execute(
                "CREATE TABLE IF NOT EXISTS notes "
                "(id INTEGER PRIMARY KEY, title TEXT NOT NULL, body TEXT NOT NULL)"
            )
            result = run(db, args)
    except (sqlite3.Error, LookupError, OverflowError) as error:
        parser.exit(1, f"test-notes: {error}\n")

    print(json.dumps(result))


def run(db, args):
    if args.command == "add":
        inserted = db.execute(
            "INSERT INTO notes (title, body) VALUES (?, ?)",
            (args.title, args.body),
        )
        return {"id": inserted.lastrowid, "title": args.title}

    if args.command == "show":
        note = db.execute(
            "SELECT id, title, body FROM notes WHERE id = ?", (args.id,)
        ).fetchone()
        if note is None:
            raise LookupError(f"No note with ID {args.id}")
        return dict(note)

    if args.command == "search":
        notes = db.execute(
            "SELECT id, title FROM notes "
            "WHERE instr(casefold(title), ?1) OR instr(casefold(body), ?1) "
            "ORDER BY id DESC",
            (args.query.casefold(),),
        )
    else:
        notes = db.execute("SELECT id, title FROM notes ORDER BY id DESC")
    return [dict(note) for note in notes]


if __name__ == "__main__":
    main()
