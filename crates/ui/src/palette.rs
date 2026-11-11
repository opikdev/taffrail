use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::command::{CommandGroup, CommandItem};
use gpui_kit::component::{ActiveTheme, Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use taffrail_core::{ObjectLink, PodKey, PodRow, Row, Selector};

use crate::table::urgency;
use crate::views::View;
use crate::windows::NewWindow;

/// How many pods a search lists, so that large clusters stay quick to search.
const MAX_POD_RESULTS: usize = 20;

const KEY_WIDTH: f32 = 88.;

actions!(
    taffrail,
    [
        OpenPalette,
        ShowShortcuts,
        ShowAbout,
        FocusFilter,
        FocusLogFilter,
        OpenSelected,
        TailSelection,
        TailWorkload,
        ShowSummary,
        ShowEvents,
        ShowYaml,
        ToggleFollow,
        NextError,
        PreviousError,
        CopyLines,
        ExportLines,
        ToggleSidebar,
        ReloadContexts,
        CloseWindow,
        ShowPods,
        ClearSelector,
        GoBack,
    ]
);

/// Opens a resource view.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = taffrail, no_json)]
pub struct ShowView {
    pub view: View,
}

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = taffrail, no_json)]
pub struct SwitchContext {
    pub name: SharedString,
}

/// `None` shows every namespace.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = taffrail, no_json)]
pub struct SwitchNamespace {
    pub namespace: Option<SharedString>,
}

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = taffrail, no_json)]
pub struct OpenPod {
    pub key: PodKey,
}

/// Opens the view of a linked object's kind and selects the object.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = taffrail, no_json)]
pub struct OpenLink {
    pub link: ObjectLink,
}

/// Follows the pods that a label selector matches, in the chosen namespace.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = taffrail, no_json)]
pub struct TailSelector {
    pub selector: SharedString,
}

/// Every key the app answers to, for the shortcut sheet.
pub const SHORTCUTS: [(&str, &str); 23] = [
    (MODIFIER_K, "Open the command palette"),
    ("1 – 6", "Go to a view, in sidebar order"),
    ("/", "Filter the current view"),
    ("f", "Filter log lines"),
    ("j  k", "Move through rows"),
    ("Enter", "Open details"),
    ("l", "Tail logs for the selection"),
    ("w", "Tail the selected workload"),
    ("p", "Show the selected workload's pods"),
    (MODIFIER_BACK, "Go back after following a link"),
    ("y", "Show YAML"),
    ("e", "Show events"),
    ("s", "Show summary"),
    ("Space", "Pause or follow logs"),
    ("n  N", "Next or previous error"),
    ("←  →", "Move along the log timeline"),
    (MODIFIER_C, "Copy selected log lines"),
    ("Esc", "Clear the filter, then close the panel"),
    (MODIFIER_B, "Show or hide the sidebar"),
    (MODIFIER_N, "New window"),
    (MODIFIER_W, "Close window"),
    (MODIFIER_Q, "Quit"),
    ("?", "Show all shortcuts"),
];

const MODIFIER_K: &str = if cfg!(target_os = "macos") {
    "⌘K"
} else {
    "Ctrl+K"
};
const MODIFIER_BACK: &str = if cfg!(target_os = "macos") {
    "⌘["
} else {
    "Alt+←"
};
const MODIFIER_C: &str = if cfg!(target_os = "macos") {
    "⌘C"
} else {
    "Ctrl+C"
};
const MODIFIER_B: &str = if cfg!(target_os = "macos") {
    "⌘B"
} else {
    "Ctrl+B"
};
const MODIFIER_N: &str = if cfg!(target_os = "macos") {
    "⌘N"
} else {
    "Ctrl+N"
};
const MODIFIER_W: &str = if cfg!(target_os = "macos") {
    "⌘W"
} else {
    "Ctrl+W"
};
const MODIFIER_Q: &str = if cfg!(target_os = "macos") {
    "⌘Q"
} else {
    "Ctrl+Q"
};

