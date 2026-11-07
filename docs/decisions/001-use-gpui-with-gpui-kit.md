# 001. Use GPUI with GPUI Kit

- **Status**: accepted. The first milestone met its [performance budget](../roadmap.md#performance-budget) on macOS
- **Context**: The app stays open for whole shifts and renders dense tables and log streams. Official `gpui` on crates.io has had no new version since October 2025.
- **Decision**: Build on GPUI through an exact pin of `gpui-kit`, which brings its matching `gpui-pre` snapshot. Reject GPUI CE because its API is diverging and it bans agent-written contributions.
- **Consequences**:
  - Dense UI needs less custom work because GPUI Kit provides tables, docks and an editor.
  - Upgrades are planned events, tested headlessly on each bump.
  - The project depends heavily on Longbridge. Apache-2.0 permits a fork if needed.
  - Accessibility, right-to-left text and input methods stay immature, so test them per milestone.
