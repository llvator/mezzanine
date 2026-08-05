# Visualizer — force-directed graph UI + VS Code extension.

import "code_graph.elv"

c visualizer {
    d: "Force-directed graph UI + VS Code extension; watch mode re-analyzes on save. Elevator kinds render with hierarchy-encoded node sizes."
    cr: "ui/", "vscode-extension/"
    f visual_scopes
    f saved_views
    f engine_endpoint
    f theming
    f node_encoding
    f edge_encoding
    f grouping
    f viewport_overview
    f entity_search
    f entity_reading
    f editor_sync
    f spec_pairing
}

f engine_endpoint {
    d: "Which engine the UI talks to, resolved at runtime rather than assumed to be whoever served the page: the VS Code webview's apiBase first, then ?api=, then localStorage, then same-origin. apiUrl composes that base with the serve-mode slug prefix and the pairing token; flatApiUrl takes the base but never the slug, for the endpoints that live outside the per-repo namespace. connection.ts probes /api/hello before anything loads, which is what lets the connect screen tell a refused origin from a dead port from a server that is not nao — three failures a browser reports identically. liveReload runs the same probe when the SSE stream drops, so a broken stream reads as not-live instead of nothing-is-changing."
    cr: "ui/src/endpoint.ts", "ui/src/stores/connection.ts", "ui/src/components/ConnectScreen.svelte"
}

f visual_scopes {
    d: "File-path scope filtering for the graph canvas (filterToSelection). Elevator entities close over their transitive Contains descendants and attached Concepts across .elv file boundaries — a Category never renders without the Features it declares elsewhere; code entities keep strict file scoping. Descendants only, so narrowing to one branch still narrows."
    cr: "ui/src/stores/scope.ts"
    fu marked_set
}

fu f.visual_scopes.marked_set {
    d: "How a reader gets from 'which files talk to each other' to 'which entities in THESE files talk to each other'. ⌘/Ctrl-click marks a node, `m` marks the hovered one, and drillIntoMarks spends the whole set through setScopes — the plural is the point, because a relationship runs BETWEEN files, so drilling into one end throws away the other and drilling into the folder holding both usually brings back too much to draw at entity level. Nothing in it names an aggregation level: autoLevel re-picks from RENDER_BUDGET against the new, smaller scope, which is the same mechanism that already opens any small scope at Entity level. markedStats runs that comparison against the index BEFORE the click, so a set too big to draw as entities says 'opens at file level' instead of silently returning another file view. Marks are PATHS, never ids, and here that is load-bearing rather than convenient: collapseGraph mints fresh ids on every level change, and a mark's whole job is to survive the level change it causes. markPathOf reads file_path alone and never kind_raw — a File rollup's file_path is its own path, a Module rollup's is its directory, an entity's is the file it came from — so one field means 'the narrowest scope this circle is evidence of' at every level, and the ring lands on the file circle and on every entity inside it for the same reason. Ghosts and the root Module are unmarkable, both spelling their path '' , which as a scope is the whole repo. A mark is not a filter and changes nothing on screen but its own ring: comparing two files means keeping the neighbourhood that made them interesting. The set is cleared once the scope applies — every node then drawn sits under a marked path, so keeping it would ring the entire canvas, and the scope tree is the durable record."
    cr: "ui/src/viewmodels/markSet.ts", "ui/src/stores/marks.ts", "ui/src/components/GraphView.svelte", "ui/src/components/CanvasToolbar.svelte"
}

