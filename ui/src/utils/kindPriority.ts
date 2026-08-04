/**
 * Containment hierarchy for entity kinds. Lower numbers = "more parent"
 * (closer to the root of the containment tree). Used in two places:
 *
 * 1. Graph view without a focused node on an edge: picks which end is
 *    rendered as the arrow tail so the reading flows from parent to
 *    child (`Function → has param → Parameter` rather than the stored
 *    data direction param → func).
 *
 * 2. Heuristic tie-break between two entities of different kinds when
 *    we need a stable "who wins" answer.
 *
 * Kinds are matched on their `kind_raw` display form (the value that
 * ends up on `D3Node.kind_raw`), not the server's snake-case.
 */
const PRIORITY: Record<string, number> = {
  Module: 0,
  File: 1,
  Class: 2,
  Dataclass: 2,
  AbstractClass: 2,
  Struct: 2,
  Interface: 2,
  Trait: 2,
  Enum: 2,
  TypeAlias: 2,
  Service: 2,
  Import: 2,
  Method: 3,
  Function: 3,
  Constant: 3,
  Macro: 3,
  // Branch and Loop both sit at the method tier so Caller → Branch/Loop
  // → Callee renders with the stored source→target direction on both
  // hops (no hierarchy flip), matching the natural "caller holds the
  // scope, scope holds the call" reading.
  Branch: 3,
  Loop: 3,
  Variable: 4,
  Property: 4,
  Parameter: 5,
  // ansible-deploy containment: Playbook ⊃ Role, HostGroup ⊃
  // DeploymentSet ⊃ DeploymentEntry, TemplateFile ⊃ K8sResource. Tiers
  // are set so the tree reads parent → child.
  Playbook: 0,
  HostGroup: 0,
  Role: 1,
  DeploymentSet: 1,
  TemplateFile: 1,
  HelmChart: 2,
  DeploymentEntry: 2,
  K8sResource: 2,
};

/** Return the priority for a kind_raw value, or a mid-tier default for
 *  unrecognised kinds so they don't erroneously "win" either end. */
export function kindPriority(kindRaw: string): number {
  return PRIORITY[kindRaw] ?? 3;
}

/** True iff a's parent-priority is strictly higher (= larger number, more
 *  child-like) than b's. Used to decide whether a graph-view edge should
 *  flip visually so the arrow goes parent → child. */
export function isMoreChildThan(aKindRaw: string, bKindRaw: string): boolean {
  return kindPriority(aKindRaw) > kindPriority(bKindRaw);
}
