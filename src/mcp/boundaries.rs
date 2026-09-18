//! `boundaries` — the dependencies this folder's files make that reach
//! past another folder's door.
//!
//! Every other tool here grades a folder on what *arrives*.
//! `entry_concentration` and the `Entry` gate ask whether outsiders come
//! through one door; `OutsideVerdict::Breach` names the ones that do not.
//! `egress` (ADR 0031) grades where an exit *starts*. And `reshape` closes
//! its boundary section by telling the reader, of its outgoing
//! dependencies:
//!
//! > Where they land is not graded and stays their own folder's business —
//! > depending outward is what a folder is for.
//!
//! That sentence is true about the *target* and it leaves a hole, because
//! every folder says it. A file deep in `src/a` importing a file deep in
//! `src/x` is a defect nobody owns: `src/x` sees a breach it did not
//! cause and cannot fix, and `src/a` is told the landing site is none of
//! its concern. The tangle accumulates in the gap.
//!
//! This tool takes the other half. **Where an import lands is the target's
//! business; whether you knocked on the front door is yours.** It is asked
//! of the folder doing the importing, because that is the folder whose
//! files have to change.
//!
//! ## Why the three-way split matters
//!
//! Measured over this repo, 48% of cross-folder dependencies land
//! somewhere other than a door — which as a work list is useless, because
//! most of them are fine. Splitting off the files that several folders
//! reach leaves a very different picture:
//!
//! | | share |
//! | --- | ---: |
//! | lands on the door of every folder it crosses | 52% |
//! | lands on a shared contract | 35% |
//! | reaches past a door | **13%** |
//!
//! The middle bucket is what a vocabulary folder looks like — `src/models`
//! here — and `reshape` already tells agents in prose that a bag several
//! callers share is honest to leave alone. Counting it as a breach would
//! bury the 13% that is real under three times its own weight in noise,
//! and an agent who is told to fix 192 things fixes none of them. Fifty-two
//! imports across twenty-three folder pairs is a list somebody finishes.
//!
//! ## A file is a subject here, a folder is not the only one
//!
//! `reshape` and `layout` refuse a file because shape is a property of a
//! folder's children and a file has none. That rationale is theirs and
//! does not carry (MCP-040): every import in a file either lands on
//! another folder's door or reaches past it, and the file is where the
//! offending import is actually written — which is why this tool already
//! reports its line number. So [`Subject`] is a folder *or* a file, and
//! the two differ in one thing only: which of the folder's files the
//! imports counted were written in. The boundary itself is the holding
//! folder's either way, because a file has no door of its own and an
//! import that stays inside its folder crosses nothing.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use anyhow::{bail, Result};
use serde_json::Value;

use crate::analyzer::folder_shape;
use crate::graph::FileGraph;

use super::format::listed;
use super::reshape::rel;
use super::tools::cap_lines;
use super::McpServer;

/// How many distinct folders have to reach a file before it counts as
/// shared vocabulary rather than as somebody's interior.
///
/// Three, not two. Two folders sharing a file is the commonest shape of a
/// genuine leak — one of them owns it and the other reached in — and
/// excusing that would excuse most of what this tool exists to find. At
/// three the reading changes: a file that `mcp`, `analyzer` and `output`
/// all depend on is not `mcp`'s private business under any arrangement,
/// and telling anyone to route around it would be asking for the wrapper
/// on `reshape`'s forbidden list.
const CONTRACT_REACHERS: usize = 3;

/// `boundaries` — what this folder's (or this file's) own imports do to
/// everyone else's structure.
pub fn boundaries(server: &McpServer, args: &Value) -> Result<String> {
    let graph = super::tools::analyze(server, &server.root)?;
    let fg = graph.file_graph();
    let subject = Subject::of(server, args, &fg)?;

    let map = Boundary::of(&fg, &subject);
    let root = &server.root;
    let ctx = Context {
        used: names_used(&graph, &subject),
        sites: graph
            .import_sites()
            .iter()
            .map(|s| ((path_of(&s.from), path_of(&s.to)), s.line + 1))
            .collect(),
        subject: &subject,
        root,
    };

    let mut body = vec![
        format!("# Boundaries of {}", subject.heading(root)),
        String::new(),
    ];
    body.extend(tally(&map, &subject, root));
    body.extend(reaching_section(&map, &ctx));
    body.extend(contracts_section(&map, root));
    body.extend(fixes_section(&map));
    Ok(cap_lines(body, subject.shorten_hint()))
}