f saved_views {
    d: "Naming the picture on the canvas so a reader can come back to it, and switch between two of them to see the same code at two levels or two filters. What a view holds is one line: everything that decides WHICH entities and relationships reach the canvas — the scope rules, the level and autoLevel, the kind, direction, language and file filters, the per-level tri-states, the ghost toggles, the spec cross-filter and a committed search — and nothing that decides where the reader is standing. Camera, selection, expanded scopes, open panes and the Quality population are all out: restoring must not move the reader's cursor, and a view that reopened a pane would be doing two jobs. Restoring writes the same stores the controls write, so there is no view mode and a restored view stays editable — which makes the order load-bearing, since applySelection republishes and seedFromGraph re-seeds the kind filters to 'everything present', so anything the seed touches is written after the scope and anything it deliberately leaves alone (the exclusions) either side. Staleness is expected rather than exceptional: hidden files, committed search ids and spec ids are pruned against three different universes — the full graph, the graph at the level now being drawn, and the spec graph — and the count is reported instead of the restore failing. Scope rules are never pruned, a path that exists on another branch being a reason to save a view rather than a defect in one. activeViewId is derived from the stores rather than set by the restore, so it goes null the moment the reader changes anything, which is the honest signal and what makes Update mean something."
    cr: "ui/src/viewmodels/savedViews.ts", "ui/src/stores/savedViews.ts", "ui/src/components/SavedViewsSection.svelte"
    cr.server: "src/server/views_handler.rs"
    fu store
}

fu f.saved_views.store {
    d: "Where a saved view lives, and what each failure means. <root>/.nao/views.json through GET/PUT /api/views, repo-scope by ADR 0008's line — a view is a set of paths, languages and kinds of this repo, true whoever clones it, and worth committing beside the code it describes; it also puts the browser UI and the VS Code webview on one list, both talking HTTP to the same nao watch. nao serve never gets the routes, its tree having arrived from a URL a stranger pasted, so the UI reads a 404 as 'this server has no view store' and falls back to localStorage keyed by the serve slug — never by the analyzed root, which is fetched by a component mounting after the load and would have the list read under one key and written under another. A 5xx is the opposite conclusion: the file is there and could not be parsed, so saving is refused rather than replacing a list nobody has seen. The envelope is typed and the view is not — id, name and saved_at so the file diffs as a named list and the server can refuse a nameless entry, state opaque so capturing one more toggle stays a UI-only change. Writes are whole-list and atomic (temp file plus rename), and the client rolls its list back when one fails, a list showing a view the file does not have being how a reader loses one without being told."
    cr: "src/server/views_handler.rs", "ui/src/stores/savedViews.ts"
}

f node_encoding {
    d: "What a circle on the canvas means: size carries a magnitude metric (SIZE_CHANNELS, default loc), fill carries severity from the same composite_score the Quality panel ranks by. buildNodeEncoding is the one place that answers 'how big, what colour' — GraphView has two node-join sites and the legend a third, and each carrying its own copy is how the theme bug happened. Area, not radius, reads as magnitude, so radius is sqrt-scaled over R_MIN..R_MAX against a domain taken from the nodes in play, not a global maximum. Two things are deliberately not the bottom of a ramp: a node with no metrics renders at NO_DATA_RADIUS in half-opacity grey (hollow, not 'zero'), and a graph whose nodes mostly carry no metrics — isMetricFreeGraph, a majority test over isMetricFreeNode, an Elevator domain graph being the usual case — drops to kindRadius and the kind palette, because domain kinds carry no code metrics at all. A metric with no rollup at the current aggregation level is withheld from the channel picker rather than silently reading zero."
    references: f.metrics
    cr: "ui/src/viewmodels/nodeEncoding.ts", "ui/src/stores/encoding.ts", "ui/src/components/GraphView.svelte"
    fu legend
}

fu f.node_encoding.legend {
    d: "The Legend block in FilterPanel describes the ACTIVE encoding rather than a fixed palette, and owns the size-channel and colour-channel selectors — a reader answering 'what does this circle mean?' is already looking there. legendStops picks sample values by fraction of the domain in RADIUS space, not value space: sqrt scaling had 0.15/0.5 of the maximum both landing in the top half of the range, describing none of the small nodes a real graph is mostly made of. Stops dedupe, because niceValue rounding collapses two samples onto one number on a small domain. Sample dots are drawn at canvas radii shrunk by ONE shared factor (LEGEND_DOT_SCALE = 22 / R_MAX); the per-dot Math.min clamp it replaced flattened every stop past 17px into an identical circle, so the legend contradicted the channel it was explaining. Probe legend-dots-to-scale in the ui-014 suite asserts the dots stay distinct and ascending."
    cr: "ui/src/components/FilterPanel.svelte", "ui/scripts/ux-probe.mjs"
}