/// The label for the palette's key, shown in the title bar.
pub const PALETTE_KEY: &str = MODIFIER_K;

/// What the palette can offer at this moment.
pub struct PaletteSource<'a> {
    pub query: &'a str,
    pub view: View,
    pub rows: &'a [Arc<PodRow>],
    pub contexts: Vec<String>,
    pub current_context: Option<String>,
    pub namespaces: Vec<&'a str>,
    pub namespace: Option<&'a str>,
    pub has_selection: bool,
    /// Whether the selected object has pods to tail, as a workload has.
    pub has_follow: bool,
    /// Whether the selected object has pods to show, as a workload or a node has.
    pub has_pods: bool,
    pub has_tail: bool,
    pub connected: bool,
    /// Where **Back** returns to, such as `pod api-1`, after following a link.
    pub back: Option<String>,
}

/// The selector a search spells, such as `app=api` or `tier in (web)`. A bare word
/// would parse as "has this label", so it only counts with an operator.
pub fn typed_selector(query: &str) -> Option<Selector> {
    let operator = ["=", "!", " in ", " notin "]
        .iter()
        .any(|operator| query.contains(operator));
    operator.then(|| Selector::parse(query).ok()).flatten()
}

/// Up to `MAX_POD_RESULTS` pods matching `query`, with those that need attention first.
/// Nothing is listed until a search starts.
pub fn matching_pods<'a>(rows: &'a [Arc<PodRow>], query: &str) -> Vec<&'a PodRow> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut pods: Vec<_> = rows
        .iter()
        .filter(|row| row.matches(query))
        .map(Arc::as_ref)
        .collect();
    pods.sort_by_cached_key(|row| urgency::<PodRow>(row));
    pods.truncate(MAX_POD_RESULTS);
    pods
}

fn command(label: &'static str, icon: IconName, action: impl Action) -> CommandItem {
    CommandItem::new()
        .label(label)
        .icon(Icon::new(icon))
        .action(action.boxed_clone())
}

/// A pod row names its namespace too, since two namespaces can hold pods with the same name.
fn pod_item(row: &PodRow) -> CommandItem {
    let name: SharedString = row.name.clone().into();
    let namespace: SharedString = row.namespace.clone().into();
    CommandItem::new()
        .label(name.clone())
        .keywords([row.namespace.clone(), row.status.clone()])
        .action(Box::new(OpenPod { key: row.key() }))
        .child(move |_, cx| {
            let muted = cx.theme().muted_foreground;
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .items_center()
                .child(Icon::new(IconName::Box).size_4().text_color(muted))
                .child(div().min_w_0().truncate().child(name.clone()))
                .child(
                    div()
                        .ml_auto()
                        .flex_shrink_0()
                        .text_color(muted)
                        .child(namespace.clone()),
                )
        })
}