/// What the tool was asked about: a folder, or one file in it.
///
/// Both are graded against the same boundary — the doors of the folders
/// they reach into, and what counts as being outside their own — and
/// differ only in which imports are the subject's to answer for.
enum Subject {
    /// Every import written anywhere under this folder.
    Folder(String),
    /// Only the imports written in this file, against the boundary of the
    /// folder holding it.
    File { path: String, folder: String },
}

impl Subject {
    /// Resolve `path` to a folder or a file the graph actually holds.
    fn of(server: &McpServer, args: &Value, fg: &FileGraph) -> Result<Subject> {
        let resolved = super::reshape::target_path(server, args);
        let absolute = resolved.display().to_string();
        let named = rel(&resolved, &server.root);
        if !resolved.is_file() {
            if !fg.folders.contains(&absolute) {
                bail!(
                    "No analysed folder at {named}. Folders come from the files in \
                     scope, so a directory holding nothing mezz parsed has no boundary \
                     to cross."
                );
            }
            return Ok(Subject::Folder(absolute));
        }
        if !fg.files.contains(&absolute) {
            bail!(
                "{named} is not in the analysed graph — nothing mezz parsed, or nothing \
                 in scope. A file it cannot read writes no imports it can grade."
            );
        }
        let folder = parent_dir(&absolute).unwrap_or_default().to_string();
        Ok(Subject::File {
            path: absolute,
            folder,
        })
    }

    /// The folder whose doors and interior define the boundary. For a file
    /// that is the folder holding it: a file has no door of its own, and
    /// an import that stays inside its folder crosses nothing.
    fn folder(&self) -> &str {
        match self {
            Subject::Folder(f) => f,
            Subject::File { folder, .. } => folder,
        }
    }

    /// Whether an import written in `file` is this subject's to answer for.
    fn wrote(&self, file: &str) -> bool {
        match self {
            Subject::Folder(f) => is_inside(f, file),
            Subject::File { path, .. } => path == file,
        }
    }

    /// The whole project, which has nothing outside it to cross into.
    fn is_analysis_root(&self, root: &Path) -> bool {
        matches!(self, Subject::Folder(f) if Path::new(f) == root)
    }

    fn heading(&self, root: &Path) -> String {
        let named = rel(Path::new(self.path()), root);
        // `rel` of the root against the root is the empty string, and a
        // heading that trails off is worse than one that names the case.
        if named.is_empty() {
            return "the analysis root".to_string();
        }
        named
    }

    fn path(&self) -> &str {
        match self {
            Subject::Folder(f) => f,
            Subject::File { path, .. } => path,
        }
    }

    /// The files on this side of one boundary, named so the sentence
    /// agrees: the phrase, and whether it is plural. A file subject has
    /// exactly one file and *is* that file, so "one of your files" would
    /// read as if the report covered something wider than it does.
    fn ours(&self, count: usize) -> (String, bool) {
        match self {
            Subject::File { .. } => ("This file".to_string(), false),
            Subject::Folder(_) if count == 1 => ("One of your files".to_string(), false),
            Subject::Folder(_) => (format!("{count} of your files"), true),
        }
    }

    /// What to narrow to when the report runs past the line cap. A file
    /// is already the narrowest subject there is, so it is sent to the
    /// work instead.
    fn shorten_hint(&self) -> &'static str {
        match self {
            Subject::Folder(_) => "Call `boundaries` on a subfolder, or on one file, for a shorter list.",
            Subject::File { .. } => "Fix the first boundary above and re-run — this is already one file's list.",
        }
    }
}

/// `&Path` as the string spelling the graph keys everything by.
fn path_of(p: &Path) -> &str {
    p.to_str().unwrap_or_default()
}

/// The named things this folder's files actually use, per target file.
///
/// File-level pairs say *that* a boundary is crossed; only the entity
/// edges say **what for**, and that is the whole of the difference between
/// "your callers want two named things" and "your callers want the module".
/// The first is a facade the door can own in an afternoon; the second is a
/// folder with no surface, and telling anyone to route it through a door
/// would be asking for a facade in name only.
fn names_used<'a>(
    graph: &'a crate::graph::DependencyGraph,
    subject: &Subject,
) -> HashMap<&'a str, BTreeSet<&'a str>> {
    let file_of: HashMap<&str, &str> = graph
        .entities()
        .map(|e| (e.id.as_str(), e.file_path.to_str().unwrap_or_default()))
        .collect();
    let mut out: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for rel in graph.relationships() {
        if !rel.kind.is_dependency() {
            continue;
        }
        let (Some(from), Some(to)) = (
            file_of.get(rel.source_id.as_str()),
            file_of.get(rel.target_id.as_str()),
        ) else {
            continue;
        };
        if !subject.wrote(from) || is_inside(subject.folder(), to) || to.is_empty() {
            continue;
        }
        if let Some(name) = graph.get_entity(&rel.target_id).map(|e| e.name.as_str()) {
            out.entry(to).or_default().insert(name);
        }
    }
    out
}

