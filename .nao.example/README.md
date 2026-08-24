# `.nao/` — the repo's own settings

A template. Copy it and drop the `.example`:

```sh
cp -R .nao.example .nao
```

Then edit `.nao/settings.json` and commit it. It describes *this repo* — which
languages it contains, what to skip, which port to serve on — so everyone who
clones gets the same analysis without being told to pass flags.

## Two files, and the difference matters

| File | Describes | Commit it? |
|---|---|---|
| `<repo>/.nao/settings.json` | the **repo** — languages, exclusions, port | **yes** |
| `~/.config/nao/settings.json` | your **installation** — paths on your machine | no, it's outside the repo |

Resolution, first hit wins:

```
CLI flag  >  env var  >  <repo>/.nao/settings.json  >  ~/.config/nao/settings.json  >  defaults
```

Each key is valid in one scope, the other, or both. Put a key in the wrong
file and nao names it back to you rather than ignoring it.

**Repo scope only** — `output_dir`, `spec_dir`.

**User scope only** — `ui_dir`, `content_fallback`. Both are absolute paths to
somewhere on one machine; in a repo file they would be a path everyone else
has to have too. You usually need neither: `nao watch` finds the browser UI
beside the installed binary on its own.

**Either scope** — `language`, `kind`, `include_tests`, `include_external`,
`max_depth`, `min_weight`, `port`, `debounce_ms`, `exclude_patterns`,
`include_patterns`.

## Keep it portable

The file is committed, so it is read by machines that are not yours.

- **No absolute paths.** `output_dir` is repo-relative — `.nao/data`, not
  `/absolute/path/to/project/.nao/data`. An absolute path sends every clone's
  output to a directory that exists on one laptop.
- **No home directories, usernames, or anything outside the repo.**
- Add `.nao/data/` to your `.gitignore`. It is generated on every analysis.

`spec_dir` is held to that rule by nao rather than by convention: an absolute
path, or one that climbs out with `..`, is refused and named back to you. It
decides which directories nao *reads*, and a cloned file does not get to pick
those. When the spec genuinely lives outside the repo, an operator says so —
`nao watch . --spec-dir ../docs/domain`, or the **Spec folder** field in the
browser UI.

## Patterns extend, they do not replace

`exclude_patterns` and `include_patterns` are **added** to the built-in
defaults, which already skip `node_modules`, `target`, `.git`, `vendor`,
`__pycache__`, `dist` and `build`. Listing one extra rule will not silently
re-enable scanning `node_modules`.

## What a pattern is matched against

The path **relative to the repo root** — the directory this file lives under.
Write what you would read off your editor:

```json
{ "exclude_patterns": ["src/contracts.d.ts", "src/generated/**"] }
```

Both work identically for `nao analyze .`, `nao analyze src` and
`nao analyze /path/to/repo/src`. Point nao at a directory that is not in a
checkout and that directory stands in for the repo root.

Two things worth knowing:

- **`*` crosses `/`.** `*.d.ts` matches `src/a.d.ts`, not just `a.d.ts`, which
  makes the leading `**/` in the defaults decorative. Surprising, and kept:
  every pattern written against nao so far relies on it.
- **A pattern that matched nothing says so**, on stderr, naming the key and
  the spelling — which is how you tell a rule that is working from a typo,
  since both produce the same graph.

## Four keys a settings file may never set

`allow_agent_spawn`, `no_token`, `allow_origin`, `allow_unsafe_passes`.

They are refused at **both** scopes, and nao says so when it sees one. A repo
file is content you cloned from a stranger, and each of these grants
something a file should not be able to grant on its own — a terminal on your
machine, the pairing-token requirement dropped, another browser origin
allowed to read your source. Pass them as flags, where a human is deciding.

## Format

Plain JSON: **no comments, no trailing commas.** A parse error costs you the
whole file, not just the broken line — nao warns and falls back to defaults.
An unrecognised key is reported by name rather than dropped in silence.

See [guide/getting-started.md](../guide/getting-started.md) for the full key
list and what each one does.