f edge_encoding {
    d: "What a line between two circles means. Colour is the relationship kind (linkStrokeColour off LINK_COLORS), a dash marks the binding, the label carries the kind and the merged count, and thickness is the merged weight — the number of underlying relationships behind one drawn edge, which is the one fact about an edge that node size and fill structurally cannot express, since both already mean a node metric. Which way a line points is not its stored direction: isReversed flips it so the focal node is the subject of a direction-aware label ('inherited by', 'called by'), falling back to kindPriority's parent-to-child order when the selection is off the edge. Edges are drawn under the nodes, so a line runs to a circle's centre and is hidden there — but the arrow head must not be, which is what makes head placement a geometry problem rather than a constant."
    references: f.node_encoding
    cr: "ui/src/viewmodels/linkGeometry.ts", "ui/src/components/GraphView.svelte"
    fu aim
}

fu f.edge_encoding.aim {
    d: "Where the arrow head lands, and how big it is. The marker is defined with markerUnits userSpaceOnUse, which is load-bearing: SVG's default is strokeWidth, so the head silently rendered at markerWidth x stroke-width and refX at the same scale. With thickness driven by merged weight and capped, a nominally 6-unit head drew at 36px — larger than R_MAX — and sat 90px back on a 120px link. At Module level, where collapsing merges every cross-directory relationship into one edge, that was the common case rather than the tail, which is why the defect was invisible at file level. ARROW_LEN fixes the head at 10px whatever the line's weight; a directional glyph has nothing more to say for growing. refX cannot carry the setback either, being one constant against a radius the reader chooses over R_MIN..R_MAX, so arrowHeadPoint trims each line's head end to the rim of the node it points at plus ARROW_GAP, clamped by MIN_LINE so overlapping nodes cannot push the head behind the tail. positionLinks is the single writer — one pass with direct attribute writes, since it runs on every tick — and restyleEncoding re-runs it when the size channel changes, or the setbacks describe the radius the circles just left. linkStrokeWidth keeps weight as a channel but re-ranged to 1.2-3.5px: at the old 6px cap it stopped discriminating exactly where it had the most to say."
    cr: "ui/src/viewmodels/linkGeometry.ts", "ui/scripts/link-geometry.test.ts"
}

f grouping {
    d: "Making the parts of a codebase visible without reading labels. The layout answers 'what calls what' by construction, and on a full-repo graph that alone is a hairball; grouping adds 'what lives together' as a second question the picture answers. folderKeyOf is the single definition of a group — the directory holding the file — and the force, the outlines and the hover membership (groupMemberIds) all take it, so a highlight can never light a different set from the region it is drawn inside. It normalises a leading ./ because one analyze run emits both spellings (AN-015): raw, five directories on this repo arrive twice, worst ui/src/stores at 744 entities one way and 85 the other, which is two centroids pulling one folder apart. Grouping may not claim colour or size — both already mean a metric — so it spends position, an outline, and a name."
    references: f.node_encoding
    cr: "ui/src/utils/forceCohesion.ts", "ui/src/viewmodels/folderHulls.ts", "ui/src/components/GraphView.svelte"
    fu cohere
    fu seed
    fu outline
}

fu f.grouping.cohere {
    d: "forceFolderCohesion nudges each node toward its folder's centroid, and toward each ancestor's above that. Two linear passes per tick over (node, tier) pairs — accumulate centroids, then apply — because a per-node centroid recomputation is quadratic and the render budget is 400 nodes. ancestorChainOf gives the chain, tierWeightFor prices each tier at ANCESTOR_DECAY^depth scaled by the share of the graph it EXCLUDES: a directory holding every drawn node has the graph's centroid, so pulling toward it is a disguised forceCenter, and it comes out at exactly zero. The node's own folder is exempt from that scaling, being UI-052's promise. Weights are normalised per node, so COHESION_STRENGTH still means what a reader tuned it to however many tiers a node has, and off is provably 0. Inert at Module level, where each node already is a folder. Measured nested against flat on this repo's widest scope: kin/stranger distance ratio 0.502 to 0.484 at Low and 0.464 to 0.376 at High."
    cr: "ui/src/utils/forceCohesion.ts"
}

