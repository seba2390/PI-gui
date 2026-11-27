# Web search

For chats with web search on: the globe next to the paperclip in the message box, set per chat
(`/websearch off|search|deep` and `/websearch effort light|standard|thorough|max` work too).
Changes apply from the next message, without a restart.

## The two modes

Search
: A quick lookup: a few searches and pages, then a concise answer with links.

Deep search
: A researched, sourced report. The model plans sub-questions, searches from several angles,
  reads many pages (PDFs too) and cross-checks key facts. If it tries to report before the
  effort level's minimum, or leaves a sub-question open, it is sent back to its research.

  Before the report is shown, it is checked: links are tried, quotes are looked up on their
  pages, and (from Standard on) the model judges, from the sources' own text, whether they
  back the claims. The report is then corrected with what the check found. While this runs,
  the chat shows the searches, pages, plan, checks and drafts. If the check fails or you press
  Esc, the latest complete draft is the answer, marked as not checked.

The model searches with `web_search` and reads pages with `web_fetch`. Pistachio does the work
itself: pages are read without JavaScript, reduced to their main text, and cut down to the
passages that match what the model is looking for. Only public addresses are fetched, unless
you allow local ones below.

## Search service

Service
: **DuckDuckGo** (no key, the default), **SearXNG** (no key), **Brave Search** or **Tavily**
  (API key). Keys are stored only on this computer, readable by you alone.

SearXNG
: Choosing SearXNG checks whether one answers at the address. Use your own instance (its
  `settings.yml` must allow the `json` format), or let Pistachio run one on this computer
  (macOS and Linux, no Docker): it asks first, then downloads SearXNG with a private Python
  (about 80 MB, 190 MB on disk), each part pinned and checked against its checksum. Only this
  computer can reach it. It starts with the first chat that searches and stops when the app
  quits. Its row has a status light and a **⋮** menu to start or stop it, update it, change the
  address or remove it.

## New chats

Web search
: Whether new chats start with web search **Off**, **Search** or **Deep search**.

Deep search effort
: The effort new deep searches start with:

  | Level | What it does |
  |---|---|
  | Light | A quick pass over a few sources. Checks that links work. |
  | Standard | Answers every part of the question. Checks links and the key claims. |
  | Thorough | Keeps researching until every part is backed. Checks links and every claim. |
  | Max | Also looks for contrary evidence. Checks links and every claim, one by one. |

## Advanced search options

What a quick search may do: **Searches** and **Pages read** per message, and **Text per page**
(characters, about four to a token; lower it for models with a small context window).

## Advanced deep search settings

How much work each effort level does. Pick a level, then set:

At most
: **Searches**, **Pages read** and **Results per search**, per message.

At least
: **Sub-questions** (a broad question is split into at most this many), **Searches** and
  **Pages per sub-question**, **Different sites**, and **Sent back** (how often the model is sent
  back to its research before its report is taken as it is).

Checking the report
: **Claims checked** against their sources (0 checks only links), **Claims per question** (how
  many are judged at once), and **Revisions** (how often the report is rewritten after a
  check).

For every level
: **Time per question**, **Time per check** and **Answer length** for the checks.

## Network

Allow local network addresses
: Off by default. When on, pages the model asks for can reach devices on your network
  (localhost, your LAN).

## Files

The settings live in `~/.pi/agent/pistachio/deepsearch/`:

- `config.json`: everything on this page.
- `prompt-search.md` and `prompt-deep.md`: each mode's instructions to the model. Delete a file
  to get the default back.

To see what the model would get without the app:

```bash
pistachio --deepsearch-tool search <query>
pistachio --deepsearch-tool fetch <url> [what to look for]
```