// ------------------------------------------------------------------
//  The reading
// ------------------------------------------------------------------

/// One dependency leaving the folder, and what it did on the way in.
struct Crossing {
    from: String,
    to: String,
    /// The shallowest folder whose door this bypassed. `None` when it
    /// bypassed none.
    bypassed: Option<String>,
    /// How many distinct folders reach `to`. Three or more and it is
    /// vocabulary rather than interior — see [`CONTRACT_REACHERS`].
    reachers: usize,
}

impl Crossing {
    fn verdict(&self) -> Verdict {
        match &self.bypassed {
            None => Verdict::Door,
            Some(_) if self.reachers >= CONTRACT_REACHERS => Verdict::Contract,
            Some(_) => Verdict::Reaches,
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    /// Landed on the door of every folder it crossed.
    Door,
    /// Landed past a door, on a file several folders reach.
    Contract,
    /// Landed past a door, in what is somebody's interior.
    Reaches,
}

/// Every dependency this folder's files make on code outside it.
struct Boundary {
    crossings: Vec<Crossing>,
    /// Folder → the file outsiders are supposed to come through, for the
    /// folders this one bypassed. Rendered so a reader can act without a
    /// second call.
    doors: HashMap<String, Vec<String>>,
}

impl Boundary {
    fn of(fg: &FileGraph, subject: &Subject) -> Boundary {
        let doors = folder_shape::doors_by_folder(&fg.pairs, &fg.folders);
        let reachers = reachers_of(fg);
        let folder = subject.folder();
        let mut seen: HashSet<(&str, &str)> = HashSet::new();
        let mut crossings = Vec::new();
        for (from, to) in &fg.pairs {
            let leaving = subject.wrote(from) && !is_inside(folder, to) && to != folder;
            if !leaving || !seen.insert((from, to)) {
                continue;
            }
            crossings.push(Crossing {
                bypassed: bypassed_door(from, to, &doors),
                reachers: reachers.get(to.as_str()).map_or(0, HashSet::len),
                from: from.clone(),
                to: to.clone(),
            });
        }
        crossings.sort_by(|a, b| (&a.to, &a.from).cmp(&(&b.to, &b.from)));
        Boundary { crossings, doors }
    }

    fn with(&self, verdict: Verdict) -> Vec<&Crossing> {
        self.crossings
            .iter()
            .filter(|c| c.verdict() == verdict)
            .collect()
    }
}

/// Every file, and which folders reach it from outside themselves.
///
/// Folders rather than files, because the question is how many *parts of
/// the system* treat this file as theirs. Ten call sites in one folder is
/// one folder's business; one call site each from three folders is a
/// contract.
fn reachers_of(fg: &FileGraph) -> HashMap<&str, HashSet<&str>> {
    let mut out: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (from, to) in &fg.pairs {
        let (Some(a), Some(b)) = (parent_dir(from), parent_dir(to)) else {
            continue;
        };
        if a != b {
            out.entry(to.as_str()).or_default().insert(a);
        }
    }
    out
}

/// The shallowest folder on the way in whose door this dependency did not
/// land on, or `None` when it landed on every one of them.
///
/// Shallowest because that is the boundary the fix belongs at. An import
/// that bypasses `src/parser`'s door and then also `src/parser/rust`'s is
/// one problem, and it is `src/parser`'s door that was supposed to stop
/// it; naming the deeper one would send an agent to route through a folder
/// it should not have been able to see.
fn bypassed_door(from: &str, to: &str, doors: &HashMap<String, Vec<String>>) -> Option<String> {
    crossed_folders(from, to)
        .into_iter()
        .find(|folder| !doors.get(*folder).is_some_and(|d| d.iter().any(|x| x == to)))
        .map(str::to_string)
}

/// The folders this dependency enters on the target side: everything below
/// the point where the two files part company, outermost first.
///
/// The same walk `folder_shape::accumulate` makes when it decides which
/// folders an edge crosses the boundary of — so a folder this names is a
/// folder that already counted the edge as an arrival, and the two
/// readings of one import agree.
fn crossed_folders<'a>(from: &'a str, to: &'a str) -> Vec<&'a str> {
    let (_, theirs, shared) = parting(from, to);
    theirs.into_iter().skip(shared).collect()
}

/// Where two files part company: their ancestor chains and how much of the
/// front of them is shared.
///
/// A local copy of `folder_shape::parting`, which is private and sits in a
/// file under heavy concurrent edit. Kept deliberately identical, and
/// tested against the same boundary case, so the two cannot read one
/// import differently — see the note in the module header of
/// `analyzer::folder_shape` about one definition of the subtraction.
fn parting<'a>(src: &'a str, tgt: &'a str) -> (Vec<&'a str>, Vec<&'a str>, usize) {
    let from = ancestors(parent_dir(src).unwrap_or_default());
    let to = ancestors(parent_dir(tgt).unwrap_or_default());
    let shared = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    (from, to, shared)
}

/// Every folder on the way down to `dir`, outermost first, `dir` last.
fn ancestors(dir: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut cur = dir;
    while !cur.is_empty() {
        out.push(cur);
        cur = parent_dir(cur).unwrap_or_default();
    }
    out.reverse();
    out
}

/// The folder holding `path`, or `None` for a path with no separator.
fn parent_dir(path: &str) -> Option<&str> {
    let cut = path.rfind(std::path::MAIN_SEPARATOR)?;
    Some(&path[..cut])
}

/// Whether `path` sits anywhere under `folder`, compared on a separator
/// boundary so `src/uid` does not claim `src/uidx`.
fn is_inside(folder: &str, path: &str) -> bool {
    path.strip_prefix(folder)
        .is_some_and(|tail| tail.starts_with(std::path::MAIN_SEPARATOR))
}

// ------------------------------------------------------------------
//  Rendering
// ------------------------------------------------------------------

fn tally(map: &Boundary, subject: &Subject, root: &Path) -> Vec<String> {
    let total = map.crossings.len();
    if total == 0 {
        return vec![nothing_crosses(subject, root), String::new()];
    }
    vec![
        opening(subject, total, root),
        String::new(),
        format!(
            "- **{}** land on the door of every folder they cross.",
            map.with(Verdict::Door).len()
        ),
        format!(
            "- **{}** land on a file {CONTRACT_REACHERS} or more folders reach — \
             shared vocabulary, and honest to leave alone.",
            map.with(Verdict::Contract).len()
        ),
        format!(
            "- **{}** reach past a door, into what is one folder's interior.",
            map.with(Verdict::Reaches).len()
        ),
        String::new(),
    ]
}

/// The opening sentence, in the subject's own grammar: a folder answers
/// for what its files do, a file answers for itself.
fn opening(subject: &Subject, total: usize, root: &Path) -> String {
    let plural = if total == 1 {
        "dependency"
    } else {
        "dependencies"
    };
    match subject {
        Subject::Folder(_) => format!(
            "Its files make {total} {plural} on code outside it. Where those land is \
             the target folder's business; whether they knocked on its front door is \
             this folder's."
        ),
        Subject::File { folder, .. } => format!(
            "It makes {total} {plural} on code outside `{}`, the folder holding it — \
             the boundary is that folder's, since a file has no door of its own. Where \
             those land is the target folder's business; whether they knocked on its \
             front door is this file's, and each one is an import line in it.",
            rel(Path::new(folder), root),
        ),
    }
}

/// Nothing left the boundary at all, which means three different things
/// depending on what was asked.
fn nothing_crosses(subject: &Subject, root: &Path) -> String {
    if subject.is_analysis_root(root) {
        return "The analysis root has nothing outside it, so every dependency it makes \
                is internal by construction and there is no boundary here to cross. \
                Call `boundaries` on a subfolder — the tangle this finds lives between \
                siblings, not above them."
            .to_string();
    }
    match subject {
        Subject::Folder(_) => "This folder's files depend on nothing outside it. There \
                               is no boundary behaviour to report, which is the most \
                               self-contained a folder gets."
            .to_string(),
        Subject::File { folder, .. } => format!(
            "This file depends on nothing outside `{}`, the folder holding it. Every \
             import it writes stays inside, so it crosses no boundary and there is \
             nothing here to grade.",
            rel(Path::new(folder), root),
        ),
    }
}

/// Everything the work list needs besides the boundary map: what was
/// asked about, where each import is written, and what it reaches for.
struct Context<'a> {
    subject: &'a Subject,
    /// `(source file, target file)` → the 1-based line the import is on.
    sites: HashMap<(&'a str, &'a str), usize>,
    /// Target file → the named things this side actually uses from it.
    used: HashMap<&'a str, BTreeSet<&'a str>>,
    root: &'a Path,
}