fu f.grouping.seed {
    d: "makeSeeder places every node before the first tick, deterministically, from its path. Wedges are allocated by member count and ordered lexicographically — string order over paths IS depth-first order over the tree, so siblings take neighbouring wedges and a subtree occupies one arc with no tree walk. Angle and radius come from two FNV-1a hashes (hash32) of the node id, sqrt-spread by area; hashing the id rather than the path is what keeps ghosts, which share an empty file_path, off a single point. No Math.random anywhere: one call would void the reproducibility the module exists for. wedgeKeyOf normalises ./ exactly as folderKeyOf does — `.` sorts before every letter, so the two spellings would take wedges on opposite sides of the circle and the force would spend the settle undoing the seed."
    cr: "ui/src/utils/layoutSeed.ts"
}

fu f.grouping.outline {
    d: "computeFolderHulls draws a named region per group: a d3.polygonHull sampled around each member circle rather than through the centres, so the outline clears the nodes it encloses. Self-regulating by two guards that must not be loosened — trimOutliers drops members past 2.5 medians from the median centre, because a convex hull does not stretch for a stray but drags across everything between, and MAX_FOREIGN_SHARE 0.35 drops an outline that is mostly other groups' nodes, which is a lasso round the middle of the graph and not a region. hullDepth draws ancestor tiers too; the tier count limits what is DRAWN, never what a region holds, since a region has to contain every node beneath it or its name promises more than its shape delivers. Ancestry is therefore in the definition of foreign, not in the threshold. A parent (hasChildren) gets outline and name only — three nested washes at 6% stack to 17% and start shifting what the metric fills inside them appear to say. Painted largest-first, which for tiers is outside-in."
    cr: "ui/src/viewmodels/folderHulls.ts", "ui/src/components/FilterPanel.svelte"
}

f viewport_overview {
    d: "Where the viewport is, once the canvas has stopped being able to say. Zooming past ~2x fills the screen with a dozen circles that all look like the middle, and the only ways back were Fit View, which throws the magnification away, and panning blind. The overview panel is a floating corner of the canvas drawing every drawn node unzoomed, with a box around what is on screen. It draws circles and NOTHING else — labels and edges at 176px are a smear that buries the one mark the panel exists for — and takes position, radius and fill from GraphView's live encoding rather than a palette of its own, so it is a picture of this graph and not a second opinion about it. GraphView is the only writer of overviewDots and canvasViewport and the panel the only reader; the command back is a method call (panTo), a command having one recipient. The two publishes are deliberately separate because they change at different rates: the viewport is one object on a zoom or resize, the dots are a render budget's worth rebuilt off a tick handler firing 60 times a second, so they carry the same 100ms throttle as the folder hulls and the same immediate republish on settle, teardown and encoding change. Tree mode gets exactly one publish, from applyDisplayPlan, the simulation being stopped there."
    references: f.node_encoding
    cr: "ui/src/viewmodels/overviewFrame.ts", "ui/src/stores/overview.ts", "ui/src/components/OverviewPanel.svelte"
    fu steer
}

fu f.viewport_overview.steer {
    d: "The box, and the click that moves it. Everything is a mapping between world (where the simulation puts nodes), screen (world x the d3 zoom transform) and panel pixels; viewportWorldRect is the transform INVERTED, which is the whole trick — d3 says where the world goes on screen and the box needs the opposite reading. overviewWorld fits the union of the dots and the viewport, never the dots alone: panning off the side of the graph is half of getting lost, and fitted to the nodes only the box slides out of the panel at exactly the moment the reader most needs it. One scale on both axes, or the panel is a stretched graph the reader cannot recognise. centreTransform keeps the current k, because the panel moves you and does not re-frame you — a click that also changed the zoom would undo a magnification just set up by hand. Click and drag share one rule (centre on the pointer) rather than the drag picking up a grab offset, which would make a press on empty panel do nothing until the pointer moved; the SVG captures the pointer, since at 176px wide most drags leave it. dotRadius floors at MIN_DOT_R because sub-pixel circles render at an opacity that depends on where they land, making the cloud show density of antialiasing rather than density of code. The panel sits at bottom: 60px, not 12: the mode bar's live-status and endpoint chips own that corner, and covering a status indicator is worse than not having one."
    cr: "ui/src/viewmodels/overviewFrame.ts", "ui/scripts/overview-frame.test.ts"
}

