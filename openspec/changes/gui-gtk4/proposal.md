# Proposal: GTK4 GUI — Optional Desktop Application

> **Status**: Skeleton — this proposal is a roadmap placeholder. Full specs and tasks will be drafted when this change is picked up. **This proposal is optional and intentionally scheduled last.**

## Tier

Optional (post-Tier 2 polish)

## Summary

A GTK4 + libadwaita desktop application that provides a rich graphical view into the companion daemon — multi-pane conversation history, visual memory graph, configuration editor, session management, cost tracking. Written in Rust via `gtk4-rs` and `relm4`. Explicitly optional: most users will be well-served by the TUI dashboard, and this GUI is for users who prefer a proper desktop app experience.

## Motivation

The TUI dashboard (`tui-dashboard`) covers the majority of the dashboard use case for terminal-native users. But some users prefer a graphical app — for visual memory exploration, comfortable long-form reading of conversation history, mouse-driven session switching, and a persistent window in their workspace rather than a terminal buffer. On axios specifically, a libadwaita app would fit naturally into the GNOME HIG if the user is on a DE that respects it, and still runs fine (if as a "guest" application) on Niri + DankMaterialShell.

This proposal is explicitly last in the roadmap. It should NOT be started until Tiers 0, 1, and 2 are shipped and stable. The GUI is a value-add, not a foundation.

## Scope

### In scope

- `companion-gui` binary using `gtk4-rs` + `libadwaita-rs` + `relm4`
- Panels:
  - Conversation history browser with search
  - Memory graph visualization (nodes = memory files, edges = wikilinks/references)
  - Session manager (list, filter, resume, delete)
  - Configuration editor (wraps the home-manager module options for runtime tweaking where possible)
  - Cost/usage charts (if applicable)
- libadwaita design patterns: sidebars, toasts, banners, preferences dialog
- D-Bus client connection to the companion daemon
- Graceful handling of daemon unavailability

### Out of scope

- Replacing the TUI dashboard — both exist in parallel
- Cross-DE theming beyond what libadwaita provides by default
- Chat composition inside the GUI (you use the CLI or TUI for that; this is a dashboard, not a chat client)
- Mobile or tablet layouts

### Non-goals

- A custom chat UI — the goal is retrospective viewing and management, not driving conversations
- Qt/KDE variant (Rust + Qt is a painful path; libadwaita runs fine on KDE even if it looks guest-ish)
- Electron, Tauri, or any web-based GUI

## Dependencies

- `bootstrap`
- `daemon-core`
- `cli-client`
- `tui-dashboard` (establishes the D-Bus client patterns the GUI can reuse)
- `distributed-routing` is optional — the GUI works with Tier 1 single-machine daemons too

## Success criteria

1. `companion-gui` launches as a libadwaita application and connects to the local daemon
2. All panels render correctly on GNOME, KDE, and tiling WMs (Niri, Hyprland, Sway)
3. Conversation history is searchable and browsable
4. Memory graph visualizes wikilink relationships between memory files
5. Configuration edits persist correctly (either by rewriting nix config with a warning to rebuild, or by writing to a runtime-tweakable config file)
6. The GUI is NOT required for any Tier 0/1/2 functionality — users who never install it have full access to every feature via CLI and TUI
