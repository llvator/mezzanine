# mezz.elv — domain map of mezz itself (root file of the spec).
# Sketch-first: every area named in one line here; deepened categories
# live in <category>.elv (split rule: a category moves out when it
# gains Features).
# Health: `elevator spec/ --check` and `elevator spec/ --drift --code-root .`
# (cr paths are repo-root-relative, so drift needs the explicit root).

import "elevator.elv"
import "code_graph.elv"
import "agent_tools.elv"
import "server.elv"
import "cli.elv"
import "educator.elv"
import "visualizer.elv"
import "concepts.elv"