f entity_search {
    d: "The Entities box: type a query, get ranked hits, commit some of them to reshape the canvas. Two phases on purpose — searchTerm drives a preview list and changes nothing, committedSearchIds is the only thing that reaches the display plan. A hit can sit at three distances from the canvas: drawn, loaded but not drawn, and present in the repo but never loaded. searchResults merges all three into ONE ranked sequence rather than a list per tier, because per-group ordering puts a strong match in a lower tier below a weak one in the top tier — which is the whole reason to rank. Every row therefore has to say which tier it is in and what would bring it onto the canvas."
    references: f.visual_scopes
    cr: "ui/src/viewmodels/searchResults.ts", "ui/src/components/EntitySearchResults.svelte"
    fu rank
    fu classify
    fu reinstate
    fu commit
}

fu f.entity_search.rank {
    d: "scoreSearch parses the query once and hands fuzzyPath's scoreQuery to scoreEntity as a PARAMETER rather than importing it — that is what lets the weighting policy be asserted against a stub scorer, and what keeps the module loadable by the node --test runner. FIELD_WEIGHT is name 1 > qualifiedName 0.9 > filename 0.8 > folder 0.6, the mirror of the scope box's discount in the other direction, because 'graph' means the file there and the function here. Best field wins, never the sum: summing rewards a node for matching in three mediocre places over one that matched the name exactly. matchesSearch is a wrapper over scoreSearch rather than a second body, so the predicate the cross-scope list shares cannot drift from the order the panel renders. Negation is tested against name, qualified_name and file_path together — scoped per field, `graph !test` returned an entity named graph living in a .test.ts file, because the name alone satisfied both terms."
    cr: "ui/src/utils/entityScore.ts", "ui/src/viewmodels/filterViewModel.ts"
}

fu f.entity_search.classify {
    d: "Why a hit is not on the canvas, so no row is silently undrawable. classifyBlock walks the same tests as nodePassesFilters in the same order and returns the FIRST that rejects, so the badge names the control actually deciding rather than an arbitrary one of several. It is a leaf taking a FilterSnapshot with type-only imports, which is what keeps it unit-testable. Three reasons are not per-node and are constants: COLLAPSED_BLOCK when the canvas draws above entity level, DISTANCE_BLOCK when a selection's BFS reach excluded it, CEILING_BLOCK when the plan overflowed and every visibility set is empty at once. Collapsed is its own reason and not 'out of scope' — the corpus is rawEntityGraph and visibleNodeIds is computed over the collapsed graphData, so at file level every entity hit reported itself outside a scope it was plainly inside. The two have opposite fixes: out-of-scope wants more data loaded, collapsed wants the same data drawn finer."
    cr: "ui/src/utils/blockReason.ts", "ui/src/viewmodels/displayPlan.ts"
}

fu f.entity_search.reinstate {
    d: "unblock maps a reason back to the control that reverses it and records a Relaxation, so relaxedFilters is visible and undoRelaxations puts every widening back newest-first. Nothing is relaxed silently: the reader set those filters deliberately, and a search that quietly widened the view would be worse than the problem it solves. The collapsed case must set autoLevel false BEFORE graphLevel — the level is re-picked from the render budget on the next scope application, and a scope large enough to have been collapsed is collapsed straight back, so the click reads as a no-op. Every other manual level setter in the UI pins it for the same reason. unblockAll dedupes by kind and value, because twenty rows behind one kind toggle is one relaxation and not twenty. CEILING_BLOCK alone returns false: the fix there is to narrow, and widening is what overflowed it."
    cr: "ui/src/viewmodels/searchResults.ts"
}

