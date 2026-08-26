#!/usr/bin/env python3
"""AN-005 — call-edge recall benchmark, scored against the LSP oracle.

mezz resolves call edges by name heuristic. AN-004 added an exact path: with
`MEZZ_LSP_EXACT=1`, rust-analyzer resolves each Rust call site and the edge is
labelled `exact`. That gives us ground truth to score the heuristic against.

This script analyses one tree twice — once heuristic (the *subject*, what every
agent gets today), once exact (the *oracle*) — pairs the Calls edges between the
two runs by `(source_id, order)`, and classifies each pair:

    agree        heuristic target == oracle target
    mistargeted  both resolved, different targets      (precision loss)
    missed       oracle resolved, heuristic did not    (recall loss)

Edges the oracle itself could not resolve (std, external crates, macro-generated
call sites) are *excluded* rather than counted as agreement — leaving them in
would inflate agreement with cases nobody resolved. This is why the JSON
renderer exposes `precision`: without it, an unchanged target is ambiguous
between "the oracle confirmed the guess" and "the oracle never looked".

    recall    = agree / (agree + mistargeted + missed)
    precision = agree / (agree + mistargeted)

Recall is the number this benchmark exists for: a missed edge is why `impact`
reports "Used by (0)" for a function that has callers.

Not a CI gate. rust-analyzer needs ~130s on mezz itself before it answers a
single go-to-definition, and that cost does not amortise across runs (the
tracer's lifecycle is spawn-per-analysis). Run it by hand, commit the report.

Both runs analyse with a **cold, private parse cache**. mezz's parse store is
global and generation-keyed; borrowing whatever the ambient `~/.cache/mezz`
holds would measure the parser that filled it rather than the binary under
test. `--mezz` defaults to `mezz` on PATH, so the report names the binary by
content hash — the checkout you are standing in is not evidence of what ran.

**The oracle is a fixture, not a re-derivation.** Even cold and correctly
attributed, two runs do not agree on what the ground truth *is*:
rust-analyzer answers from whatever it has finished indexing when the batch
fires, and the tracer's readiness signal returns as soon as one probe
resolves. Measured on one pristine corpus: 3589 exact edges (AN-005), 3649
(AN-012), then 3644, 3676 and 3666 within one afternoon. That is tens of
edges of ground truth moving underneath the percentage, and a resolution fix
worth celebrating is 24. So two headlines scored against two oracle runs
cannot be subtracted, however cleanly each was measured.

To measure a change, pin the oracle and vary only the binary:

    # once — pay the 300s, keep the ruler
    python3 scripts/call_edge_recall.py --path CORPUS --save-oracle oracle.json

    # then per binary, in seconds each, on the same ground truth
    python3 scripts/call_edge_recall.py --path CORPUS --oracle oracle.json \
        --mezz ./before/mezz --out before.md
    python3 scripts/call_edge_recall.py --path CORPUS --oracle oracle.json \
        --mezz ./after/mezz  --out after.md

Usage:
    python3 scripts/call_edge_recall.py                     # analyse cwd
    python3 scripts/call_edge_recall.py --path ../other-repo
    python3 scripts/call_edge_recall.py --keep-json         # keep raw analyses
    python3 scripts/call_edge_recall.py --reuse /tmp/an005  # re-score, no re-run
"""

import argparse
import datetime
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from collections import Counter
from pathlib import Path

# A degraded oracle is this benchmark's one silent failure mode: every tracer
# failure path (no rust-analyzer, no Cargo.toml, timeout, budget overrun)
# returns an empty upgrade map, which scores as perfect recall. AN-004 measured
# ~2.4k exact edges on mezz itself, so anything in the low hundreds means the
# oracle degraded rather than that the tree is small. Small crates legitimately
# fall below this — pass --min-exact to lower it, deliberately.
DEFAULT_MIN_EXACT = 200

GHOST_PREFIX = "ghost:"