/// The work list: one heading per folder reached into, so the fix is
/// read per boundary rather than per import.
fn reaching_section(map: &Boundary, ctx: &Context) -> Vec<String> {
    let reaching = map.with(Verdict::Reaches);
    // Nothing crossed the boundary at all: the tally above has already said
    // so, and a second sentence congratulating the folder on discipline it
    // never had to exercise is noise.
    if map.crossings.is_empty() {
        return Vec::new();
    }
    if reaching.is_empty() {
        return vec![
            format!(
                "**Nothing here reaches past a door.** Every dependency {} makes \
                 either lands on a door or lands on shared vocabulary, which is \
                 the boundary discipline holding.",
                match ctx.subject {
                    Subject::Folder(_) => "this folder",
                    Subject::File { .. } => "this file",
                }
            ),
            String::new(),
        ];
    }
    let mut by_folder: BTreeMap<&str, Vec<&Crossing>> = BTreeMap::new();
    for c in reaching {
        if let Some(f) = c.bypassed.as_deref() {
            by_folder.entry(f).or_default().push(c);
        }
    }
    let imports: usize = by_folder.values().map(Vec::len).sum();
    let mut body = vec![
        format!(
            "## Reaching past a door — {} {} across {} {}",
            imports,
            if imports == 1 { "import" } else { "imports" },
            by_folder.len(),
            if by_folder.len() == 1 {
                "boundary"
            } else {
                "boundaries"
            },
        ),
        String::new(),
    ];
    for (folder, crossings) in by_folder {
        body.extend(one_boundary(folder, &crossings, map, ctx));
    }
    body
}