fu f.entity_search.commit {
    d: "Multi-select over the ranked list. Per-row checkboxes drive committedSearchIds through setCommittedMatches; shift-click applies the clicked box's NEW state to the run between it and the anchor, bound to click rather than change because shiftKey is not on the change event and the box has already flipped itself by then. Committing a blocked row also unblocks it — filtering the graph down to something no filter will draw is exactly the failure the badges exist to prevent. searchHidesNonMatches is the Highlight/Filter mode, drawn as a segmented control that is present before anything is committed, since the mode decides what the search will DO — but the canvas only consults it while committedSearchIds is non-empty, so before a commit the control changes nothing on screen and has to say so, or it reads as broken rather than as waiting. The same applies to selection: a bare column of checkboxes announces neither that it takes a shift-click range nor, when the column is all spacers, that the reason is a scope holding none of these entities. Out-of-scope rows go through one scopeToResults, which unions their files in a single addScopes: one focusScope per row would replace the scope N times, take N timeouts, and leave only the last row's file selected."
    cr: "ui/src/components/EntitySearchResults.svelte"
}

f entity_reading {
    d: "Which entity the side panels are about. Three stores in graph.ts rather than one: hoveredNode follows the pointer, selectedNode is the pinned subject and also the canvas's focus root, hoverLocked freezes the preview so the cursor can leave the canvas. DetailsPanel reads `selectedNode ?? hoveredNode` — selection wins — and renders the hover preview compact, without relationships, which are noise while the cursor is still moving. A hover is only true while the canvas draws the node: mouseout never fires when a re-render destroys the element the pointer was over, so graphData drops a hovered node it no longer contains, and focusNode — clear the hover, then select — is what the panels call, because the pointer is over a panel when they run. A frozen preview survives both, being the one hover the reader asked to keep."
    cr: "ui/src/stores/graph.ts", "ui/src/components/DetailsPanel.svelte"
    fu describe
    fu relate
}

fu f.entity_reading.describe {
    d: "The Description pane reads the graph as prose instead of as topology. buildDescriptionChain climbs parent_id from the subject (MAX_DEPTH 8; Elevator tops out at 4), so an entity reads as its own description followed by the ones that explain what it is for; an ancestor outside the loaded graph still contributes its sidecar description under a labelFromId label rather than truncating the chain at a scope boundary. buildChildEntries is the other direction and one level only — a tree of descriptions is the scope tree with prose attached — rendered under the subject rung as `contains (N)`, capped at CHILD_PREVIEW behind Show all N and collapsed again when the subject changes. It exists because a Feature's meaning is largely the Functionalities declared under it, which a walk that only climbs can never reach. Both directions accept a child whose parent_id is the parent's bare name (Rust impl blocks), but the descent only honours it when no entity actually owns that id, or a namesake adopts another entity's children. describeOnHover off makes the pane selection-only."
    cr: "ui/src/viewmodels/descriptionChain.ts", "ui/src/stores/description.ts", "ui/src/components/DescriptionPanel.svelte"
}

fu f.entity_reading.relate {
    d: "The Details column's Relationships section: getOutgoing and getIncoming split the subject's edges by direction, sortByOrder keeps the parser's call order where it emitted one, and the incoming side renders incoming_kind ('defined in') rather than the active label. Each row is a button, not a div — it names the entity at the other end, and clicking it makes that entity the subject (focusRelated), which is the navigation the section had looked like it offered since it was written. Both endpoints come out of graphData, so the node is on the canvas by construction: no scope widening, unlike revealSpecEntity behind the Claimed-by rows."
    cr: "ui/src/components/EntityInfo.svelte"
}