def run_mezz(mezz, path, out_file, exact, timeout, cache_dir):
    """One `mezz analyze` run. Returns (elapsed_seconds, stderr_text)."""
    env = dict(os.environ)
    if exact:
        env["MEZZ_LSP_EXACT"] = "1"
    else:
        # Do not inherit an exported MEZZ_LSP_EXACT into the subject run — that
        # would score the oracle against itself and report perfect recall.
        env.pop("MEZZ_LSP_EXACT", None)

    # Analyse cold, in a cache nothing else writes to.
    #
    # mezz's parse store is global (`~/.cache/mezz`) and keyed by a generation
    # tag. Any warm generation the ambient cache happens to hold is served in
    # preference to re-parsing, so a benchmark run inherits whatever parser
    # produced those entries rather than the binary under test. Measured on
    # this repo: the same binary emitted 975 different call targets warm vs
    # cold, which is 40x the effect size of a typical parser fix. A benchmark
    # that can report the previous parser's numbers is not a benchmark.
    env["MEZZ_CACHE_DIR"] = str(cache_dir)

    cmd = [mezz, "analyze", str(path), "-f", "json", "-o", str(out_file),
           "-l", "rust", "--include-tests"]
    started = time.monotonic()
    proc = subprocess.run(cmd, env=env, capture_output=True, text=True,
                          timeout=timeout)
    elapsed = time.monotonic() - started
    if proc.returncode != 0:
        sys.exit(f"mezz analyze failed ({'exact' if exact else 'heuristic'} run, "
                 f"exit {proc.returncode}):\n{proc.stderr[-2000:]}")
    return elapsed, proc.stderr


def load(path):
    with open(path) as fh:
        doc = json.load(fh)
    entities = {e["id"]: e for e in doc.get("entities", [])}
    calls = {}
    unkeyed = 0
    for rel in doc.get("relationships", []):
        if rel.get("kind") != "calls":
            continue
        order = rel.get("metadata", {}).get("order")
        if order is None:
            # Call edges from a parser that does not emit order metadata.
            # Cannot be paired across runs; counted and reported, not scored.
            unkeyed += 1
            continue
        calls[(rel["source_id"], order)] = rel
    return doc, entities, calls, unkeyed


def is_unresolved(target_id, entities):
    """True when this target is a ghost — mezz could not resolve the callee."""
    if target_id.startswith(GHOST_PREFIX):
        return True
    entity = entities.get(target_id)
    if entity is None:
        return True
    return "ghost" in entity.get("tags", [])


def callee_string(target_id, entities):
    """The raw callee text the parser emitted, recoverable from a ghost id."""
    if target_id.startswith(GHOST_PREFIX):
        return target_id[len(GHOST_PREFIX):]
    entity = entities.get(target_id)
    return entity.get("qualified_name", target_id) if entity else target_id


def module_stems(entities):
    """File stems and directory names in the analysed tree — used to tell
    `diff::f` (a module path) from `graph::f` (a receiver variable's text).
    Both are lowercase and both reach the resolver looking identical."""
    stems = set()
    for entity in entities.values():
        path = entity.get("file_path", "")
        if path.endswith(".rs"):
            stem = Path(path).stem
            if stem not in ("mod", "lib", "main"):
                stems.add(stem)
            parent = Path(path).parent.name
            if parent:
                stems.add(parent)
    return stems


def classify_shape(callee, stems):
    """Bucket a callee string by the syntax that produced it.

    Heuristic, and reported as such: the JSON does not carry the tree-sitter
    node kind, so `Type::method` (a method call whose receiver type inference
    resolved) and `path::func` (a scoped identifier) are told apart by case,
    and a lowercase prefix is attributed to a module only when it matches a
    real file stem in the tree. Without that check `graph::detect_smells` and
    `diff::resolve_git_ref` are indistinguishable.
    """
    if "::" not in callee:
        return "bare identifier"
    prefix, _, _name = callee.rpartition("::")
    head = prefix.split("::")[0]
    if not head:
        return "bare identifier"
    if head[0].isupper():
        return "Type::method"
    if head in stems:
        return "module::function"
    return "receiver::method"


def location(entity):
    if not entity:
        return "?"
    start = (entity.get("span") or {}).get("start") or {}
    line = start.get("line", "?")
    return f"{entity.get('file_path', '?')}:{line}"