fn one_boundary(
    folder: &str,
    crossings: &[&Crossing],
    map: &Boundary,
    ctx: &Context,
) -> Vec<String> {
    let root = ctx.root;
    let doors: Vec<String> = map
        .doors
        .get(folder)
        .map(|d| d.iter().map(|x| format!("`{}`", rel(Path::new(x), root))).collect())
        .unwrap_or_default();
    let mut body = vec![
        format!(
            "**`{}`** — {}",
            rel(Path::new(folder), root),
            match doors.len() {
                0 => "nothing outside it depends on any one file more than another, \
                      so it has no door to come through yet"
                    .to_string(),
                1 => format!("its door is {}", doors[0]),
                _ => format!(
                    "it has {} doors ({}), so there is no single one to route \
                     through — run `reshape` on it first",
                    doors.len(),
                    doors.join(", ")
                ),
            }
        ),
        String::new(),
    ];
    body.extend(listed(crossings.iter().map(|c| {
        let at = ctx
            .sites
            .get(&(c.from.as_str(), c.to.as_str()))
            .map(|line| format!(":{line}"))
            .unwrap_or_default();
        format!(
            "`{}{}` → `{}`",
            rel(Path::new(&c.from), root),
            at,
            rel(Path::new(&c.to), root)
        )
    })));
    body.push(String::new());
    body.push(prescribe(
        &Evidence::of(crossings, map.doors.get(folder), &ctx.used),
        ctx.subject,
    ));
    body.push(String::new());
    body
}

/// What one boundary looks like, in the terms the fix is chosen by.
struct Evidence {
    /// Files in this folder that reach across.
    mine: usize,
    /// Files on the other side they land on.
    theirs: usize,
    /// Named things actually used across the boundary. The number that
    /// separates a facade from a folder with no surface.
    names: BTreeSet<String>,
    /// How many doors the target has. Not one, and there is nothing to
    /// route through.
    doors: usize,
    /// The fewest folders reaching any one file here. Two is one short of
    /// vocabulary, and wants moving rather than routing.
    fewest_reachers: usize,
}

