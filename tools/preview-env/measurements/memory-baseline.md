# Where dux's memory goes: a measured baseline

Every figure below was read off `/proc/<pid>/smaps_rollup` and `/proc/<pid>/status`
inside the preview container while dux was in the stated condition. Numbers marked
**measured** were printed by the machine. Anything marked **inference** is a reading
of those numbers, not a second measurement.

The short answer: an independent report's unmeasured claim that a terminal with a
full 10,000-line scrollback holds roughly 19 MiB at 80 columns and 37 MiB at 160,
and that everything else is small, is **confirmed**. Measured here: 19.5 MiB per
agent at 80 columns and 38.7 MiB at 160, against a 12.2 MiB idle floor of which only
about 2 MiB is heap.

## Environment

| | |
| --- | --- |
| Host kernel | 7.2.5-1-cachyos |
| Container base | `archlinux:latest`, glibc 2.44 |
| dux build | `cargo build --release --bin dux`, rustc 1.98.1 |
| Product commit | `2e534bb4` (branch `server-mode`) |
| Worktree commit | `7502c742` (the same product code plus the tools-only burst fixture below) |
| Serving mode | `dux server --bind 0.0.0.0:8790 --no-tailscale`, no TUI |
| Provider | the preview's `fake` provider |
| `ui.agent_scrollback_lines` | 10000 (the shipped default) |

## Method

- The container was brought up with `DUX_SRC=<worktree> ./up.sh` and wiped with
  `docker compose down -v` between states, so every "fresh start" below really is a
  first boot with an empty workspace.
- Sampling: `docker exec` of a small shell script that reads `smaps_rollup` and
  `status` for `pidof dux`. Three samples two seconds apart per state; the tables
  give the median and the range. The range is usually zero: dux's RSS is extremely
  quiet once a state has settled.
