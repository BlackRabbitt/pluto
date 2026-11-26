# Lightweight MCP tool discovery in Pluto

[Development guide](../DEVELOPMENT.md#code-map) · [Offline ranking pilot](tool-discovery-pilot.py)

## Research question

What is the smallest local search method that reliably finds the right MCP tool, recognizes ambiguity, and keeps tool definitions out of the model context until needed? Compare relevance, the cost of a search, and the number of extra model requests required to choose a tool.

The initial candidate is an exact-name check followed by a linear text scan. Compare a weighted word scorer with BM25 before adding a model or an index. The pilot below is exploratory. It gives us failure cases to investigate on a larger, separate set.

## What Pluto does today

- The chat turn connects to all configured MCP servers before its first model request. Connections run in parallel. A server has up to 12 seconds to connect and list tools. The slash picker can make its own discovery pass. This network work is independent of ranking cost. [Turn startup](../pluto/src/agent.rs#L163) · [Discovery timeout](../pluto/src/tools.rs#L617) · [VM picker request](../pluto/src/vm.rs#L224)
- Without a picker selection, Pluto exposes the catalog up front when it fits within three tools and 8 KiB of definitions. An explicit selection exposes the selected schemas. The model can search with `find_tools`. The current scorer counts exact words in the server and tool name, then counts matches in the description at a lower weight. It returns one positive match, with ties decided by catalog order. The winner's schema becomes available on the next model request. [Initial selection](../pluto/src/agent.rs#L171) · [Schema budget and search](../pluto/src/tools.rs#L899) · [Schema activation](../pluto/src/agent.rs#L564)
- The picker has stable server and tool names and filters their display labels. Its current response carries names and warnings. Ranking by description in the picker would require returning compact metadata. MCP already exposes tool names, optional titles, and descriptions through its paginated list operation. [Picker filter](../pluto-desktop/src/desktop/chat_ui.rs#L56) · [MCP tool listing](https://modelcontextprotocol.io/specification/2025-11-25/server/tools#listing-tools)

## Algorithms to compare

| Method | What it can improve | Cost to measure |
| --- | --- | --- |
| **Exact name and weighted words** | A full tool name wins. A server name resolves duplicates. A scan of pre-tokenized names, titles, and descriptions finds action queries with shared words. | Catalog scan, token storage, and ambiguity handling. No model or extra dependency is required. |
| **BM25 plus exact-name priority** | Common words carry less weight. Matches in unusually long descriptions count less than they would in simple word counts. Preserve a separate boost for tool names. | Token counts and document frequencies. A linear scan is enough for the initial comparison. [Lucene BM25](https://lucene.apache.org/core/9_9_1/core/org/apache/lucene/search/similarities/BM25Similarity.html) |
| **Character trigrams on names** | Shared three-character sequences can recover misspelled tool names. Try it as a fallback on short, name-like queries. | Extra token storage and false positives from similar names. |
| **Local embeddings with a flat scan** | A semantic score can recover paraphrases with little word overlap. Keep exact matches first and embed only names, titles, and short descriptions. | Encoder size, startup, warm query time, and memory. Cache tool vectors when metadata or the encoder version changes. |
| **Clustering or approximate vector search** | Groups vectors so a search can examine fewer candidates. | Index construction, updates, and lost matches when relevant tools fall outside the searched groups. Compare against exact vector search if the flat scan exceeds the latency budget. [Faiss index guidance](https://github.com/facebookresearch/faiss/wiki/Guidelines-to-choose-an-index) |

Start with the existing server grouping to honor explicit server constraints and display duplicate tool names. A learned cluster is a performance optimization to test after we have vectors and a measured scan problem. For scale, one thousand 384-dimensional float32 vectors take about 1.46 MiB for the raw values, plus the metadata and encoder. That is a size calculation. Search latency and peak memory still need measurement on Mac and Linux.

## First offline pilot

Run from the repository root with Python 3.10 or newer.

```sh
python3 research/tool-discovery-pilot.py
```

The script uses 19 fictional tools and 25 hand-written queries. It compares a reproduction of the current ranking rule, weighted words with exact-name priority, and BM25 with the same exact-name priority and a name boost. The two variants share a small common-word filter, so their false-positive counts measure the full variant. The current scorer's top-three column shows what its ranking would yield if it returned three results. The product returns one. [Pilot fixtures and scoring](tool-discovery-pilot.py)

| Ranker | Correct first, single target | Correct in top three, single target | All duplicates in top three | Negative queries with any candidates |
| --- | ---: | ---: | ---: | ---: |
| Pluto scoring rule | 15/19 | 17/19 | 2/2 | 3/4 |
| Exact names and weighted words | 13/19 | 17/19 | 2/2 | 1/4 |
| Exact names and BM25 with a name boost | 16/19 | 17/19 | 2/2 | 1/4 |

On this fixture, BM25 put one additional single target first compared with the baseline. Both variants returned fewer candidates for negative queries. The three rankers missed the same two paraphrases in their top three. The fixture is small, synthetic, and written alongside the rankers. It has no independent holdout or timing measurements. The pilot covers ranking only. It does not measure ambiguity decisions, model behavior, live discovery, embeddings, or clustering.

## Decision and next measurements

1. **Preserve exact identity.** An explicit server and tool name resolves directly. If an unqualified exact name exists on several servers, keep those candidates together. Rank action queries separately so a description cannot outrank an exact name.
2. **Spend model context on likely tools.** Return up to three metadata summaries for an ambiguous search. Expose one schema for a clear winner or after a narrower search. Keep the normal call approval path. Compare the extra lookup cost with exposing a small set of schemas immediately.
3. **Measure on held-out catalogs.** Include common tools, similar names with different actions, typos, paraphrases, unsupported actions, and multi-step requests that need more than one tool. Record top-one accuracy, top-three recall, ambiguity and false activation rates, search latency, context bytes, and model round trips. Use an independent sample from [ToolRet](https://arxiv.org/abs/2503.01763) as a scale check alongside representative Pluto catalogs. ToolRet reports that generic retrieval performance does not reliably predict tool retrieval performance.
4. **Pick the simplest method that meets the budget.** Compare the dependency-free scanners first. Compare character trigrams on typo cases and a local embedding model on paraphrases the text rankers miss. Benchmark cold and warm searches and the added binary, model, and memory footprint on both supported execution modes. Evaluate clustered search only if exact vector comparisons are the measured bottleneck.
5. **Measure connection startup separately.** Reuse per-instance catalog metadata when comparing ranking changes. Refresh changed metadata and validate against the live server when loading a schema or calling a tool. MCP supports tool-list change notifications from servers that advertise them. [MCP list changes](https://modelcontextprotocol.io/specification/2025-11-25/server/tools#list-changed-notification)