impl Evidence {
    fn of(
        crossings: &[&Crossing],
        doors: Option<&Vec<String>>,
        used: &HashMap<&str, BTreeSet<&str>>,
    ) -> Evidence {
        let theirs: BTreeSet<&str> = crossings.iter().map(|c| c.to.as_str()).collect();
        Evidence {
            mine: crossings
                .iter()
                .map(|c| c.from.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            names: theirs
                .iter()
                .filter_map(|t| used.get(t))
                .flatten()
                .map(|n| (*n).to_string())
                .collect(),
            theirs: theirs.len(),
            doors: doors.map_or(0, Vec::len),
            fewest_reachers: crossings.iter().map(|c| c.reachers).min().unwrap_or(0),
        }
    }
}

/// How many files on the other side stop this being one boundary to cross
/// and start it being a folder with no surface.
const DISSOLVED: usize = 3;

/// How many named things a door can plausibly be given at once. Past this
/// the "facade" is the module again.
const FACADE_NAMES: usize = 4;

/// The move to make here, chosen from the evidence rather than left as
/// four options for the reader to pick between.
///
/// Ordered by what blocks what. A folder with several doors cannot be
/// routed through at all, so that is asked first; a file two folders reach
/// wants moving rather than routing, so it is asked before anything about
/// facades; and a boundary crossed at three files is not one door being
/// gone round, so it is separated from the case a door can actually
/// absorb. The last two differ only in how much of the fix is yours.
fn prescribe(e: &Evidence, subject: &Subject) -> String {
    let (ours, many) = subject.ours(e.mine);
    if e.doors != 1 {
        return format!(
            "→ **Blocked on the other side.** That folder has {}, so there is no \
             single entrance to route through. Run `reshape` on it first — a folder \
             cannot be given a surface from the outside.",
            match e.doors {
                0 => "no door yet — nothing depends on any one of its files more than another".to_string(),
                n => format!("{n} doors"),
            }
        );
    }
    if e.fewest_reachers >= 2 {
        return "→ **Move it, do not route it.** Another folder reaches this too, which \
                puts it one caller short of being shared vocabulary. If you both need \
                it for the same reason, it belongs somewhere you can both see and the \
                boundary disappears; if for different reasons, it is two things in one \
                file and wants splitting. Routing it through a door makes it that \
                folder's property, which it is not."
            .to_string();
    }
    if e.theirs >= DISSOLVED {
        return format!(
            "→ **Reshape the other folder first.** {ours} {} {} different files \
             inside it for {} named things. That is not one door being gone round, it \
             is a folder with no surface — and pushing {} things through one file \
             would be a facade in name only. `reshape` and `layout` on that folder are \
             the work; come back here after.",
            if many { "reach" } else { "reaches" },
            e.theirs,
            e.names.len(),
            e.names.len(),
        );
    }
    if e.names.len() <= FACADE_NAMES {
        return format!(
            "→ **The door should own {}.** {ours} {} the same small set — a facade that \
             genuinely owns the concept, not a re-export of the file behind it. If you \
             are its only caller anywhere, moving it into this folder is the other \
             honest answer, and often the better one.",
            names_phrase(&e.names),
            if many { "want" } else { "wants" },
        );
    }
    format!(
        "→ **Too wide for one facade, too narrow to reshape.** {} named things across \
         {} {}. Split it: the two or three you depend on most belong at that door, \
         and the rest are probably a dependency that belongs further down your own \
         tree, next to the caller that actually needs it.",
        e.names.len(),
        e.theirs,
        if e.theirs == 1 { "file" } else { "files" },
    )
}

/// The names themselves when there are few enough to read, a count when
/// there are not.
fn names_phrase(names: &BTreeSet<String>) -> String {
    if names.len() > FACADE_NAMES {
        return format!("those {} names", names.len());
    }
    let quoted: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, front)) => format!("{} and {last}", front.join(", ")),
        None => "what you use".to_string(),
    }
}

/// The bucket that is not a work list, said out loud so nobody treats it
/// as one.
fn contracts_section(map: &Boundary, root: &Path) -> Vec<String> {
    let contracts = map.with(Verdict::Contract);
    if contracts.is_empty() {
        return Vec::new();
    }
    let files: BTreeSet<String> = contracts
        .iter()
        .map(|c| {
            format!(
                "`{}` — {} folders reach it",
                rel(Path::new(&c.to), root),
                c.reachers
            )
        })
        .collect();
    let mut body = vec![
        format!("## Shared vocabulary ({}) — leave these alone", files.len()),
        String::new(),
        format!(
            "Each is reached by {CONTRACT_REACHERS} or more folders, so it is not any \
             one folder's interior and routing around it would add a wrapper without \
             changing who depends on what. This section is here so it is not mistaken \
             for the list above."
        ),
        String::new(),
    ];
    body.extend(listed(files));
    body.push(String::new());
    body
}

