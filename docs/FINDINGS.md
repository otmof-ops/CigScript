# Findings

Measurements behind the numbers the other documents quote. Each entry says
what was run, on what, and what came out, so the figure can be checked by
running the same thing again rather than believed.

## The tin in chunks (1.5.0, #82)

**Question.** A hop edits a 200 MB lockfile a little on every run, and the
run is rolled back each time. What does the tin cost per run?

**Setup.** Linux x86_64 (ext4, NVMe), one pack root holding a 200,000,000
byte file of random bytes (incompressible, so nothing but chunking can
share anything). Each run: a Python hop under `touches: ["**"]` writes one
byte at a new offset and appends one 20-byte line, then the script coughs,
so the rollback restores the file from the tin and moves the hop's output
into the tin. After each run, `cig --json tin du`. The script and the
driver are reproducible from this description; the state directory was
outside the pack.

**Before: cig 1.4.3, one object per distinct content.**

| run | seconds | objects | bytes stored |
|---|---|---|---|
| 1 | 0.52 | 3 | 400,000,272 |
| 2 | 0.52 | 4 | 600,000,292 |
| 3 | 0.47 | 5 | 800,000,312 |
| 4 | 0.49 | 6 | 1,000,000,332 |
| 5 | 0.49 | 7 | 1,200,000,352 |

Every run stored the hop's output whole: 200 MB a run, 1.2 GB on disk after
five. Fifty runs would have cost about 10 GB.

**After: cig 1.5.0, content-defined chunks (FastCDC, 16 KiB min, 64 KiB
average, 256 KiB max; files of 64 KiB and more).**

| run | seconds | objects (chunks) | bytes stored | bytes the kept files would take |
|---|---|---|---|---|
| 1 | 3.37 | 2,483 | 200,185,319 | 400,000,272 |
| 2 | 0.86 | 2,484 | 200,208,709 | 600,000,292 |
| 5 | 0.87 | 2,487 | 200,469,991 | 1,200,000,352 |
| 10 | 0.91 | 2,492 | 201,028,857 | 2,200,000,452 |
| 20 | 0.87 | 2,502 | 202,049,669 | 4,200,000,652 |

Twenty runs, 4.2 GB of file versions kept, 202 MB stored: the original once
(2,482 chunks) and then one new chunk a run, the one the byte landed in,
plus a tiny chunk for the appended tail. On disk with directories,
206,752,283 bytes. The file came back byte for byte after every rollback
(the SHA-256 matched the original each time).

**Cost.** The first run is slower (3.4 s against 0.5 s): the stash reads and
chunks 200 MB, the rollback streams 200 MB back out of the chunks, and the
hop's output is chunked on its way into the tin instead of being renamed.
Later runs settle at about 0.9 s, since a chunk the tin already holds is
checked against its name (one read) and never written again. Where 1.4
copied 200 MB and renamed 200 MB a run, 1.5 reads about 600 MB and writes
about 90 KB.

## Hop watching that scales (1.6.0, #84)

**Question.** How long does a hop take to watch, and how much memory does
it hold, on a pack of 100,000, 500,000 and a million entries; and does
every change still come back?

**Setup.** Linux x86_64 (ext4, NVMe, 8 threads), one pack root holding N
one-byte files in directories of a thousand, two levels deep. One hop under
`touches: ["d000/s000/f0001.txt"]` writes that file and creates another,
then the script coughs. Measured with `/usr/bin/time -f "%e %M"` around the
whole `cig run` (both scans, the stash, the hop, the rollback); the
"restored" and "removed" columns are the file checked afterwards.

**Before: cig 1.5.0** (one thread, a map of paths, `watch_cap` 50,000).

| entries | seconds | max RSS | saw the whole pack | modified file restored | created file removed |
|---|---|---|---|---|---|
| 100,000 | 0.55 | 25 MB | no (stopped at 50,000) | **no** | **no** |

Past the cap, 1.5.0 said so and left both changes: the file it had not
reached was not kept, and the created file could not be told from one the
scan never saw.

**After: cig 1.6.0** (every core, one arena of relative paths, entries of
about 60 bytes sorted by path, a merge walk for the difference, the scan
after reusing the scan before for directories whose mtime has not moved;
`watch_cap` 5,000,000, a memory guard).

| entries | seconds | max RSS | saw the whole pack | modified file restored | created file removed |
|---|---|---|---|---|---|
| 100,000 | 0.25 | 34 MB | yes | yes | yes |
| 500,000 | 1.00 | 106 MB | yes | yes | yes |
| 1,000,000 | 2.03 | 214 MB | yes | yes | yes |

Two seconds and about 210 MB for a million entries, the scan before and
the scan after both held at once (about 100 MB each: 60 bytes an entry and
the sort's working space). Half of 1.5.0's time at 100,000 entries while
reading twice as much. The issue hoped for 60 to 100 MB a million; the
scan itself is in that range and holding two of them is the rest. macOS
and Windows were not measured here; the CI runs the half-million test on
Linux in release mode.
