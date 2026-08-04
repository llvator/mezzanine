# Visualizer — force-directed graph UI + VS Code extension.

import "code_graph.elv"

c visualizer {
    d: "Force-directed graph UI + VS Code extension; watch mode re-analyzes on save. Elevator kinds render with hierarchy-encoded node sizes."
    cr: "ui/", "vscode-extension/"
    f visual_scopes
    f engine_endpoint
    f theming
    f node_encoding
}

f engine_endpoint {
    d: "Which engine the UI talks to, resolved at runtime rather than assumed to be whoever served the page: the VS Code webview's apiBase first, then ?api=, then localStorage, then same-origin. apiUrl composes that base with the serve-mode slug prefix and the pairing token; flatApiUrl takes the base but never the slug, for the endpoints that live outside the per-repo namespace. connection.ts probes /api/hello before anything loads, which is what lets the connect screen tell a refused origin from a dead port from a server that is not nao — three failures a browser reports identically. liveReload runs the same probe when the SSE stream drops, so a broken stream reads as not-live instead of nothing-is-changing."
    cr: "ui/src/endpoint.ts", "ui/src/stores/connection.ts", "ui/src/components/ConnectScreen.svelte"
}

f visual_scopes {
    d: "File-path scope filtering for the graph canvas (filterToSelection). Elevator entities close over their transitive Contains descendants and attached Concepts across .elv file boundaries — a Category never renders without the Features it declares elsewhere; code entities keep strict file scoping. Descendants only, so narrowing to one branch still narrows."
    cr: "ui/src/stores/scope.ts"
}

f node_encoding {
    d: "What a circle on the canvas means: size carries a magnitude metric (SIZE_CHANNELS, default loc), fill carries severity from the same composite_score the Quality panel ranks by. buildNodeEncoding is the one place that answers 'how big, what colour' — GraphView has two node-join sites and the legend a third, and each carrying its own copy is how the theme bug happened. Area, not radius, reads as magnitude, so radius is sqrt-scaled over R_MIN..R_MAX against a domain taken from the nodes in play, not a global maximum. Two things are deliberately not the bottom of a ramp: a node with no metrics renders at NO_DATA_RADIUS in half-opacity grey (hollow, not 'zero'), and an Elevator graph — isElevatorGraph, a majority test — drops to kindRadius and the kind palette, because domain kinds carry no code metrics at all. A metric with no rollup at the current aggregation level is withheld from the channel picker rather than silently reading zero."
    references: f.metrics
    cr: "ui/src/viewmodels/nodeEncoding.ts", "ui/src/stores/encoding.ts", "ui/src/components/GraphView.svelte"
    fu legend
}

fu f.node_encoding.legend {
    d: "The Legend block in FilterPanel describes the ACTIVE encoding rather than a fixed palette, and owns the size-channel and colour-channel selectors — a reader answering 'what does this circle mean?' is already looking there. legendStops picks sample values by fraction of the domain in RADIUS space, not value space: sqrt scaling had 0.15/0.5 of the maximum both landing in the top half of the range, describing none of the small nodes a real graph is mostly made of. Stops dedupe, because niceValue rounding collapses two samples onto one number on a small domain. Sample dots are drawn at canvas radii shrunk by ONE shared factor (LEGEND_DOT_SCALE = 22 / R_MAX); the per-dot Math.min clamp it replaced flattened every stop past 17px into an identical circle, so the legend contradicted the channel it was explaining. Probe legend-dots-to-scale in the ui-014 suite asserts the dots stay distinct and ascending."
    cr: "ui/src/components/FilterPanel.svelte", "ui/scripts/ux-probe.mjs"
}

f theming {
    d: "Five palettes in one THEMES array, whose order is both the picker's render order and the product default — first entry wins, currently llvator (amber on near-black, matching llvator.com); midnight sits last. applyTheme writes every palette colour onto :root as a CSS custom property and stamps data-theme, so components style off var(--…) and never a literal. Three things must move together with the default: THEMES[0], loadTheme's fallback, and the :root block in app.css, which paints the first frame before any script runs — a mismatch there is a visible flash of the wrong palette, not a silent one. The chosen id persists raw (not JSON) under localStorage nao-theme and always beats the default, so changing the default only affects first runs. The graph canvas cannot use CSS vars: canvasChrome re-reads them at render time and GraphView restyles on every activeTheme change. The VS Code webview has no theme code of its own — it serves the same build, so it inherits all of this."
    cr: "ui/src/stores/settings.ts", "ui/src/app.css", "ui/src/components/Settings.svelte", "ui/src/utils/canvasChrome.ts", "ui/src/components/GraphView.svelte"
}