/// The palette's entries. Each one dispatches an action, so a command behaves the same from
/// the palette, a key or a button.
pub fn groups(source: PaletteSource) -> Vec<CommandGroup> {
    let filter_label = format!("Filter {}", source.view.name().to_lowercase());
    let mut actions = CommandGroup::new().label("Commands").item(
        CommandItem::new()
            .label(filter_label)
            .icon(Icon::new(IconName::Search))
            .action(Box::new(FocusFilter)),
    );
    if source.connected && source.view == View::Pods {
        actions = actions.item(command("Tail logs", IconName::ScrollText, TailSelection));
    }
    let on_other_view = source.connected && source.has_selection && source.view != View::Pods;
    if on_other_view && source.has_follow {
        actions = actions.item(
            command("Tail its pods", IconName::ScrollText, TailWorkload)
                .keywords(["follow", "logs", "workload", "rollout"]),
        );
    }
    if on_other_view && source.has_pods {
        actions = actions.item(
            command("Show its pods", IconName::Box, ShowPods)
                .keywords(["selector", "workload", "node"]),
        );
    }
    if source.connected && source.has_selection && source.view == View::Pods {
        actions = actions.item(
            command("Tail its workload", IconName::ScrollText, TailWorkload).keywords([
                "follow",
                "deployment",
                "statefulset",
                "daemonset",
                "job",
                "replicaset",
                "rollout",
            ]),
        );
    }
    if source.connected
        && let Some(selector) = typed_selector(source.query)
    {
        let label = match source.namespace {
            Some(namespace) => format!("Tail pods matching {selector} in {namespace}"),
            None => format!("Tail pods matching {selector}"),
        };
        actions = actions.item(
            CommandItem::new()
                .label(label)
                .icon(Icon::new(IconName::ScrollText))
                .keywords([source.query.to_owned()])
                .action(Box::new(TailSelector {
                    selector: selector.to_string().into(),
                })),
        );
    }
    if source.has_tail {
        actions = actions
            .item(
                command("Filter log lines", IconName::Search, FocusLogFilter)
                    .keywords(["severity", "regex", "json"]),
            )
            .item(command(
                "Pause or follow logs",
                IconName::Pause,
                ToggleFollow,
            ))
            .item(command("Next error", IconName::ArrowDown, NextError).keywords(["jump"]))
            .item(
                command("Copy selected lines", IconName::Copy, CopyLines)
                    .keywords(["clipboard", "log"]),
            )
            .item(
                command("Export lines…", IconName::Download, ExportLines)
                    .keywords(["save", "file", "download", "log"]),
            )
            .item(command("Previous error", IconName::ArrowUp, PreviousError).keywords(["jump"]));
    }
    if source.has_selection {
        actions = actions
            .item(command("Show summary", IconName::Info, ShowSummary))
            .item(command("Show events", IconName::Activity, ShowEvents))
            .item(command("Show YAML", IconName::FileText, ShowYaml));
    }
    if let Some(back) = source.back {
        actions = actions.item(
            CommandItem::new()
                .label(format!("Back to {back}"))
                .icon(Icon::new(IconName::ArrowLeft))
                .keywords(["back", "return", "link"])
                .action(Box::new(GoBack)),
        );
    }
    for view in View::ALL {
        actions = actions.item(
            CommandItem::new()
                .label(format!("Go to {}", view.name()))
                .icon(Icon::new(view.icon()))
                .keywords(["view", "resource", view.group()])
                .checked(view == source.view)
                .action(Box::new(ShowView { view })),
        );
    }
    actions = actions
        .item(command(
            "Show or hide the sidebar",
            IconName::PanelLeft,
            ToggleSidebar,
        ))
        .item(command("New window", IconName::Plus, NewWindow))
        .item(command(
            "Reload contexts",
            IconName::RefreshCw,
            ReloadContexts,
        ))
        .item(command(
            "Show all shortcuts",
            IconName::Keyboard,
            ShowShortcuts,
        ))
        .item(command("About Taffrail", IconName::Info, ShowAbout));

    let pods = matching_pods(source.rows, source.query);
    let mut groups = Vec::new();
    if !pods.is_empty() {
        groups.push(
            CommandGroup::new()
                .label("Pods")
                .items(pods.into_iter().map(pod_item)),
        );
    }
    groups.push(actions);
    groups.push(
        CommandGroup::new()
            .label("Contexts")
            .items(source.contexts.into_iter().map(|name| {
                let current = source.current_context.as_ref() == Some(&name);
                CommandItem::new()
                    .label(name.clone())
                    .icon(Icon::new(IconName::Server))
                    .keywords(["context", "cluster"])
                    .checked(current)
                    .action(Box::new(SwitchContext { name: name.into() }))
            })),
    );
    let chosen = source.namespace;
    let all = CommandItem::new()
        .label("All namespaces")
        .icon(Icon::new(IconName::Folder))
        .checked(chosen.is_none())
        .action(Box::new(SwitchNamespace { namespace: None }));
    groups.push(CommandGroup::new().label("Namespaces").item(all).items(
        source.namespaces.into_iter().map(|namespace| {
            CommandItem::new()
                .label(namespace.to_owned())
                .icon(Icon::new(IconName::Folder))
                .keywords(["namespace"])
                .checked(chosen == Some(namespace))
                .action(Box::new(SwitchNamespace {
                    namespace: Some(namespace.to_owned().into()),
                }))
        }),
    ));
    groups
}