def one_line(text, limit=90):
    """Callee strings can carry whole chained expressions, newlines included.
    Keep the table readable without hiding that the string is junk."""
    flat = " ".join(str(text).split())
    return flat if len(flat) <= limit else flat[:limit - 1] + "…"


def score(subject_entities, subject_calls, oracle_calls, stems):
    buckets = {"agree": [], "mistargeted": [], "missed": []}
    out_of_scope = 0
    unpaired = 0

    for key in sorted(oracle_calls):
        oracle_rel = oracle_calls[key]
        # Only exact edges are ground truth. Everything else is a call the
        # oracle could not resolve either — excluded, not scored as agreement.
        if oracle_rel.get("precision") != "exact":
            out_of_scope += 1
            continue

        subject_rel = subject_calls.get(key)
        if subject_rel is None:
            unpaired += 1
            continue

        subject_target = subject_rel["target_id"]
        oracle_target = oracle_rel["target_id"]
        record = {
            "source_id": key[0],
            "order": key[1],
            "subject_target": subject_target,
            "oracle_target": oracle_target,
            "callee": callee_string(subject_target, subject_entities),
        }
        record["shape"] = classify_shape(record["callee"], stems)

        if subject_target == oracle_target:
            buckets["agree"].append(record)
        elif is_unresolved(subject_target, subject_entities):
            buckets["missed"].append(record)
        else:
            buckets["mistargeted"].append(record)

    return buckets, out_of_scope, unpaired


def pct(numerator, denominator):
    return f"{100.0 * numerator / denominator:.1f}%" if denominator else "n/a"


def shape_table(buckets):
    """Shape counts for the *missed* bucket only.

    Shape is read off the callee string the parser emitted, and that string
    survives only when resolution failed — a miss leaves it in the ghost id.
    Once an edge resolves, the target is a real entity and the original syntax
    is gone, so agreeing edges cannot be attributed to a shape without the
    parser recording it. Scoring them as "bare identifier" (what the entity's
    qualified_name happens to look like) would silently move every fix into
    that row, so this table reports the loss only.
    """
    missed = buckets["missed"]
    counts = Counter(r["shape"] for r in missed)
    total = len(missed) or 1
    return [(shape, n, f"{100.0 * n / total:.1f}%")
            for shape, n in sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))]


def git_sha(path):
    try:
        out = subprocess.run(["git", "-C", str(path), "rev-parse", "--short", "HEAD"],
                             capture_output=True, text=True, timeout=10)
        return out.stdout.strip() or "unknown"
    except Exception:
        return "unknown"


def oracle_coverage(stderr):
    """How many call sites rust-analyzer actually answered, per the tracer.

    A *total* oracle failure is caught by --min-exact. A partial one is not,
    and it is the reason reports from different days do not compare: the
    tracer works to a wall-clock budget, so a loaded machine simply answers
    fewer sites, and every unanswered site silently leaves the scored set.
    AN-011 and AN-012 excluded 11014 and 10951 edges respectively — a 63-edge
    swing in the denominator, on a change worth 24 edges. Reporting coverage
    is what lets a reader tell a real delta from a moved goalpost.

    Returns (answered, total) or None when the tracer said nothing (a --reuse
    re-score, or an older mezz that did not print the line).
    """
    answered = total = None
    for line in stderr.splitlines():
        got = re.search(r"resolved (\d+)/(\d+) call sites exact", line)
        if got:
            total = int(got.group(2))
            answered = total
        short = re.search(r"budget exceeded, (\d+)/(\d+) call sites answered", line)
        if short:
            answered, total = int(short.group(1)), int(short.group(2))
    return None if total is None else (answered, total)


