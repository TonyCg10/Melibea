# Melibea roadmap

## Now — Focus-responsive geometry

### Outcome

In a real niri session, a configured terminal expands when focused and returns
to its compact width when focus moves back to the editor, without corrupting
manual width state or reacting to excluded surfaces.

### In

- niri IPC connection and event-stream recovery.
- In-memory window and column registry.
- Rules matching `app_id`, with optional title matching only where necessary.
- Focused, unfocused, and `preserve` width policies.
- Dry-run diagnostics before live mutation.
- Unit tests for transitions and stale-event handling.

### Out

- Minimization and bubbles.
- Celestina integration.
- Changes to niri source.
- Floating windows, dialogs, popups, and fullscreen policy.
- Visual clipping that preserves the client's full width.
- Persistence across compositor restarts.

### Work

1. Define typed configuration and width values.
2. Add a pure transition engine for focused and previously focused columns.
3. Adapt niri's event stream into the internal registry with reconnection.
4. Emit dry-run decisions with enough context to explain every transition.
5. Apply width actions through niri IPC and ignore obsolete transitions.
6. Exercise the 10% terminal / 90% editor workflow in a nested and then daily session.

### Exit

- Rapid focus changes converge on the latest focused column.
- A matching terminal follows its configured focused and unfocused widths.
- A `preserve` editor is never contracted by Melibea.
- Disconnecting or stopping Melibea leaves niri usable and reports a clear error.
- The interaction remains comfortable during a sustained real-work session.

## Next

### Reliable daily-use controller

Preserve manual widths, handle tabbed columns and reconnect from a fresh niri
snapshot. Decide from observed evidence whether niri needs one atomic action.

### Native minimization spike

Prototype a niri-owned minimized state that removes a live surface from layout,
rendering, input, overview, and workspace navigation, then restores it by an
explicit deterministic rule.

## Later

- A versioned Melibea client contract and diagnostic CLI.
- Celestina Shell bubbles backed by native minimized state.
- Coordinated minimize and restore animations.
- Optional compositor-rendered previews when icons and titles prove ambiguous.
- Native visual contraction without resizing client content.

## Non-goals

- A new compositor or replacement for niri.
- A general plugin collection competing with Piri.
- AI classification or habit learning.
- Contexts, scenes, or automatic project inference.
- Hidden or off-screen workspaces presented as real minimization.
- Requiring Celestina to run Melibea.