f editor_sync {
    d: "The two-way bridge between the editor and the graph, and the loop it exists to not close. Editor to graph: onDidChangeActiveTextEditor, and onDidChangeTextEditorSelection behind a 150ms debounce, push focusFile and focusCursor into the webview while nao.autoVisualize is on; setCurrentEditorFile runs outside every gate because it only feeds Quality's 'Current file' option and moves nothing on screen. Graph to editor is follow. Both legs ride the same two events, so each side's write is the other's read, and suppressEditorEventsUntil is what keeps a programmatic open from returning as a focusCursor — a timestamp rather than a flag, the echo being asynchronous. The analysis-scope tree follows the active editor through applyFollow on that same event but deliberately outside the panel check, since narrowing the scope tree is useful with no visualizer open; nao.analysisScopeFollowMode picks file or folder, and the on/off state persists in workspaceState while followSelection does not."
    references: f.entity_reading
    cr: "vscode-extension/src/extension.ts", "vscode-extension/src/panel.ts"
    fu follow
}

fu f.editor_sync.follow {
    d: "'Follow selection to editor': selecting a node opens its file at its line, preserveFocus so the canvas keeps the keyboard. openTextDocument loads from disk, so the file does not have to be open already. isOpenableSelection is the guard and it declines twice. Module nodes, because collapseGraph sets file_path to the directory the entities share and openTextDocument rejects on a directory — the handler is async and nothing observes its rejection, so that was a click that did nothing, reported nothing, and still armed the 800ms suppression window; the window is now dropped on failure and the reason reaches the output channel. And a selection whose span already holds the caret, because cursor-sync turns a click at line 500 into a selection of the function at 480 which arrives straight back here, and following it drags the caret to the declaration — the same round-trip the non-empty-selection test blocks for drag-select, reached by an ordinary click. resolveWorkspacePath tests path.isAbsolute before joining: the analyzer falls back to a raw absolute path for a file outside the analyzed root, and Uri.joinPath appends an absolute segment rather than substituting it. The checkbox is mirrored into ControlsViewProvider and stamped into its markup rather than posted on ready, the view being registered without retainContextWhenHidden — the markup is the rehydration point, and without the mirror, hiding the view redrew it unchecked while following stayed on."
    cr: "vscode-extension/src/extension.ts", "vscode-extension/src/controlsViewProvider.ts"
}

f theming {
    d: "Five palettes in one THEMES array, whose order is both the picker's render order and the product default — first entry wins, currently llvator (amber on near-black, matching llvator.com); midnight sits last. applyTheme writes every palette colour onto :root as a CSS custom property and stamps data-theme, so components style off var(--…) and never a literal. Three things must move together with the default: THEMES[0], loadTheme's fallback, and the :root block in app.css, which paints the first frame before any script runs — a mismatch there is a visible flash of the wrong palette, not a silent one. The chosen id persists raw (not JSON) under localStorage nao-theme and always beats the default, so changing the default only affects first runs. The graph canvas cannot use CSS vars: canvasChrome re-reads them at render time and GraphView restyles on every activeTheme change. The VS Code webview has no theme code of its own — it serves the same build, so it inherits all of this."
    cr: "ui/src/stores/settings.ts", "ui/src/app.css", "ui/src/components/Settings.svelte", "ui/src/utils/canvasChrome.ts", "ui/src/components/GraphView.svelte"
}

f spec_pairing {
    d: "Reading the code through the Elevator spec rather than beside it. The .elv layer draws on its own canvas next to the graph, and selecting in it narrows the graph to the code those entities declare with cr:. One selection, two surfaces — the pane and the Filters pane's Spec section — and the filter depends on neither being visible, which is what lets the pane be collapsed for canvas room without losing it. The filter is a layer over the visual scope and never a write to it, so clearing restores the reader's scope including its exclusions; it is not setScopes, the destructive door showImplementingCode goes through for in-place pairing. Every path comparison runs through refPaths on BOTH sides (normalizeRefPath, pathClaims, pathsClaim, buildPathUniverse) — a ref normalised against a raw file_path matched nothing and reported the whole spec as drift while looking healthy. ADR 0011 supersedes 0005, which rejected this on the grounds that it needed a second GraphView, a second force simulation and a scope store holding two selections; the first two were avoided and the third was simply wrong."
    references: f.visual_scopes, f.entity_search
    cr: "ui/src/utils/refPaths.ts"
    fu filter
    fu drill
    fu mark
    fu pick
}

