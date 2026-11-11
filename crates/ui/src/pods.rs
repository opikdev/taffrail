use std::collections::BTreeMap;

use taffrail_core::{LogTarget, PodRow, log_targets};

use crate::table::{
    CellStyle, ColumnDef, ResourceTable, TableRow, Tone, age_text, by_age, urgency,
};

pub type PodTable = ResourceTable<PodRow>;

pub fn status_tone(status: &str) -> Tone {
    match status {
        "Running" => Tone::Healthy,
        "Completed" | "Succeeded" => Tone::Neutral,
        "Pending" | "ContainerCreating" | "PodInitializing" | "Terminating" => Tone::Degraded,
        _ => Tone::Failed,
    }
}

fn ready_counts(ready: &str) -> (u32, u32) {
    let (ready, total) = ready.split_once('/').unwrap_or_default();
    (ready.parse().unwrap_or(0), total.parse().unwrap_or(0))
}

impl TableRow for PodRow {
    const SINGULAR: &'static str = "pod";
    const PLURAL: &'static str = "pods";
    const COLUMNS: &'static [ColumnDef<Self>] = &[
        ColumnDef {
            name: "Name",
            width: 320.,
            style: CellStyle::Text,
            text: |row, _| row.name.clone().into(),
            compare: |a, b| a.name.cmp(&b.name),
        },
        ColumnDef {
            name: "Namespace",
            width: 120.,
            style: CellStyle::Muted,
            text: |row, _| row.namespace.clone().into(),
            compare: |a, b| a.namespace.cmp(&b.namespace),
        },
        ColumnDef {
            name: "Status",
            width: 170.,
            style: CellStyle::Status,
            text: |row, _| row.status.clone().into(),
            compare: |a, b| {
                urgency(a)
                    .cmp(&urgency(b))
                    .then_with(|| a.status.cmp(&b.status))
            },
        },
        ColumnDef {
            name: "Node",
            width: 130.,
            style: CellStyle::Muted,
            text: |row, _| row.node.clone().into(),
            compare: |a, b| a.node.cmp(&b.node),
        },
        ColumnDef {
            name: "Ready",
            width: 64.,
            style: CellStyle::Number,
            text: |row, _| row.ready.clone().into(),
            compare: |a, b| ready_counts(&a.ready).cmp(&ready_counts(&b.ready)),
        },
        ColumnDef {
            name: "Restarts",
            width: 80.,
            style: CellStyle::Number,
            text: |row, _| row.restarts.to_string().into(),
            compare: |a, b| a.restarts.cmp(&b.restarts),
        },
        ColumnDef {
            name: "Age",
            width: 60.,
            style: CellStyle::Number,
            text: |row, now| age_text(row.created_unix, now),
            compare: |a, b| by_age(a.created_unix, b.created_unix),
        },
    ];

    fn tone(&self) -> Tone {
        status_tone(&self.status)
    }

    fn labels(&self) -> Option<&BTreeMap<String, String>> {
        Some(&self.labels)
    }
}

impl PodTable {
    /// What a tail would follow: the rows as shown, so the pods on top come first.
    pub fn log_targets(&self) -> Vec<LogTarget> {
        log_targets(self.rows())
    }

