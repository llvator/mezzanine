# tree-sitter-dart, regenerated at grammar ABI 14

This is [nielsenko/tree-sitter-dart](https://github.com/nielsenko/tree-sitter-dart)
at commit `b57d734c84f510bbd524097902cab671e4dbfca9` (published to crates.io
as `tree-sitter-dart` 0.2.0), with `src/parser.c` regenerated to target
**grammar ABI 14** instead of the 15 the published crate ships.

**Do not edit `src/parser.c`, `src/grammar.json` or `src/node-types.json`** —
they are generated. `grammar.js` and `src/scanner.c` are the upstream sources.

## Why it is vendored

Mezzanine pins `tree-sitter` at 0.22, which loads grammar ABI 13–14 and
rejects 15 outright:

```
Incompatible language version 15. Expected minimum 13, maximum 14
```

Every `tree-sitter-dart` release carrying Dart 3 support is built at ABI 15,
and the only published ABI-14 release (0.0.4) predates Dart 3 — it cannot
read `sealed class`, record types, patterns or extension types, and loses the
whole declaration rather than degrading.

Moving Mezzanine to a newer `tree-sitter` would fix that, and it is the right
eventual move, but it cannot be done one grammar at a time: two `tree-sitter`
versions in one binary means two copies of the same `ts_*` C symbols. Moving
all of them together forces `tree-sitter-kotlin` (capped at `<0.23`) onto
`tree-sitter-kotlin-ng`, a different grammar in which 7 of the 76 node kinds
`src/parser/kotlin/` is built on no longer exist — including
`simple_identifier` and `type_identifier`, which every name in that parser
goes through. See DA-002.

Regenerating one grammar at the ABI we already load costs 8 MB of generated C
and nothing else. That is the same size the published crate compiles anyway,
and smaller than the Kotlin grammar already in the build.

## Regenerating

Needed when upgrading to a newer upstream commit — not otherwise.

```sh
cd vendor/tree-sitter-dart
npx --yes tree-sitter-cli@0.25.10 generate --abi 14
```

That rewrites `src/parser.c`, `src/grammar.json` and `src/node-types.json`
from `grammar.js`. Confirm the result still targets ABI 14 before committing:

```sh
grep -m1 LANGUAGE_VERSION src/parser.c        # -> #define LANGUAGE_VERSION 14
```

`build.rs` folds `src/parser.c` and `src/scanner.c` into the parse-store
fingerprint, so a regeneration invalidates cached parses automatically. It
does not fold in `grammar.js`: that file is an input to generation, not to
the build, and hashing it would leave a comment-only edit looking like a
grammar change.

## Licence

MIT, retained in `LICENSE`. Compatible with Mezzanine's AGPL-3.0-only.