fu f.spec_pairing.filter {
    d: "crossFilterPaths is claimedPathsForAll over specSelection, and nodePassesFilters hides any code entity no path in it claims. Matching on file_path rather than id is what keeps it working at every aggregation level: collapseGraph mints fresh ids for File and Module nodes, and file_path survives the collapse, so one predicate answers for an entity, its file circle and its module circle. null and [] are different and must not collapse — null is no filter, [] is 'the selected entities declare no code' and empties the canvas on purpose, which is the only way missing cr: coverage ever becomes visible. Not gated on splitViewOpen: collapsing the pane asks for canvas room, not for filtering to stop, and the gate that used to do that made the two wants mutually exclusive. classifyBlock walks the same tests in the same order so a search hit blocked by this reports 'hidden: spec filter' rather than blaming a kind toggle. A spec entity selected while the pane is open is not a selection as far as the code canvas is concerned — otherwise tree mode re-roots on a Feature and force mode restricts to the BFS reach of a node with no code edges, emptying it."
    cr: "ui/src/stores/crossFilter.ts"
}

fu f.spec_pairing.drill {
    d: "The pane reveals one level at a time: revealedIds draws specRoots plus every open step and its children, so a level exists on screen only once its parent was opened. 8 nodes, then 16, then 18, against 76 flat. The flat version's failure was not node count but equal weight — the Feature you wanted was indistinguishable from the forty you did not, and emphasis had to fade three quarters of the pane to make a selection findable. Roots come from containment rather than a kind test, so a spec using Extensions opens on those, Concepts appear because nothing contains them, and an orphan Feature no Category declares stays reachable. filterSpecGraph carries the claims map over untouched: drilling changes what you can see and never what a selection means, or the same click would filter to different code depending how far you had drilled. trailTo takes the first parent only — a Feature under two Categories has two honest trails, and drawing both is what made the flat view unreadable. A code selection forces its claimant's trail open, so the reverse leg survives progressive disclosure."
    cr: "ui/src/components/SpecGraphView.svelte"
}

fu f.spec_pairing.mark {
    d: "Why clicking an entity would show nothing, when it would. specScopeStates classifies against TWO path universes from buildPathUniverse — the loaded scope's and the whole repo's — into SpecScopeState: in-scope, out-of-scope, unanalyzed, unanchored. Four because they take four different fixes, and against the loaded scope alone a dead path and a narrow scope are the same picture. unanalyzed stops short of claiming drift on purpose: this sees the parsed graph and not the filesystem, so a cr: at a file type outside the analysed languages is indistinguishable from one that died — f recall_bench claims a .py script that .nao/settings.json does not parse, and the earlier wording badged that correct ref as drift. elevator --drift is the check that separates them. Classification is on the rolled-up claims, not own refs, or every Category reads unanchored, never carrying a cr: of its own. Marking is the default and followAnalysisScope hides instead, because a map that loses rows as you re-scope stops being one. The store this reads is rawEntityGraph, not analysisGraphData: UI-051 renamed the sidebar heading to 'Analysis Scope' and left the store alone, so analysisRules belongs to the VS Code-only tree and defaults to the whole repo — keying off it made the marker inert in the browser, where it was asked for."
    cr: "ui/src/viewmodels/specGraph.ts"
}

fu f.spec_pairing.pick {
    d: "The same filter as a checkbox list, in the Filters pane's Post-Filtering block, and the reason the filter can outlive its pane at all — the fix for 'no visible control' is a second control, not a self-cancelling filter. specOptions returns one list per tier; a checked entity stays offered even when the drill rule would hide it, or unchecking a Category strands its Features checked and unreachable while still filtering the canvas. Drill down is optional here and mandatory in the pane, because the list's whole advantage is reaching a Functionality without knowing which Category it hangs from, and drill-down trades exactly that away. Selection is a union, matching how the kind and language filters read; an intersection would be near-always empty, two Categories sharing code being the exception the cr: duplicate marker exists to flag. A click in the pane replaces the selection because it also moves the view, while a checkbox adds — multi-select is an explicit act on the explicit surface."
    cr: "ui/src/components/SpecFilterSection.svelte"
}