- Agents were created over REST (`POST /api/v1/sessions`), so no browser was
  attached except where a state says otherwise. A REST-created agent's PTY is
  **24 rows by 80 columns** (the web's fixed launch seed), confirmed by reading the
  child's own `stty size` through `/proc/<child>/fd/0`.
- Scrollback was filled with a measurement-only fixture added to
  `tools/preview-env/fake-agent.sh` (`DUX_FAKE_FIXTURE=burst`, with
  `DUX_FAKE_BURST_LINES`, `DUX_FAKE_BURST_COLS` and `DUX_FAKE_BURST_DELAY`). It
  prints a stated number of lines of a stated width and then idles. The shipped
  `live` fixture prints under two lines a second and cannot fill a 10,000-line
  scrollback in any reasonable time. That fixture is a tools change, committed
  separately; no product code was touched for this measurement.
- Wide-terminal states were resized by opening the tab's own PTY WebSocket, sending
  one take-over resize frame, and closing it again. The child's winsize and the
  emulator grid survive the socket; only ownership is released. The 80-column arm
  was run through the same procedure as a control, and it landed within 20 kB of the
  agent that was never resized, so the procedure itself costs nothing.
- The in-app figure is `GET /api/v1/resources`, the read behind the web Task Manager.

## The states

All RSS figures are the dux process only, in kB as the kernel reports them.
"Δ idle" is against the 12496–12800 kB idle floor measured on the same run.

### a. Freshly started, no agents, no browser (measured)

Three separate fresh containers:

| Run | Rss | Pss | Anonymous | Shared_Clean | Private_Dirty | Threads |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 12800 | 11295 | 2400 | 2428 | 2432 | 26 |
| 2 | 12800 | 11302 | 2332 | 2388 | 2364 | 26 |
| 3 | 12496 | 11058 | 2336 | 2296 | 2368 | 26 |

Range across runs: 12496–12800 kB. Within a run the three samples were identical.

What the floor is made of, by mapping (measured, run 3, total 12636 kB at that
instant):

| Backing | Rss |
| --- | --- |
| `/usr/local/bin/dux` (text + rodata, file-backed, clean) | 7792 kB |
| `libc.so.6` | 1704 kB |
| `[heap]` | 1376 kB |
| anonymous mappings | 640 kB |
| `libm.so.6` | 620 kB |
| `ld-linux`, `libgcc_s`, `[stack]`, sqlite shm, misc | ~500 kB |

**Inference:** about 10.2 MiB of the 12.2 MiB idle floor is file-backed pages of the
binary and its shared libraries, most of which the kernel can evict and re-read at
will and which is shared with any other dux process on the machine. The part dux
actually allocates at idle is about 2 MiB.

### b. Same, plus one browser page on the workspace (measured)

One headless Chromium tab at 1440x900 on `/`, no agent selected.

| Rss | Pss | Anonymous | Threads | Δ idle |
| --- | --- | --- | --- | --- |
| 14604 (14604–14604) | 13143 | 3244 | 26 | +1804 kB (1.8 MiB) |

In-app Task Manager: `dux (this process)` 15007744 bytes. That is the same number as
`VmRSS` sampled a moment apart, so **the in-app figure is plain process RSS**
(measured: 13295616 bytes reported while `VmRSS` read 12936 kB in state a).

### c. One agent, scrollback filled to the 10,000-line cap, no browser (measured)

PTY geometry 24 rows x 80 columns (read off the child). One live PTY, one provider
process.

| Run | Rss | Anonymous | Threads | Δ idle |
| --- | --- | --- | --- | --- |
| first container | 35160 | 23736 | 28 | +22360 kB |
| fresh container | 33672 (33668–33672) | 23204 | 28 | +20872 kB |

The 1.5 MiB spread between the two is ordinary churn in the rest of the process (the
first container had had a browser attached and several git operations earlier). The
fresh-container figure is the one to quote: **+20.4 MiB for one agent with a full
10,000-line scrollback at 80 columns.**

In-app: `dux (this process)` 33.0 MiB, plus an `Agent (fake)` row of 5.7 MiB for the
provider's own two processes.

### d. After the process is stopped, and after the agent is deleted (measured)

Same agent as state c (first container, whose full-scrollback figure was 35160).

| State | Rss | Anonymous | Threads | Δ idle |
| --- | --- | --- | --- | --- |
| streaming, scrollback full | 35160 | 23736 | 28 | +22360 kB |
| provider stopped, tab dormant | 16448 (16448–16448) | 5020 | 26 | +3648 kB |
| agent deleted (worktree removed) | 16508 (16508–16508) | 5080 | 26 | +3708 kB |

**The memory comes back, and it comes back when the process stops, not when the
agent is deleted.** 18712 kB of the 22360 kB (84%) was returned to the kernel the
moment the PTY went away; deleting the agent returned nothing further. The 3.6 MiB
that remained above the idle floor is a process that has been running for a while
and has done git work, not retained history: it is 2.6 MiB of anonymous memory over
a floor of 2.4, and state b showed +0.8 MiB of that appearing from a browser page
alone. **Inference**, not separately measured.

### e. Three agents, each with a full scrollback (measured)

Fresh container, agents added one at a time, all at 24x80.

| Live agents | Rss | Anonymous | Threads | Δ idle | Δ previous |
| --- | --- | --- | --- | --- | --- |
| 0 | 12800 | 2332 | 26 | – | – |
| 1 | 33672 | 23204 | 28 | +20872 kB | +20872 kB |
| 2 | 52804 | 42336 | 30 | +40004 kB | +19132 kB |
| 3 | 72788 | 62320 | 32 | +59988 kB | +19984 kB |

Two threads per agent. The cost is **linear**: 20.4, 18.7 and 19.5 MiB for the first,
second and third. In-app at three agents: dux 71.1 MiB, three `Agent (fake)` rows of
about 5.7 MiB each, TOTAL 88.3 MiB.

### f. 80 columns versus 160 columns (measured)

Fresh container each. Identical procedure: create the agent, resize its PTY through
its own socket, close the socket, let the burst print 10,000 lines whose width
matches the grid.

| Grid | Line width | Rss | Anonymous | Δ idle | Per row | Per cell |
| --- | --- | --- | --- | --- | --- | --- |
| 24 x 80 | 80 chars | 33692 (33692–33692) | 23240 | +20892 kB | 2139 B | 26.7 B |
| 24 x 160 | 160 chars | 52412 (52412–52412) | 42024 | +39612 kB | 4056 B | 25.4 B |

Doubling the columns multiplied the cost by **1.90**. **Wide terminals cost very
nearly proportionally more**, and the constant that falls out is about 26 bytes of
RSS per stored cell.

### g. Streaming 20,000 lines past the cap (measured)

Fresh container, one agent, the burst set to 30,000 lines at 80 columns. Sampled
every five seconds for three minutes.

| Moment | Rss |
| --- | --- |
| PTY up, nothing printed yet | 14276–14288 |
| first sample after output starts | 33504 |
| every sample for the next 2.5 minutes | 33504, then 33476 |
| final three-sample median | 33476 (33476–33476) |

**RSS plateaus and stays there.** 30,000 lines cost the same as 10,000 (33476 against
33672 for the 10,000-line run, i.e. very slightly less). There is no allocator
retention to chase past the cap: the ring reaches its size and the numbers stop
moving, to the kilobyte, for minutes.

Note also the pre-output line: a PTY that exists but has printed nothing costs about
1.5 MiB, not 20. **The history is grown as lines arrive, not preallocated at spawn.**
State h below shows the same thing from the other side.

### h. One companion shell with 10,000 lines of `seq` (measured)

A standalone terminal (`POST /api/v1/terminals`), claimed over its socket, told to run
`seq 1 10000`. Each printed line is at most five characters.

| State | Rss | Anonymous | Δ |
| --- | --- | --- | --- |
| terminal spawned, nothing printed | 12760 (12756–12760) | 2624 | +0 vs idle |
| after `seq 1 10000` | 32440 (32440–32440) | 22176 | +19680 kB |

An empty live PTY costs nothing measurable. Ten thousand five-character lines cost
**19.2 MiB**, within 6% of the agent's ten thousand eighty-character lines. **The cost
is per stored row at the full grid width, not per character typed.**

### Where the growth lands, by mapping (measured)

The same by-mapping breakdown as state a, taken with one full 10,000-line scrollback
present (total 33544 kB):

| Backing | Idle | One full scrollback | Δ |
| --- | --- | --- | --- |
| anonymous mappings | 640 kB | 21420 kB | +20780 kB |
| `[heap]` | 1376 kB | 1376 kB | 0 |
| `/usr/local/bin/dux` | 7792 kB | 7856 kB | +64 kB |
| everything else | ~2800 kB | ~2900 kB | ~+100 kB |

All of the growth is anonymous memory and none of it is in the `brk` heap.
**Inference:** allocations of this size go through `mmap` rather than `brk`, which is
also why the kernel gets the pages straight back on free (state d).

## Conclusions

**What fraction of RSS is terminal history.** With one agent holding a full
10,000-line scrollback at 80 columns, 20.4 MiB of a 32.9 MiB process is terminal
history: **62%**. With three such agents, 58.6 MiB of 71.1 MiB: **82%**. Terminal
history is the only part of dux's footprint that scales with use, and past two agents
everything else is a rounding error. (Measured; the percentages are arithmetic on
measured figures.)

**What the idle floor is and what it consists of.** 12.2–12.5 MiB, reproducible to
within 300 kB across three fresh boots. About 7.6 MiB of that is the dux binary's own
text and read-only data, about 2.5 MiB is libc/libm/ld, and only about 2 MiB is memory
dux allocated. There is nothing to win here: the floor is mostly file-backed pages the
kernel can drop, and the heap at idle is smaller than a single screenful of terminal.
(Measured.)

**Does memory come back.** Yes, and promptly. Stopping an agent's provider returned
84% of what its scrollback held, in the same second, to the kernel. Deleting the agent
afterwards returned nothing more, because there was nothing left to return. There is
no leak in the stop/delete path at this granularity. (Measured.)

**Do wide terminals cost proportionally more.** Yes, 1.90x for 2x the columns. The
underlying constant is about 26 bytes of RSS per stored cell, and it holds at both
widths and for both an agent and a plain shell. A 10,000-line scrollback is really a
10,000 x <columns> cell grid, and its cost is set by the grid, not by the text.
(Measured. **Inference:** the 26 bytes is the emulator's per-`Cell` footprint plus
per-row `Vec` overhead; nothing here separated the two.)

**The in-app number.** `dux (this process)` in the web Task Manager is exactly the
process's RSS as `VmRSS` reports it. It does not include the provider processes, which
appear as their own `Agent` / `Terminal` rows (about 5.7 MiB for the fake provider's
shell pair, about 4.6 MiB for a plain login shell). The `TOTAL` row is their sum.
(Measured.)

## What was not measured, and why

- **No TUI.** Every figure is `dux server` with no terminal UI in front of it. The
  `serve_while_tui` and `start-web-server` modes hold the engine on a different
  thread and add a ratatui surface; nothing here says what that costs.
- **A browser attached to a full-scrollback agent** was not sampled. State b measured
  a browser on an empty workspace (+1.8 MiB) and state c measured a full scrollback
  with no browser; the combination was not run, so no claim is made about whether a
  PTY socket's replay path holds a second copy of anything while it streams.
- **The raw byte stream versus the cell grid.** An agent's 10,000 eighty-character
  lines cost about 1.2 MiB more than a shell's 10,000 five-character lines. That is
  roughly the size of one raw copy of the agent's own bytes (810 kB), which would be
  consistent with a byte buffer being held somewhere alongside the grid, but the two
  states differ in more than one way and nothing here isolated it. Flagged as a
  question, not a finding.
- **Fragmentation over a long session.** Every state here was reached in minutes.
  Whether a process that has held and released many scrollbacks over days returns to
  the same floor was not tested.
