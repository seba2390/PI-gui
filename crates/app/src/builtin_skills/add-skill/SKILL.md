---
name: add-skill
description: Create a new pi skill, or improve an existing one, so pi can load it in later sessions. Use when the user asks to add, make, write or save a skill, to turn a workflow, checklist or set of instructions into a skill, or to fix a skill that pi does not load or does not pick up when it should.
---

# Adding a skill

A skill is a folder holding a `SKILL.md`: front matter that tells pi what the skill is for,
then the instructions pi reads when a task calls for them. pi lists every skill's name and
description in its system prompt and reads the rest only when it decides the skill applies,
so the description does the routing and the body does the work.

## 1. Settle what the skill is for

Before writing anything, be able to say in one sentence what the skill makes pi do and when.
Ask the user only what you cannot work out from the conversation or the project:

- The task it covers, and a few requests that should trigger it (and some that should not).
- Whether pi should load it by itself, or only when the user types `/skill:<name>`.
- Whether it belongs to this project only or to every project.

If the user just walked through the workflow with you, write the skill from that: the steps
that worked, the commands, the gotchas, the corrections the user made.

## 2. Pick the place

| Where | Folder | Use for |
|---|---|---|
| Personal | `<agent dir>/skills/<name>/` | Skills for every project. The agent dir is `~/.pi/agent` unless `PI_CODING_AGENT_DIR` points elsewhere (on an SSH host that Pistachio installed pi on, it is `~/.pistachio/agent`). |
| Personal, shared with other tools | `~/.agents/skills/<name>/` | Agent Skills that other agents read too. |
| Project | `.pi/skills/<name>/` in the project | Skills about this codebase. pi loads them only once the user trusts the folder (`/trust`, or the Skills page in Pistachio). |
| Project, shared with other tools | `.agents/skills/<name>/` | Found from the working folder up to the repository root. |

Check that no skill with the same name exists already (list the folders above): when two
share a name pi keeps the first one it finds and warns. Improving an existing skill means
editing its `SKILL.md` in place.

Do not write into `<agent dir>/pistachio/skills/`: Pistachio rewrites its built-in skills
there every time it starts a session.

## 3. Write SKILL.md

```markdown
---
name: pdf-tools
description: Extract text and tables from PDF files and fill PDF forms. Use when the user wants to read, convert, split or fill a PDF.
---

# PDF tools

Instructions pi follows once it has loaded the skill.
```

Front matter rules (pi skips a skill whose front matter is broken or has no description):

- `name`: lowercase letters, digits and hyphens; no hyphen at either end and no double
  hyphens; at most 64 characters. Make it the folder's name.
- `description`: at most 1024 characters, on one line. Say what the skill does **and** when
  to use it, with the words a user would actually type ("PDF", "fill a form", "invoice").
  "Helps with PDFs" is too vague for pi to route on. Quote the value with double quotes if
  it contains `: ` or ` #`, or starts with a character such as `-`, `[`, `{`, `*` or `>`.
- `disable-model-invocation: true` (optional): pi never loads it on its own; the user calls
  it with `/skill:<name>`. Use it for skills with side effects the user should start
  deliberately, such as a release or a deploy.
- `license`, `compatibility`, `metadata` and `allowed-tools` are optional Agent Skills fields.

The body:

- Write instructions, not a description: imperative steps pi can follow, in the order it
  should follow them.
- Put in what pi would not know on its own: project commands, paths, conventions, the
  mistakes to avoid and why. Leave out what any competent agent already does.
- Explain the reason behind a rule when it is not obvious; pi handles cases the rule did not
  foresee better when it knows the purpose.
- Keep `SKILL.md` short enough to read in one go (a few hundred lines at most). Move long
  reference material into separate files and say when to read each one.
- Include a short example of the output when its format matters.

## 4. Add supporting files (optional)

```text
pdf-tools/
├── SKILL.md
├── scripts/      code pi runs instead of writing it again each time
├── references/   longer docs pi reads when SKILL.md says to
└── assets/       templates and other files the output uses
```

Refer to them with paths relative to the skill folder (`scripts/extract.sh`); pi is told
where the skill lives. Make scripts executable and say in `SKILL.md` how to call them and
what they print.

## 5. Check that pi loads it

pi reads skills when it starts.

- In pi in a terminal, run `/reload` (or start pi again), then look for the skill in the
  startup notices and run `/skill:<name>` to load it by hand.
- In Pistachio, saving a skill on the Skills page restarts pi on the same session. A skill
  written some other way (by you, for instance) loads in the next session the user starts;
  to load it in the current one, the user can open it on the Skills page and save it.
- For a project skill that does not show up: the folder is probably not trusted yet.

Then try two or three requests that should trigger the skill and one that should not, and
adjust the description if pi picks it up too rarely or too often.

## 6. Tell the user

Say where the skill was written, what its description says, how to call it
(`/skill:<name>`), and anything that is still unverified.
