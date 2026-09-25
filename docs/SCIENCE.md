# CigScript in the lab

Data work has a particular failure: the script that "just cleans up the raw
files" and, in doing so, quietly rewrites the only copy. CigScript was not
designed for laboratories, but its one rule happens to be the rule every lab
already has and no scripting language enforces: **do not touch the primary
data without a record of what you did and a way back.**

Three properties make it fit:

1. **Raw data can be made physically untouchable by the language.** Reads
   are free everywhere; writes, moves and deletes only happen inside a
   `burn { }` block, and `cig check` refuses a script that tries otherwise.
   A pipeline that reads `raw/` and writes `derived/` cannot corrupt `raw/`
   by accident unless someone wrote `burn` around it on purpose, which is
   visible in review.
2. **Every run is a provenance record.** `~/.cigscript/runs/<id>/run.json`
   holds the script's SHA-256, the arguments, the working directory and the
   timestamps; `journal.jsonl` holds every effect with the hash of what was
   there before. That is a lab notebook entry that writes itself.
3. **Determinism is the default.** Ordered maps, sorted listings, checked
   integer arithmetic, no random numbers, and the same inputs give the same
   plan. `cig run --dry-run` shows the plan before the data is touched.

The three examples below are in `examples/` and run as-is; the test suite
executes them.

## Summary statistics with a report you can cite

`examples/lab-measurements.cig` reads a CSV of readings, computes mean,
standard deviation, minimum and maximum per series, and writes a JSON
report carrying the input file's hash so the numbers can be traced to the
bytes that produced them.

```cig
stick rows = csv.read(input, {header: true})
roll series = {}
for row in rows {
  stick name = row.series
  if not series.has(name) {
    series[name] = []
  }
  series[name].push(float(row.value))
}

pull describe(xs) {
  stick n = xs.len()
  stick mean = xs.sum() / n
  stick var = xs.map(pack(x) => math.pow(x - mean, 2)).sum() / (n - 1)
  snuff {n: n, mean: math.round(mean, 4), sd: math.round(math.sqrt(var), 4),
         min: xs.min(), max: xs.max()}
}

burn {
  json.save(output, {
    input: input, input_sha256: hash.sha256_file(input),
    generated: time.now_iso(),
    series: series.keys().sort().map(pack(k) => [k, describe(series[k])]),
  })
}
```

Sample standard deviation, not population: the divisor is `n - 1`, and the
example says so in a comment because a reviewer will ask.

## A manifest for a dataset

`examples/lab-manifest.cig` walks a data directory, hashes every file, and
writes `MANIFEST.json`. Run it again with `-- verify` and it reports what
changed, what is missing and what is new, without touching anything:

```cig
stick files = fs.glob("${root}/**/*").filter(pack(p) => fs.is_file(p) and path.base(p) != "MANIFEST.json")
roll manifest = {}
for f in files {
  manifest[f] = {sha256: hash.sha256_file(f), bytes: fs.size(f)}
}
```

Verifying is a pure read; writing the manifest is one `burn`, and the
dry-run of a write shows exactly one op. The manifest itself is the kind of
file a journal or a data steward asks for.

## A pipeline as a chain

`examples/lab-pipeline.cig` declares the stages as sticks and the pipeline as
a chain:

```cig
chain analysis { ingest, clean, analyse, report }
```

- `seed` writes a small synthetic dataset when no raw file exists, so the
  example runs standalone.
- `ingest` reads the raw file, hashes it, and copies it into `work/` for the
  record (a burn, journaled; the raw file is never opened for writing).
- `clean` drops rows outside a plausible range, records how many, and writes
  the cleaned rows under `work/`.
- `analyse` computes the statistics from the rows it was handed.
- `report` writes JSON and a short Markdown summary.

The rows travel between stages in memory and the files under `work/` are
provenance, which is why `cig light --dry-run lab-pipeline.cig analysis`
prints every file the pipeline would write once the raw file exists, without
writing any of them. `cig light lab-pipeline.cig analysis` runs it with a
timed log per stage, and a failure in `analyse` rolls back what `ingest` and
`clean` wrote. `cig runs <id>` afterwards is the record of the run, and
`cig unburn <id>` reverses it entirely if the input turns out to be wrong.

## What this is not

CigScript is not a numerical library. There are no arrays beyond lists, no
matrices, no plotting, and the arithmetic is 64-bit. The intended shape is:
CigScript around the pipeline, keeping the world honest; the heavy numbers in
whatever you already use, called through `proc.run` inside a burn, with its
inputs and outputs in the journal. `burn unlit` is the tool for declaring
the destructive step (deleting the raw copy after archival, say) in the
script so it is reviewed, without any way for it to run before someone
changes `unlit` to nothing.