    /// What a tail of the selection would follow.
    pub fn selection_targets(&self) -> Option<Vec<LogTarget>> {
        self.selected()?;
        Some(log_targets(self.selected_object()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui_kit::px;
    use taffrail_core::{PodRow, PodSnapshot};

    use super::{PodTable, status_tone};
    use crate::table::{
        ALL_NAMESPACES, ROW_HEIGHT, SortOrder, Tone, anchored_offset, format_age, format_count,
        sort_rows,
    };

    const NAME: usize = 0;
    const STATUS: usize = 2;
    const NODE: usize = 3;
    const READY: usize = 4;
    const RESTARTS: usize = 5;
    const AGE: usize = 6;

    fn row(name: &str, status: &str, ready: &str, restarts: i32, created: i64) -> Arc<PodRow> {
        Arc::new(PodRow {
            namespace: "shop".into(),
            name: name.into(),
            status: status.into(),
            ready: ready.into(),
            restarts,
            node: "node-1".into(),
            created_unix: Some(created),
            containers: vec!["app".into()],
            ..PodRow::default()
        })
    }

    fn rows() -> Vec<Arc<PodRow>> {
        vec![
            row("a", "Running", "1/1", 0, 300),
            row("b", "CrashLoopBackOff", "0/1", 7, 100),
            row("c", "Pending", "0/2", 2, 200),
            row("d", "Completed", "2/2", 0, 400),
        ]
    }

    fn sorted(sort: Option<(usize, SortOrder)>) -> Vec<String> {
        let mut rows = rows();
        sort_rows(&mut rows, sort);
        rows.iter().map(|row| row.name.clone()).collect()
    }

    fn table(rows: Vec<Arc<PodRow>>) -> PodTable {
        let mut table = PodTable::default();
        table.show(Arc::new(PodSnapshot {
            rows,
            synced: true,
            error: None,
        }));
        table
    }

    fn in_namespace(namespace: &str, name: &str) -> Arc<PodRow> {
        let mut row = Arc::unwrap_or_clone(row(name, "Running", "1/1", 0, 0));
        row.namespace = namespace.into();
        Arc::new(row)
    }

    fn across_namespaces() -> PodTable {
        table(vec![
            in_namespace("payments", "a"),
            in_namespace("payments", "b"),
            in_namespace("shop", "c"),
            in_namespace("web", "d"),
        ])
    }

    fn pod_names(table: &PodTable) -> Vec<&str> {
        table.rows.iter().map(|row| row.name.as_str()).collect()
    }

    #[test]
    fn pods_that_need_attention_lead_by_default() {
        assert_eq!(sorted(None), ["b", "c", "a", "d"]);
    }

    #[test]
    fn numeric_columns_sort_as_numbers() {
        let mut rows = vec![
            row("a", "Running", "9/10", 10, 0),
            row("b", "Running", "10/10", 9, 0),
        ];
        sort_rows(&mut rows, Some((READY, SortOrder::Ascending)));
        assert_eq!(rows[0].name, "a");
        sort_rows(&mut rows, Some((RESTARTS, SortOrder::Ascending)));
        assert_eq!(rows[0].name, "b");
    }

    #[test]
    fn age_sorts_youngest_first_when_ascending() {
        assert_eq!(
            sorted(Some((AGE, SortOrder::Ascending))),
            ["d", "a", "c", "b"]
        );
        assert_eq!(
            sorted(Some((AGE, SortOrder::Descending))),
            ["b", "c", "a", "d"]
        );
    }

    #[test]
    fn descending_keeps_equal_rows_in_name_order() {
        let mut rows = vec![
            row("a", "Running", "1/1", 0, 0),
            row("b", "Running", "1/1", 0, 0),
        ];
        sort_rows(&mut rows, Some((STATUS, SortOrder::Descending)));
        assert_eq!(rows[0].name, "a");
    }

    #[test]
    fn clicking_a_header_cycles_up_down_and_back_to_the_default() {
        let mut table = table(rows());
        table.cycle_sort(AGE);
        assert_eq!(table.sort, Some((AGE, SortOrder::Ascending)));
        table.cycle_sort(AGE);
        assert_eq!(table.sort, Some((AGE, SortOrder::Descending)));
        table.cycle_sort(AGE);
        assert_eq!(table.sort, None);
    }

    #[test]
    fn a_new_sort_column_starts_ascending() {
        let mut table = table(rows());
        table.cycle_sort(AGE);
        table.cycle_sort(AGE);
        table.cycle_sort(READY);
        assert_eq!(table.sort, Some((READY, SortOrder::Ascending)));
    }

    #[test]
    fn a_tail_follows_the_rows_as_shown() {
        let targets = table(rows()).log_targets();
        let pods: Vec<_> = targets.iter().map(|target| target.pod.as_str()).collect();
        assert_eq!(pods, ["b", "c", "a", "d"]);
    }

    #[test]
    fn selection_follows_a_pod_when_its_row_moves() {
        let mut table = table(rows());
        table.select_row(2);
        assert_eq!(table.selected().unwrap().1, "a");

        table.show(Arc::new(PodSnapshot {
            rows: vec![
                row("a", "CrashLoopBackOff", "0/1", 1, 300),
                row("b", "CrashLoopBackOff", "0/1", 7, 100),
            ],
            synced: true,
            error: None,
        }));

        assert_eq!(table.selected_row, Some(0));
        assert_eq!(table.selected().unwrap().1, "a");
    }

    #[test]
    fn selection_survives_a_filter_that_hides_the_pod() {
        let mut table = table(rows());
        table.select_row(0);
        table.set_filter("zzz".into());
        assert_eq!(table.selected_row, None);
        assert_eq!(table.selected().unwrap().1, "b");

        table.set_filter(String::new());
        assert_eq!(table.selected_row, Some(0));
    }

    #[test]
    fn a_namespace_narrows_the_rows_and_the_tail() {
        let mut table = across_namespaces();
        table.set_namespace(Some("payments".into()));
        assert_eq!(pod_names(&table), ["a", "b"]);
        let targets = table.log_targets();
        assert!(targets.iter().all(|target| target.namespace == "payments"));
        assert_eq!(targets.len(), 2);

        table.set_namespace(None);
        assert_eq!(pod_names(&table), ["a", "b", "c", "d"]);
    }

    #[test]
    fn a_namespace_and_a_text_filter_narrow_together() {
        let mut table = across_namespaces();
        table.set_namespace(Some("payments".into()));
        table.set_filter("b".into());
        assert_eq!(pod_names(&table), ["b"]);
    }

    #[test]
    fn the_text_filter_ignores_case_and_spans_namespace_and_name() {
        let mut table = across_namespaces();
        table.set_filter("PAYMENTS/B".into());
        assert_eq!(pod_names(&table), ["b"]);
        table.set_filter(String::new());
        assert_eq!(pod_names(&table), ["a", "b", "c", "d"]);
    }

    #[test]
    fn a_new_snapshot_stays_in_the_chosen_namespace() {
        let mut table = across_namespaces();
        table.set_namespace(Some("shop".into()));
        table.show(Arc::new(PodSnapshot {
            rows: vec![in_namespace("shop", "c"), in_namespace("web", "z")],
            synced: true,
            error: None,
        }));
        assert_eq!(pod_names(&table), ["c"]);
    }

    #[test]
    fn namespace_options_list_each_namespace_once_after_all() {
        assert_eq!(
            across_namespaces().namespace_options(),
            [ALL_NAMESPACES, "payments", "shop", "web"]
        );
    }

    #[test]
    fn namespace_options_are_only_all_for_an_empty_cluster() {
        assert_eq!(table(vec![]).namespace_options(), [ALL_NAMESPACES]);
    }

    #[test]
    fn a_namespace_that_disappears_stays_chosen_and_empty() {
        let mut table = across_namespaces();
        table.set_namespace(Some("shop".into()));
        assert_eq!(table.empty_namespace(), None);

        table.show(Arc::new(PodSnapshot {
            rows: vec![in_namespace("payments", "a"), in_namespace("web", "d")],
            synced: true,
            error: None,
        }));

        assert_eq!(table.namespace(), Some("shop"));
        assert_eq!(table.visible_rows(), 0);
        assert_eq!(table.empty_namespace(), Some("shop"));
        assert_eq!(
            table.namespace_options(),
            [ALL_NAMESPACES, "payments", "shop", "web"]
        );
    }

    #[test]
    fn a_namespace_hides_the_selection_without_losing_it() {
        let mut table = across_namespaces();
        table.select_row(3);
        table.set_namespace(Some("payments".into()));
        assert_eq!(table.selected_row, None);
        assert_eq!(table.selected().unwrap().1, "d");

        table.set_namespace(None);
        assert_eq!(table.selected_row, Some(3));
    }

    #[test]
    fn positions_skip_hidden_columns() {
        let mut table = table(rows());
        table.set_hidden_columns(["Namespace", "Ready"]);
        let shown: Vec<_> = (0..5).map(|position| table.column_at(position)).collect();
        assert_eq!(shown, [NAME, STATUS, NODE, RESTARTS, AGE]);
    }

    #[test]
    fn hidden_columns_read_back_in_table_order() {
        let mut table = table(rows());
        assert!(table.hidden_columns().is_empty());
        table.set_hidden_columns(vec![String::from("Age"), String::from("Node")]);
        assert_eq!(table.hidden_columns(), ["Node", "Age"]);
    }

    #[test]
    fn setting_hidden_columns_shows_the_others_again() {
        let mut table = table(rows());
        table.set_hidden_columns(["Age"]);
        table.set_hidden_columns(["Node"]);
        assert_eq!(table.hidden_columns(), ["Node"]);
    }

    #[test]
    fn the_name_column_cannot_be_hidden() {
        let mut table = table(rows());
        table.set_hidden_columns(["Name"]);
        assert!(table.hidden_columns().is_empty());
    }

    #[test]
    fn an_unknown_column_is_ignored() {
        let mut table = table(rows());
        table.set_hidden_columns(["Cost"]);
        assert!(table.hidden_columns().is_empty());
    }

    #[test]
    fn hiding_the_sorted_column_returns_to_the_default_order() {
        let mut table = table(rows());
        table.cycle_sort(AGE);
        assert_eq!(pod_names(&table), ["d", "a", "c", "b"]);

        table.set_hidden_columns(["Age"]);

        assert_eq!(table.sort, None);
        assert_eq!(pod_names(&table), ["b", "c", "a", "d"]);
    }

    #[test]
    fn hiding_another_column_keeps_the_sort() {
        let mut table = table(rows());
        table.cycle_sort(AGE);
        table.set_hidden_columns(["Node"]);
        assert_eq!(table.sort, Some((AGE, SortOrder::Ascending)));
    }

    #[test]
    fn moving_the_selection_stops_at_both_ends() {
        let mut table = table(rows());
        assert_eq!(table.move_selection(1), Some(0));
        assert_eq!(table.move_selection(-1), Some(0));
        assert_eq!(table.move_selection(10), Some(3));
        assert_eq!(table.move_selection(1), Some(3));
    }

    #[test]
    fn moving_the_selection_in_an_empty_table_does_nothing() {
        assert_eq!(table(vec![]).move_selection(1), None);
    }

    #[test]
    fn the_view_follows_a_row_that_moved_above_it() {
        assert_eq!(
            anchored_offset(px(-300.), 10, 12),
            px(-300.) - ROW_HEIGHT * 2.
        );
        assert_eq!(anchored_offset(px(-300.), 10, 9), px(-300.) + ROW_HEIGHT);
    }

    #[test]
    fn the_top_of_the_list_stays_put_so_new_rows_show() {
        assert_eq!(anchored_offset(px(0.), 0, 5), px(0.));
    }

    #[test]
    fn counts_group_thousands() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1,000");
        assert_eq!(format_count(5_500), "5,500");
        assert_eq!(format_count(1_234_567), "1,234,567");
    }

    #[test]
    fn status_tone_flags_anything_unknown_as_failed() {
        assert_eq!(status_tone("Running"), Tone::Healthy);
        assert_eq!(status_tone("Completed"), Tone::Neutral);
        assert_eq!(status_tone("Terminating"), Tone::Degraded);
        assert_eq!(status_tone("CrashLoopBackOff"), Tone::Failed);
        assert_eq!(status_tone("Init:Error"), Tone::Failed);
    }

    #[test]
    fn age_uses_the_largest_whole_unit() {
        assert_eq!(format_age(0), "0s");
        assert_eq!(format_age(59), "59s");
        assert_eq!(format_age(60), "1m");
        assert_eq!(format_age(3_599), "59m");
        assert_eq!(format_age(3_600), "1h");
        assert_eq!(format_age(86_399), "23h");
        assert_eq!(format_age(86_400), "1d");
    }

    #[test]
    fn age_treats_a_future_time_as_zero() {
        assert_eq!(format_age(-5), "0s");
    }
}