/// The sheet of keys that `?` opens.
pub fn render_shortcuts(cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    v_flex()
        .id("shortcuts")
        .test_support()
        .role(Role::Dialog)
        .aria_label("Keyboard shortcuts")
        .gap_1p5()
        .p_4()
        .text_sm()
        .child(
            div()
                .pb_1()
                .font_weight(FontWeight::MEDIUM)
                .child("Keyboard shortcuts"),
        )
        .children(SHORTCUTS.map(|(key, action)| {
            h_flex()
                .gap_3()
                .child(
                    div()
                        .w(px(KEY_WIDTH))
                        .flex_shrink_0()
                        .font_family(theme.mono_font_family.clone())
                        .text_size(theme.mono_font_size)
                        .text_color(theme.muted_foreground)
                        .child(key),
                )
                .child(action)
        }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use taffrail_core::PodRow;

    use super::{MAX_POD_RESULTS, matching_pods, typed_selector};

    fn row(namespace: &str, name: &str) -> Arc<PodRow> {
        with_status(namespace, name, "Running")
    }

    fn with_status(namespace: &str, name: &str, status: &str) -> Arc<PodRow> {
        Arc::new(PodRow {
            namespace: namespace.into(),
            name: name.into(),
            status: status.into(),
            ready: "1/1".into(),
            restarts: 0,
            node: "node-1".into(),
            containers: vec!["app".into()],
            ..PodRow::default()
        })
    }

    #[test]
    fn a_search_with_an_operator_is_a_selector() {
        let typed = |query| typed_selector(query).map(|selector| selector.to_string());
        assert_eq!(typed("app=checkout").as_deref(), Some("app=checkout"));
        assert_eq!(
            typed("tier in (web, db)").as_deref(),
            Some("tier in (db,web)")
        );
        assert_eq!(typed("!canary").as_deref(), Some("!canary"));
    }

    #[test]
    fn a_plain_search_is_not_a_selector() {
        assert!(typed_selector("checkout").is_none());
        assert!(typed_selector("checkout api").is_none());
        assert!(typed_selector("app=").is_some());
        assert!(typed_selector("a=b c").is_none());
    }

    #[test]
    fn nothing_is_listed_before_a_search_starts() {
        assert!(matching_pods(&[row("shop", "api")], "").is_empty());
    }

    #[test]
    fn search_covers_namespace_and_name() {
        let rows = [row("shop", "api"), row("web", "front")];
        let found = matching_pods(&rows, "web/fr");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "front");
    }

    #[test]
    fn pods_that_need_attention_lead_the_results() {
        let rows = [
            row("shop", "api-a"),
            with_status("shop", "api-b", "CrashLoopBackOff"),
        ];
        assert_eq!(matching_pods(&rows, "api")[0].name, "api-b");
    }

    #[test]
    fn a_failing_pod_is_not_cut_off_by_the_limit() {
        let mut rows: Vec<_> = (0..MAX_POD_RESULTS + 5)
            .map(|ix| row("shop", &format!("pod-{ix}")))
            .collect();
        rows.push(with_status("shop", "pod-last", "Error"));
        assert_eq!(matching_pods(&rows, "pod")[0].name, "pod-last");
    }

    #[test]
    fn results_stop_at_the_limit() {
        let rows: Vec<_> = (0..MAX_POD_RESULTS + 5)
            .map(|ix| row("shop", &format!("pod-{ix}")))
            .collect();
        assert_eq!(matching_pods(&rows, "pod").len(), MAX_POD_RESULTS);
    }
}