def binary_identity(mezz):
    """What actually ran, read off the binary itself.

    This used to be `git rev-parse HEAD` on the *script's* repo, which names
    the checkout you are standing in and not the build you passed to `--mezz`.
    Two reports made from two different binaries therefore claimed the same
    provenance, so "no improvement" was indistinguishable from "measured the
    same build twice" — and `--mezz` defaults to whatever `mezz` is on PATH,
    which is exactly the case where they differ. Hash the file instead: it
    cannot be wrong about which bytes were executed.
    """
    exe = shutil.which(mezz) or mezz
    try:
        digest = hashlib.sha256(Path(exe).read_bytes()).hexdigest()[:12]
        built = datetime.datetime.fromtimestamp(
            Path(exe).stat().st_mtime).astimezone().isoformat(timespec="seconds")
        return {"mezz": str(Path(exe).resolve()), "mezz_build": digest, "mezz_mtime": built}
    except OSError as e:
        return {"mezz": str(exe), "mezz_build": f"unreadable ({e})", "mezz_mtime": "unknown"}


def oracle_sidecar(oracle_path):
    """Path of the provenance file written beside a saved oracle analysis."""
    return Path(str(oracle_path) + ".meta.json")


def ra_version():
    exe = shutil.which("rust-analyzer")
    if not exe:
        return "not on PATH"
    try:
        out = subprocess.run([exe, "--version"], capture_output=True, text=True,
                             timeout=30)
        return out.stdout.strip() or "unknown"
    except Exception:
        return "unknown"


