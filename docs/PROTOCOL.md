# The burn protocol

A hop is a program cig did not write and cannot see into. Hop watching tells
what changed in the pack between the scan before and the scan after, and
`touches` says which files to keep beforehand; that is the kernel working
out a hop's write set from the outside. A tool that *knows* what it changed
(a migration runner, a deployer, a code generator, a package manager) can
say so instead, and say how to undo it. This page is the whole contract.

## The channel

Every hop a real run starts gets one environment variable:

```
CIG_JOURNAL=/home/you/.cigscript/runs/20261028T110135-2899e6/hops/3.jsonl
```

The file may not exist yet. The tool appends to it, one JSON object per
line, and never reads or rewrites it. A dry run sets nothing: a tool that
finds `CIG_JOURNAL` unset is not under cig, and a tool under cig that has
nothing to say appends nothing. The variable travels with `CIG_RUN_ID`,
which names the run.

## The lines

| line | meaning |
|---|---|
| `{"op":"write","path":"db/schema.sql","before":"sha256:…","saved":"/tmp/schema.orig"}` | the tool wrote `path`. `before` is the SHA-256 of the bytes that were there (or `"absent"`); `saved` is where the tool kept a copy of them, when it did |
| `{"op":"delete","path":"old.cfg","before":"sha256:…","saved":"…"}` | the tool removed `path`; the same two fields |
| `{"op":"create","path":"generated/out.js"}` | the tool made `path`, which did not exist before |
| `{"op":"compensate","label":"migration 0042","undo":["migrate","down","0042"],"cwd":"db"}` | how to undo what the tool did: a command (no shell), run on rollback and on `cig unburn`, from `cwd` (relative to the hop's working directory; the hop's own when omitted) |

`path` is relative to the hop's working directory, or absolute. Unknown
fields are ignored; unknown `op`s and lines that are not JSON are reported
by line number and skipped.

## What cig does with them

The kernel never trusts a hop's word about bytes it can check, and never
journals as reversible what it cannot put back.

- **Inside the pack or nothing.** A reported path outside the pack's roots
  is refused: reported once as outside the pack (`E752`), journaled as
  nothing. The protocol widens what a hop can make reversible, not where.
- **A `create`** is journaled as a file the hop created (rollback removes
  it) when the path exists after the hop and the scan before it did not see
  it. When the scan saw it, the claim is false and is dropped with a word.
  The claim earns its keep past the watch cap, where the scan alone cannot
  tell a file the hop made from one it never reached.
- **A `write` or `delete` of a file under `touches`** adds nothing the tin
  did not already have: the tin's copy is what comes back. When the tool's
  `before` hash disagrees with the tin's copy, the tin wins and the report
  says the tool's claim did not match.
- **A `write` or `delete` of a file not under `touches`** is reversible when
  the tool kept the bytes: `saved` is hashed, must match `before` when both
  are given, and goes into the tin (sealed, when the path is a secret's);
  the rollback puts it back. Without `saved`, or with a copy that does not
  hash as claimed, the change is journaled as irreversible with the reason
  (`the hop kept no copy of the bytes before it`, or `the copy it kept does
  not hash to the before it claims`).
- **A `compensate`** is journaled as a compensation, exactly as a
  `burn { } unburn { }` block's is: it runs at its place in the newest-first
  order, on the automatic rollback and on `cig unburn`, from the directory
  it names, with `CIG_RUN_ID` set, and a non-zero exit is reported as the
  compensation failing (`E7xx` in the report, the run `rollback_incomplete`).
  The hop and its unverified changes are labelled `compensated` in the plan
  and the record, as a block's are. The undo may say as much as it likes on
  either pipe (both are drained while it runs); the report quotes the first
  200 characters of its last stderr line.
- **Without a `pack { }`**, a `compensate` still counts: the hop's own undo
  needs no pack. A `write`, `delete` or `create` has nothing to be judged
  against and is said and dropped (`E761`).
- **A `proc.pipe`** gives every stage the same `CIG_JOURNAL`; the pipeline
  is one hop, and its report is read once it is over.

What this guarantees: a tool that reports its writes with copies of the
before-bytes, or an undo command, is as reversible as a native burn without
anyone writing `touches`; and nothing a tool says can make cig remove or
rewrite a file whose before-bytes it does not hold.

## cig is the first speaker

A `cig run` started by a hop (`CIG_RUN_ID` and `CIG_JOURNAL` are set) shares
its parent's pack since 1.4.1. From 1.6.0 it also reports to its parent:
when its run finishes with burns in its journal, it appends one line,

```
{"op":"compensate","label":"cig run deploy.cig (run 20261028T…)","undo":["cig","unburn","20261028T…"]}
```

so the parent's rollback, or a later `cig unburn` of the parent, puts the
child's run back through the child's own journal, which is exact.

## Ten lines in three languages

Python:

```python
import hashlib, json, os, shutil
def report(**line):
    if p := os.environ.get("CIG_JOURNAL"):
        with open(p, "a") as f: f.write(json.dumps(line) + "\n")
before = hashlib.sha256(open("db/schema.sql", "rb").read()).hexdigest()
shutil.copy("db/schema.sql", "/tmp/schema.sql.orig")
write_new_schema("db/schema.sql")
report(op="write", path="db/schema.sql", before=f"sha256:{before}", saved="/tmp/schema.sql.orig")
report(op="compensate", label="migration 0042", undo=["migrate", "down", "0042"])
```

Node:

```js
const fs = require("fs"), crypto = require("crypto");
const report = (line) => process.env.CIG_JOURNAL &&
  fs.appendFileSync(process.env.CIG_JOURNAL, JSON.stringify(line) + "\n");
const before = crypto.createHash("sha256").update(fs.readFileSync("db/schema.sql")).digest("hex");
fs.copyFileSync("db/schema.sql", "/tmp/schema.sql.orig");
writeNewSchema("db/schema.sql");
report({ op: "write", path: "db/schema.sql", before: `sha256:${before}`, saved: "/tmp/schema.sql.orig" });
report({ op: "compensate", label: "migration 0042", undo: ["migrate", "down", "0042"] });
```

bash:

```bash
report() { [ -n "${CIG_JOURNAL:-}" ] && printf '%s\n' "$1" >> "$CIG_JOURNAL"; }
before=$(sha256sum db/schema.sql | cut -c1-64)
cp db/schema.sql /tmp/schema.sql.orig
migrate up 0042
report "{\"op\":\"write\",\"path\":\"db/schema.sql\",\"before\":\"sha256:$before\",\"saved\":\"/tmp/schema.sql.orig\"}"
report '{"op":"compensate","label":"migration 0042","undo":["migrate","down","0042"]}'
```

A tool that keeps no copy may still report the change (cig records it
honestly as irreversible, with the path, where the scan would have said
only `hop modified`) and the compensation, which is the part that undoes
it.
