"""Offline, synthetic comparison of MCP tool rankers. Run with Python 3."""

import json
import math
import re
from collections import Counter


# Preserve server order and server-local discovery order for Pluto's alias tie-break.
CATALOG = [
    ("calendar", "list_events", "List calendar appointments over a date range."),
    ("calendar", "find_availability", "Check attendee availability and free time."),
    ("calendar", "create_event", "Schedule a meeting and invite attendees."),
    ("calendar", "cancel_event", "Cancel a calendar appointment."),
    ("chat", "search_messages", "Find messages and threads in chat channels."),
    ("chat", "post_message", "Send a message to a team channel."),
    ("files", "search_files", "Find documents by filename or text."),
    ("files", "list_files", "List the contents of a folder."),
    ("files", "read_file", "Read the contents of a file."),
    ("finance", "list_events", "List incoming payment events and refunds."),
    ("finance", "lookup", "Look up a customer invoice by number."),
    ("finance", "list_invoices", "List customer invoices and payment status."),
    ("issues", "lookup", "Fetch a ticket by its ID."),
    ("issues", "search_issues", "Find work tickets by status, title, or assignee."),
    ("issues", "create_issue", "File a bug report or create a work ticket."),
    ("notes", "lookup", "Find a saved note by title."),
    ("notes", "write_note", "Save a note or jot down text."),
    ("weather", "get_forecast", "Fetch a weather forecast and expected rainfall."),
    ("weather", "get_temperature", "Read the current temperature."),
]

# Each expected tuple lists all relevant tools. An empty tuple means abstain.
QUERIES = [
    ("exact", "search_files", None, ("files/search_files",)),
    ("exact", "cancel_event", None, ("calendar/cancel_event",)),
    ("exact", "list_events", "finance", ("finance/list_events",)),
    ("exact", "READ-FILE", None, ("files/read_file",)),
    ("qualified", "calendar/list_events", None, ("calendar/list_events",)),
    ("qualified", "notes/lookup", None, ("notes/lookup",)),
    ("duplicate", "list_events", None, ("calendar/list_events", "finance/list_events")),
    ("duplicate", "lookup", None, ("finance/lookup", "issues/lookup", "notes/lookup")),
    ("description", "find a ticket by ID", None, ("issues/lookup",)),
    ("description", "jot down text", None, ("notes/write_note",)),
    ("description", "show expected rainfall", None, ("weather/get_forecast",)),
    ("description", "fetch an invoice by number", None, ("finance/lookup",)),
    ("description", "list the contents of a folder", None, ("files/list_files",)),
    ("description", "messages and threads", None, ("chat/search_messages",)),
    ("description", "find open tickets by assignee", None, ("issues/search_issues",)),
    ("description", "file a bug report", None, ("issues/create_issue",)),
    ("paraphrase", "who is free tomorrow", None, ("calendar/find_availability",)),
    ("paraphrase", "when can everyone get together", None, ("calendar/find_availability",)),
    ("paraphrase", "will I need an umbrella", None, ("weather/get_forecast",)),
    ("typo", "search_mesages", None, ("chat/search_messages",)),
    ("typo", "get_temprature", None, ("weather/get_temperature",)),
    ("none", "list train departures and delays", None, ()),
    ("none", "download a podcast", None, ()),
    ("none", "weather forecast", "notes", ()),
    ("none", "which of the trains is late", None, ()),
]

COMMON = set("a an and are by can for i is it me of the to what when which who with".split())


def words(text):
    return re.findall(r"[^\W_]+", text.lower())


def meaningful(text):
    return set(words(text)) - COMMON


DOCUMENTS = [
    Counter(word for word in words(f"{name} {description}") if word not in COMMON)
    for _, name, description in CATALOG
]
DOCUMENT_FREQUENCIES = Counter(word for doc in DOCUMENTS for word in doc)
MEAN_LENGTH = sum(map(Counter.total, DOCUMENTS)) / len(DOCUMENTS)


def identity(tool):
    server, name, _ = tool
    return f"{server}/{name}"


def baseline(query, server):
    terms = set(words(query))
    scores = []
    for index, tool in enumerate(CATALOG):
        source, name, description = tool
        if server and server != source:
            continue
        target = set(words(f"{source} / {name}"))
        remaining = set(words(description)) - target
        score = 3 * len(terms & target) + len(terms & remaining)
        if score:
            scores.append((-score, f"mcp_{index}_", identity(tool)))
    return [key for _, _, key in sorted(scores)]


def exact_matches(query, tools):
    prefix, slash, name = query.partition("/")
    if slash:
        return [
            identity(tool)
            for tool in tools
            if prefix.strip().lower() == tool[0] and words(name) == words(tool[1])
        ]
    return [identity(tool) for tool in tools if words(query) == words(tool[1])]


def bm25_score(terms, counts):
    score = 0.0
    normalization = 1.2 * (0.25 + 0.75 * counts.total() / MEAN_LENGTH)
    for word in sorted(terms & counts.keys()):
        frequency = counts[word]
        document_frequency = DOCUMENT_FREQUENCIES[word]
        inverse = math.log1p(
            (len(CATALOG) - document_frequency + 0.5) / (document_frequency + 0.5)
        )
        score += inverse * (frequency * 2.2) / (frequency + normalization)
    return score


def ranked(query, server, use_bm25):
    terms = meaningful(query)
    tools = [tool for tool in CATALOG if not server or tool[0] == server]
    exact = exact_matches(query, tools)
    if exact or "/" in query:
        return exact
    scores = []
    for tool, doc in zip(CATALOG, DOCUMENTS):
        if tool not in tools:
            continue
        _, name, description = tool
        names = meaningful(name)
        score = 3 * len(terms & names)
        if use_bm25:
            score += bm25_score(terms, doc)
        else:
            score += len(terms & (meaningful(description) - names))
        if score:
            scores.append((-score, identity(tool)))
    return [key for _, key in sorted(scores)]


def evaluate(rank):
    totals = Counter()
    misses = []
    for label, query, server, expected in QUERIES:
        results = rank(query, server)
        if len(expected) == 1:
            totals["single_total"] += 1
            totals["single_top1"] += results[:1] == list(expected)
            totals["single_top3"] += expected[0] in results[:3]
            if expected[0] not in results[:3]:
                misses.append({"query": query, "kind": label, "candidates": results[:3]})
        elif expected:
            totals["duplicate_total"] += 1
            totals["duplicate_all_in_top3"] += set(expected).issubset(results[:3])
        else:
            totals["none_total"] += 1
            totals["none_returned_candidates"] += bool(results)
    return {**totals, "single_top3_misses": misses}


if __name__ == "__main__":
    print(json.dumps({
        "tools": len(CATALOG),
        "queries": len(QUERIES),
        "pluto_baseline": evaluate(baseline),
        "weighted_words": evaluate(lambda query, server: ranked(query, server, False)),
        "bm25_plus_name_boost": evaluate(lambda query, server: ranked(query, server, True)),
    }, indent=2))
