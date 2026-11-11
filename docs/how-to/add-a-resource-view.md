# Add a resource view

Add a live table for a kind of object, such as StatefulSets, with its details panel. Deployments are the worked example: read `crates/core/src/deployments.rs` and `crates/ui/src/deployments.rs` beside this page.

A view needs one core module, one UI module and one registration. The watch, the table, sorting, filters, hidden columns, selection, scroll anchoring, the empty and **No permission** states, and the saved view all come for free.

## 1. Describe the kind in core

Add `crates/core/src/<kind>.rs` and export it from `lib.rs`.

- A row type with the fields the table shows, such as `DeploymentRow`. Keep it small, because every object's row stays in memory.
- `impl Row for <Row>`: `type Object`, `from_object`, `api`, `namespace` and `name`. Use `namespaced_api` for a namespaced kind. For a cluster-scoped kind such as Nodes, set `const NAMESPACED: bool = false`, return `Api::all`, and return `""` from `namespace`.
- A status word that marks problems, worked out in core from conditions and counts, and tested there.
- A details type with `impl ObjectDetails`: `type Object`, `api`, `read`, `yaml` and `events`. Use `to_yaml`, which drops `managedFields`.
- Tests against `fake_api`, including a 403 for the kind. `deployments.rs` has a test where Deployments are refused while pods still list.

`Backend::watch::<Row>` and `Backend::details::<Details>` then work with no further changes.

## 2. Show it in the UI

Add `crates/ui/src/<kind>.rs` and declare it in `main.rs`.

- `impl Inspect for <Row>`: the Details type and the Summary tab. Reuse `field` and `section_title` from `details.rs`. Name related objects with `object_link` and an `ObjectLink`, such as a StatefulSet's pods' Node, so they open in their view. If the details need a second read to name one, as a pod's ReplicaSet does to find its Deployment, override `ObjectDetails::resolve` in core.
- `impl TableRow for <Row>`: `SINGULAR` and `PLURAL` for notices, `COLUMNS`, and `tone`, which colours the status and sets the default problems-first order. Each `ColumnDef` gives a name, a width, a `CellStyle`, the cell text and the ascending order. The first column is the name, which always shows. Override `follow` for a workload, so **Tail its pods** and **Show its pods** work, and `labels` if a selector should narrow the kind. A kind whose pods are not chosen by a selector, such as Nodes, overrides `pods` with a `RowMatch::Field` instead, and the pod row answers that field in `field`. For a **Pods** column counted from the pod watch, use `CellStyle::PodCount` and override `pod_count`. Override `jump` if `Enter` on a selected row should go somewhere other than its details, as a namespace's does.
- `impl Inspect for <Row>`: the Details type and the Summary tab. Reuse `field` and `section_title` from `details.rs`. Name related objects with `object_link` and an `ObjectLink`, such as a StatefulSet's pods' Node, so they open in their view. If the details need a second read to name one, as a pod's ReplicaSet does to find its Deployment, override `ObjectDetails::resolve` in core.

## 3. Register the view

In `crates/ui/src/views.rs`, add a variant to `View` with its name, group, key and icon, and a line to `new_panel`. The name must match the sidebar label in `sidebar.rs`, which then enables the item. Links to the view's kind, such as a pod's Node, then open it too: `KIND_NAMES` in `views.rs` maps each sidebar label to its Kubernetes kind, and a link whose kind has no registered view stays plain text. Digits are reserved in sidebar order so that parallel work does not collide: 3 StatefulSets, 4 DaemonSets, 5 Jobs, 6 Nodes, 7 Namespaces, 8 Events. The palette's **Go to** command follows from `View::ALL`.

## 4. Check it

- Add UI tests in `root.rs` that open the view, and one for its **No permission** state. `fake_api::Fixture` serves a list from `objects` and refuses the paths in `forbidden`.
- Add preview scenes in `preview.rs` for the table and its details, in dark and light, and look at them. See [Preview the UI](preview-the-ui.md).
- If the simulator has no objects of the kind, add some to `scripts/sim-cluster.sh`, then measure with `TAFFRAIL_BENCH_VIEW=<name>`. See [Measure performance](measure-performance.md).
- Update `docs/design.md` with the view's columns and status words, and the keyboard table with its digit.

## Rules that hold for every view

- Keep it read-only. List, watch and get only. Do not ask the API what you may read with a `SelfSubjectAccessReview` or `SelfSubjectRulesReview`: those are create calls, and a 403 from the list says the same.
- Never list Secret values. Secrets need their own decision first.