def render(args, meta, buckets, out_of_scope, unpaired, subject_entities):
    agree = len(buckets["agree"])
    mis = len(buckets["mistargeted"])
    missed = len(buckets["missed"])
    scored = agree + mis + missed
    resolved = agree + mis

    out = []
    add = out.append
    add("# AN-005 — call-edge recall vs the LSP oracle\n")
    add(f"- Date: {meta['date']}")
    add(f"- Tree analysed: `{meta['path']}` @ `{meta['tree_sha']}`")
    add(f"- mezz build: `{meta['mezz_build']}` (`{meta['mezz']}`, built {meta['mezz_mtime']})")
    add(f"- rust-analyzer: `{meta['ra']}`")
    add("- Language filter: rust, tests included")
    add("- Parse cache: cold, private to this run")
    coverage = meta.get("oracle_coverage")
    if coverage:
        answered, asked = coverage
        add(f"- Oracle coverage: {answered}/{asked} call sites answered "
            f"({pct(answered, asked)})")
    def secs(value):
        return "unrecorded" if value != value else f"{value:.1f}s"  # NaN check
    add(f"- Wall clock: heuristic {secs(meta['subject_secs'])}, exact {secs(meta['oracle_secs'])}")
    add(f"- Oracle: {meta['oracle_origin']} — {meta['exact_edges']} exact edges")
    origin = meta.get("oracle_meta")
    if origin:
        add(f"  — measured {origin.get('date')} on `{origin.get('tree_sha')}`"
            f" with `{origin.get('ra')}`")
    if meta.get("reused_from"):
        add(f"- Re-scored from saved analyses at `{meta['reused_from']}`")
    add("")

    if meta.get("oracle_fresh"):
        add("> This run derived its own oracle. rust-analyzer resolves whatever it")
        add("> has finished indexing when the batch fires, so the scored set differs")
        add("> from every other run's by tens of edges — more than most single fixes")
        add("> move. **Do not subtract this headline from another report's.** To")
        add("> measure a change, `--save-oracle` once and `--oracle` it for both")
        add("> binaries.\n")

    add("Scored over Rust `Calls` edges the oracle resolved exactly. Edges the")
    add("oracle could not resolve (std, external crates, macros) are excluded")
    add("rather than counted as agreement.\n")

    add("## Headline\n")
    add("| Metric | Value |")
    add("|---|---|")
    add(f"| **Recall** (agree / scored) | **{pct(agree, scored)}** |")
    add(f"| **Precision** (agree / resolved) | **{pct(agree, resolved)}** |")
    add(f"| Scored edges | {scored} |")
    add(f"| — agree | {agree} |")
    add(f"| — mistargeted (wrong entity) | {mis} |")
    add(f"| — missed (left unresolved) | {missed} |")
    add(f"| Excluded: oracle could not resolve | {out_of_scope} |")
    add(f"| Unpaired (join-key failures) | {unpaired} |\n")

    if coverage and coverage[0] < coverage[1]:
        answered, asked = coverage
        add(f"> WARNING: the oracle answered only {answered} of {asked} call sites")
        add("> before its budget ran out, so the scored set here is a subset of")
        add("> the tree chosen by wall-clock rather than by anything meaningful.")
        add("> Percentages are still internally valid, but **do not compare this")
        add("> report to one with different coverage** — the denominator moved.")
        add("> Raise `MEZZ_LSP_TIMEOUT_SECS`, or re-run on a quieter machine.\n")

    if unpaired:
        add(f"> WARNING: {unpaired} oracle edges had no `(source_id, order)`")
        add("> counterpart in the heuristic run. The join key is supposed to be")
        add("> stable across runs; a non-zero count here undermines every number")
        add("> above. Investigate before trusting this report.\n")

    add("## What the recall loss is made of\n")
    add("Missed edges only — see `shape_table` for why resolved edges cannot be")
    add("attributed to a shape. Shape itself is inferred from the callee string,")
    add("not the parse tree; `classify_shape` documents its limits.\n")
    add("| Callee shape | missed | share of misses |")
    add("|---|---:|---:|")
    for row in shape_table(buckets):
        add(f"| `{row[0]}` | {row[1]} | {row[2]} |")
    add("")

    for bucket, title, blurb in (
        ("missed", "Missed edges",
         "The recall loss: the oracle resolved these, mezz left them on a ghost. "
         "Each one is a dependent `impact` will not report."),
        ("mistargeted", "Mistargeted edges",
         "The precision loss: mezz resolved these to a different entity than the oracle."),
    ):
        records = buckets[bucket]
        add(f"## {title} ({len(records)})\n")
        add(f"{blurb}\n")
        if not records:
            add("_None._\n")
            continue
        shown = sorted(records, key=lambda r: (r["callee"], r["source_id"]))[:args.top]
        add("| callee (as the parser emitted it) | shape | caller |")
        add("|---|---|---|")
        for record in shown:
            caller = location(subject_entities.get(record["source_id"]))
            add(f"| `{one_line(record['callee'])}` | {record['shape']} | {caller} |")
        if len(records) > len(shown):
            add(f"\n_… and {len(records) - len(shown)} more (raise with `--top`)._")
        add("")

    add("## Most-missed callees\n")
    counts = Counter(r["callee"] for r in buckets["missed"])
    if counts:
        add("| callee | missed edges |")
        add("|---|---:|")
        for callee, count in counts.most_common(args.top):
            add(f"| `{one_line(callee)}` | {count} |")
    else:
        add("_None._")
    add("")
    return "\n".join(out)


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--path", default=".", help="tree to analyse (default: cwd)")
    parser.add_argument("--mezz", default="mezz", help="mezz binary (default: mezz on PATH)")
    parser.add_argument("--out", default=None, help="report path (default: stdout)")
    parser.add_argument("--json-out", default=None, help="also write raw scores as JSON")
    parser.add_argument("--top", type=int, default=40, help="rows per detail table (default 40)")
    parser.add_argument("--min-exact", type=int, default=DEFAULT_MIN_EXACT,
                        help=f"abort below this many exact edges (default {DEFAULT_MIN_EXACT})")
    parser.add_argument("--timeout", type=int, default=1800, help="per-run timeout, seconds")
    parser.add_argument("--keep-json", action="store_true", help="keep the two raw analyses")
    parser.add_argument("--reuse", default=None,
                        help="dir holding a previous run's subject.json/oracle.json; re-scores without re-running mezz")
    parser.add_argument("--oracle", default=None,
                        help="score against this saved oracle analysis instead of running the "
                             "exact pass; the only way to compare two binaries on one ruler")
    parser.add_argument("--save-oracle", default=None,
                        help="write the oracle analysis here (plus provenance) so later runs can --oracle it")
    args = parser.parse_args()

    if args.reuse and (args.oracle or args.save_oracle):
        sys.exit("--reuse re-scores a saved pair; it cannot be combined with "
                 "--oracle/--save-oracle")
    if args.oracle and args.save_oracle:
        sys.exit("--oracle reuses an oracle; --save-oracle writes a new one. Pick one.")

    path = Path(args.path).resolve()
    if not path.is_dir():
        sys.exit(f"not a directory: {path}")

    oracle_stderr = ""
    recorded = None
    oracle_meta = None
    pinned = bool(args.oracle)
    if args.reuse:
        workdir = Path(args.reuse).resolve()
        subject_json, oracle_json = workdir / "subject.json", workdir / "oracle.json"
        if not subject_json.exists() or not oracle_json.exists():
            sys.exit(f"--reuse needs subject.json and oracle.json in {workdir}")
        subject_secs = oracle_secs = float("nan")
        oracle_origin = f"saved pair at `{workdir}`"
        # Provenance belongs to the run that produced these analyses, not to
        # whatever is checked out now. Re-deriving it here would silently
        # attribute the measurement to the wrong tree and the wrong binary.
        meta_file = workdir / "meta.json"
        if meta_file.exists():
            recorded = json.loads(meta_file.read_text())
    else:
        workdir = Path(tempfile.mkdtemp(prefix="an005-"))
        subject_json, oracle_json = workdir / "subject.json", workdir / "oracle.json"

        # One cold cache per run, inside the workdir so --keep-json keeps it
        # and the normal path deletes it with everything else.
        print("[1/2] heuristic run (subject) ...", file=sys.stderr)
        subject_secs, _ = run_mezz(args.mezz, path, subject_json, False, args.timeout,
                                  workdir / "cache-subject")
        print(f"      {subject_secs:.1f}s", file=sys.stderr)

        if pinned:
            # The whole point: no exact pass, so the ground truth is byte-identical
            # to the run that produced it and the only thing that varied is --mezz.
            oracle_json = Path(args.oracle).resolve()
            if not oracle_json.exists():
                sys.exit(f"--oracle: no such analysis: {oracle_json}")
            oracle_secs = float("nan")
            oracle_origin = f"pinned: `{oracle_json}`"
            sidecar = oracle_sidecar(oracle_json)
            if sidecar.exists():
                oracle_meta = json.loads(sidecar.read_text())
            print(f"[2/2] oracle pinned to {oracle_json} — exact pass skipped",
                  file=sys.stderr)
        else:
            print("[2/2] exact run (oracle, rust-analyzer) — the slow one ...",
                  file=sys.stderr)
            oracle_secs, oracle_stderr = run_mezz(args.mezz, path, oracle_json, True,
                                                 args.timeout, workdir / "cache-oracle")
            print(f"      {oracle_secs:.1f}s", file=sys.stderr)
            oracle_origin = "derived fresh by this run"

    _, subject_entities, subject_calls, subject_unkeyed = load(subject_json)
    _, _oracle_entities, oracle_calls, oracle_unkeyed = load(oracle_json)

    exact_count = sum(1 for r in oracle_calls.values() if r.get("precision") == "exact")
    if exact_count < args.min_exact:
        tail = "\n".join(oracle_stderr.strip().splitlines()[-15:])
        sys.exit(
            f"ORACLE DEGRADED: only {exact_count} exact call edges "
            f"(expected >= {args.min_exact}).\n"
            "Every tracer failure path returns an empty upgrade map, which would\n"
            "score as perfect recall — so this aborts rather than publishing a\n"
            "flattering number. Check rust-analyzer is on PATH and the tree has a\n"
            "Cargo manifest, or lower --min-exact deliberately.\n"
            f"\nTail of the oracle run's stderr:\n{tail}")

    # A pinned oracle brings its own way to be silently wrong: point it at a
    # ruler measured on a different tree and every edge scores as a miss, which
    # reads as a catastrophic regression rather than as operator error. The join
    # keys are entity ids, so a mismatched corpus strands almost all of them.
    if pinned:
        exact_keys = {k for k, r in oracle_calls.items() if r.get("precision") == "exact"}
        stranded = len(exact_keys - set(subject_calls))
        if stranded > 0.01 * len(exact_keys):
            sys.exit(
                f"ORACLE/CORPUS MISMATCH: {stranded} of {len(exact_keys)} exact edges in\n"
                f"{oracle_json}\nhave no counterpart in the subject analysis of {path}.\n"
                "A pinned oracle is only a ruler for the tree it was measured on — "
                "re-derive\nit for this corpus, or point --path at the one it came from.")

    if args.save_oracle:
        target = Path(args.save_oracle).resolve()
        shutil.copyfile(oracle_json, target)
        oracle_sidecar(target).write_text(json.dumps({
            "date": datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
            "corpus": str(path),
            "tree_sha": git_sha(path),
            "ra": ra_version(),
            "exact_edges": exact_count,
            # Carried so a pinned run can still report how much of the tree the
            # oracle actually answered — its stderr is long gone by then.
            "oracle_coverage": oracle_coverage(oracle_stderr),
            "produced_by": binary_identity(args.mezz),
        }, indent=2, sort_keys=True))
        print(f"oracle saved: {target} ({exact_count} exact edges)", file=sys.stderr)

    if subject_unkeyed or oracle_unkeyed:
        print(f"note: {subject_unkeyed} subject / {oracle_unkeyed} oracle call edges "
              "carried no order metadata and were skipped", file=sys.stderr)

    stems = module_stems(subject_entities)
    buckets, out_of_scope, unpaired = score(subject_entities, subject_calls,
                                            oracle_calls, stems)

    meta = {
        "date": datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
        "path": str(path),
        "tree_sha": git_sha(path),
        **binary_identity(args.mezz),
        "ra": ra_version(),
        "subject_secs": subject_secs,
        "oracle_secs": oracle_secs,
        "exact_edges": exact_count,
        # Coverage describes the exact pass, so a pinned oracle inherits it from
        # the sidecar rather than from this run's (silent) stderr.
        "oracle_coverage": (oracle_meta or {}).get("oracle_coverage") if pinned
                           else oracle_coverage(oracle_stderr),
        "oracle_origin": oracle_origin,
        # Whether these numbers rest on a ruler this run invented. Inherited
        # from `recorded` under --reuse, deliberately: re-scoring a saved pair
        # does not make an oracle that was derived fresh any more comparable.
        "oracle_fresh": not args.reuse and not pinned,
        "oracle_meta": oracle_meta,
    }
    if recorded:
        # Reuse: the numbers describe the recorded run, so its provenance wins.
        meta.update({k: v for k, v in recorded.items() if k != "date"})
        meta["reused_from"] = str(workdir)
        # A meta.json written before binaries were identified by hash cannot
        # say what produced these analyses. Say that, rather than let the
        # binary named on *this* command line be read as the one that ran.
        if "mezz_build" not in recorded:
            meta["mezz_build"] = "unrecorded (pre-hash run)"
            meta["mezz_mtime"] = "unknown"
    elif args.reuse:
        # Same trap, one step further out: a saved pair with no meta.json at all
        # records nothing about what produced it, and the binary named on *this*
        # command line certainly did not.
        meta["mezz_build"] = "unrecorded (no meta.json beside the analyses)"
        meta["mezz_mtime"] = "unknown"
    else:
        (workdir / "meta.json").write_text(json.dumps(meta, indent=2, sort_keys=True))
    report = render(args, meta, buckets, out_of_scope, unpaired, subject_entities)

    if args.out:
        Path(args.out).write_text(report)
        print(f"wrote {args.out}", file=sys.stderr)
    else:
        print(report)

    if args.json_out:
        Path(args.json_out).write_text(json.dumps({
            "meta": meta,
            "counts": {k: len(v) for k, v in buckets.items()},
            "out_of_scope": out_of_scope,
            "unpaired": unpaired,
            "records": buckets,
        }, indent=2, sort_keys=True))

    if args.keep_json and not args.reuse:
        print(f"raw analyses kept: {workdir}", file=sys.stderr)
    elif not args.reuse:
        shutil.rmtree(workdir, ignore_errors=True)


if __name__ == "__main__":
    main()