fn fixes_section(map: &Boundary) -> Vec<String> {
    if map.with(Verdict::Reaches).is_empty() {
        return Vec::new();
    }
    vec![
        "## What counts as a fix".to_string(),
        String::new(),
        "Each of those imports is one of four things, and only the first two are \
         work on this side of the boundary:"
            .to_string(),
        String::new(),
        "1. **The door already offers what you need.** Depend on it instead. This is \
         the common case and the cheap one."
            .to_string(),
        "2. **The door does not offer it, and should.** The target folder is hiding \
         something its callers legitimately need behind a file that is not its \
         entrance. Give the door the concept — not a re-export of the file. If the \
         thing behind the door still changes when the door does, it is not a door."
            .to_string(),
        "3. **You should not need it at all.** A dependency that reaches into \
         somebody's interior is often one that belongs further down your own tree, \
         where the caller that actually needs it lives."
            .to_string(),
        "4. **It is vocabulary that has not spread yet.** A type two folders share \
         is one folder short of the section above. Moving it somewhere both can see \
         is the fix, and routing it through a door is not."
            .to_string(),
        String::new(),
        "The test is the same one `reshape` applies: after the change, does anything \
         depend on a *different* thing than it did before? Re-exporting the same file \
         from the door moves this count to zero and changes nothing, which makes the \
         count a lie rather than a fix."
            .to_string(),
        String::new(),
        "Re-run `boundaries` here when you are done, and `reshape` on any folder \
         above whose door you had to widen."
            .to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> String {
        s.replace('/', std::path::MAIN_SEPARATOR_STR)
    }

    fn folder(s: &str) -> Subject {
        Subject::Folder(p(s))
    }

    /// The file subject `Subject::of` builds for a path that is a file:
    /// the boundary is the holding folder's.
    fn file(s: &str) -> Subject {
        let path = p(s);
        let folder = parent_dir(&path).unwrap_or_default().to_string();
        Subject::File { path, folder }
    }

    fn graph(pairs: &[(&str, &str)]) -> FileGraph {
        let pairs: Vec<(String, String)> =
            pairs.iter().map(|(a, b)| (p(a), p(b))).collect();
        let mut files: BTreeSet<String> = BTreeSet::new();
        for (a, b) in &pairs {
            files.insert(a.clone());
            files.insert(b.clone());
        }
        let mut folders: HashSet<String> = HashSet::new();
        for f in &files {
            let mut cur = parent_dir(f);
            while let Some(dir) = cur {
                folders.insert(dir.to_string());
                cur = parent_dir(dir);
            }
        }
        FileGraph {
            files: files.into_iter().collect(),
            pairs,
            folders,
            declaration_only: HashSet::new(),
            imports: Vec::new(),
        }
    }

    /// The case the tool exists for: a file reaching past another folder's
    /// door into its interior, where neither folder's own report owns it.
    #[test]
    fn an_import_past_a_door_is_named_against_the_folder_that_wrote_it() {
        // `x/door.rs` is what everyone else depends on, so it is x's door;
        // `a/one.rs` goes round it to `x/inner.rs`.
        let fg = graph(&[
            ("src/b/uses.rs", "src/x/door.rs"),
            ("src/c/uses.rs", "src/x/door.rs"),
            ("src/a/one.rs", "src/x/inner.rs"),
        ]);
        let map = Boundary::of(&fg, &folder("src/a"));
        assert_eq!(map.crossings.len(), 1);
        assert_eq!(map.crossings[0].verdict(), Verdict::Reaches);
        assert_eq!(map.crossings[0].bypassed.as_deref(), Some(p("src/x").as_str()));
    }

    /// Landing on the door is the discipline holding, and must not be
    /// reported as anything.
    #[test]
    fn an_import_onto_the_door_is_not_a_finding() {
        let fg = graph(&[
            ("src/a/one.rs", "src/x/door.rs"),
            ("src/b/two.rs", "src/x/door.rs"),
        ]);
        let map = Boundary::of(&fg, &folder("src/a"));
        assert_eq!(map.crossings[0].verdict(), Verdict::Door);
    }

    /// The bucket that stops the work list drowning: a file several
    /// folders reach is vocabulary, and `reshape` already tells agents a
    /// bag several callers share is honest to leave alone.
    #[test]
    fn a_file_three_folders_reach_is_vocabulary_not_a_breach() {
        let fg = graph(&[
            ("src/x/door.rs", "src/x/inner.rs"),
            ("src/a/one.rs", "src/models/kind.rs"),
            ("src/b/two.rs", "src/models/kind.rs"),
            ("src/c/three.rs", "src/models/kind.rs"),
            ("src/d/four.rs", "src/models/door.rs"),
            ("src/e/five.rs", "src/models/door.rs"),
            ("src/f/six.rs", "src/models/door.rs"),
            ("src/g/g.rs", "src/models/door.rs"),
        ]);
        let map = Boundary::of(&fg, &folder("src/a"));
        assert_eq!(map.crossings[0].verdict(), Verdict::Contract);
    }

    /// Two folders sharing a file is the commonest shape of a real leak,
    /// so the contract bar is three and not two.
    #[test]
    fn a_file_only_two_folders_reach_is_still_a_breach() {
        // `door.rs` has to out-poll `inner.rs` to be the door at all —
        // three reachers against two.
        let fg = graph(&[
            ("src/z/z.rs", "src/x/door.rs"),
            ("src/w/w.rs", "src/x/door.rs"),
            ("src/v/v.rs", "src/x/door.rs"),
            ("src/a/one.rs", "src/x/inner.rs"),
            ("src/b/two.rs", "src/x/inner.rs"),
        ]);
        let map = Boundary::of(&fg, &folder("src/a"));
        assert_eq!(map.crossings[0].verdict(), Verdict::Reaches);
    }

    /// The boundary a prefix test gets wrong.
    #[test]
    fn a_sibling_with_a_shared_prefix_is_outside_the_folder() {
        let fg = graph(&[("src/uid/a.rs", "src/uidx/b.rs")]);
        let map = Boundary::of(&fg, &folder("src/uid"));
        assert_eq!(map.crossings.len(), 1);
    }

    /// The fix belongs at the outermost boundary that should have stopped
    /// it, not the innermost one it ended up in.
    #[test]
    fn the_shallowest_bypassed_door_is_the_one_named() {
        let fg = graph(&[
            ("src/b/uses.rs", "src/x/door.rs"),
            ("src/c/uses.rs", "src/x/door.rs"),
            ("src/a/one.rs", "src/x/deep/inner.rs"),
        ]);
        let map = Boundary::of(&fg, &folder("src/a"));
        assert_eq!(map.crossings[0].bypassed.as_deref(), Some(p("src/x").as_str()));
    }

    // ---------------------------------------------------------------
    //  Asked about one file (MCP-040)
    // ---------------------------------------------------------------

    /// The narrowing, and the whole of it: the same boundary, graded over
    /// the imports written in one file rather than all of the folder's.
    #[test]
    fn a_file_is_graded_on_its_own_imports_and_not_its_neighbours() {
        // `door.rs` has to out-poll `inner.rs` to be the door at all —
        // three dependencies against the two written in `src/a`.
        let fg = graph(&[
            ("src/b/uses.rs", "src/x/door.rs"),
            ("src/c/uses.rs", "src/x/door.rs"),
            ("src/d/uses.rs", "src/x/door.rs"),
            ("src/a/one.rs", "src/x/inner.rs"),
            ("src/a/two.rs", "src/x/inner.rs"),
        ]);
        let whole = Boundary::of(&fg, &folder("src/a"));
        assert_eq!(whole.crossings.len(), 2);

        let just_one = Boundary::of(&fg, &file("src/a/one.rs"));
        assert_eq!(just_one.crossings.len(), 1);
        assert_eq!(just_one.crossings[0].from, p("src/a/one.rs"));
        // Same verdict, same bypassed door: only the set considered narrowed.
        assert_eq!(just_one.crossings[0].verdict(), Verdict::Reaches);
        assert_eq!(
            just_one.crossings[0].bypassed.as_deref(),
            Some(p("src/x").as_str())
        );
    }

    /// The doors are the enclosing folder's, not the file's. A file has
    /// no door of its own, and an import that stays inside the folder
    /// holding it crosses nothing to be graded on.
    #[test]
    fn an_import_inside_the_holding_folder_is_not_a_crossing() {
        let fg = graph(&[
            ("src/a/one.rs", "src/a/two.rs"),
            ("src/a/one.rs", "src/a/deep/three.rs"),
        ]);
        let map = Boundary::of(&fg, &file("src/a/one.rs"));
        assert!(map.crossings.is_empty());
    }

    /// A file that lands on the door is the discipline holding, exactly as
    /// it is for the folder — the three-way split is unchanged.
    #[test]
    fn a_files_import_onto_a_door_is_not_a_finding() {
        let fg = graph(&[
            ("src/a/one.rs", "src/x/door.rs"),
            ("src/b/two.rs", "src/x/door.rs"),
        ]);
        let map = Boundary::of(&fg, &file("src/a/one.rs"));
        assert_eq!(map.crossings.len(), 1);
        assert_eq!(map.crossings[0].verdict(), Verdict::Door);
    }

    /// The report speaks about the file it was asked about, not about
    /// "one of your files" — which would read as a folder's report.
    #[test]
    fn a_file_subject_speaks_in_the_singular() {
        let e = Evidence {
            mine: 1,
            theirs: 1,
            names: ["Door".to_string()].into_iter().collect(),
            doors: 1,
            fewest_reachers: 1,
        };
        let said = prescribe(&e, &file("src/a/one.rs"));
        assert!(said.contains("This file wants"), "unexpected: {said}");
        let folder_said = prescribe(&e, &folder("src/a"));
        assert!(
            folder_said.contains("One of your files wants"),
            "unexpected: {folder_said}"
        );
    }
}
