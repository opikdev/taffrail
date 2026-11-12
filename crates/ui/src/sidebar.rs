use gpui_kit::IntoElement;
use gpui_kit::assets::IconName;
use gpui_kit::component::sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::{ActiveTheme, Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::palette::ShowView;
use crate::table::format_count;
use crate::views::View;

pub const SIDEBAR_WIDTH: f32 = 216.;

const GROUPS: [(&str, &[(&str, IconName)]); 4] = [
    (
        "Workloads",
        &[
            ("Pods", IconName::Box),
            ("Deployments", IconName::Layers),
            ("StatefulSets", IconName::Database),
            ("DaemonSets", IconName::Boxes),
            ("Jobs", IconName::Play),
            ("CronJobs", IconName::Clock),
        ],
    ),
    (
        "Network",
        &[
            ("Services", IconName::Network),
            ("Ingresses", IconName::Globe),
        ],
    ),
    (
        "Config",
        &[
            ("ConfigMaps", IconName::FileText),
            ("Secrets", IconName::KeyRound),
        ],
    ),
    (
        "Cluster",
        &[
            ("Nodes", IconName::Server),
            ("Namespaces", IconName::Folder),
            ("Events", IconName::Activity),
        ],
    ),
];

/// Every resource the sidebar can list, in order, whether or not it has a view yet.
#[cfg(test)]
pub fn sidebar_names() -> impl Iterator<Item = &'static str> {
    GROUPS
        .into_iter()
        .flat_map(|(_, resources)| resources.iter().map(|(name, _)| *name))
}

/// What the sidebar shows for one view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidebarItem {
    pub view: View,
    pub active: bool,
    /// How many objects its watch lists, once it has.
    pub count: Option<usize>,
    /// Whether the API refused to list them, which a lock marks.
    pub forbidden: bool,
}

/// The groups the sidebar shows, with each resource that has a view. Resources without one
/// are hidden until their view lands, and so is a group left empty.
fn shown_groups(
    items: &[SidebarItem],
) -> impl Iterator<Item = (&'static str, Vec<(&'static str, IconName, SidebarItem)>)> {
    GROUPS.into_iter().filter_map(|(group, resources)| {
        let shown: Vec<_> = resources
            .iter()
            .filter_map(|&(name, icon)| {
                let item = items.iter().find(|item| item.view.name() == name)?;
                Some((name, icon, *item))
            })
            .collect();
        (!shown.is_empty()).then_some((group, shown))
    })
}

/// The resource navigation.
pub fn resource_sidebar(
    items: &[SidebarItem],
    collapsed: bool,
    footer: impl IntoElement,
) -> impl IntoElement {
    let sidebar = Sidebar::new("resources")
        .w(px(SIDEBAR_WIDTH))
        .collapsed(collapsed)
        .border_r_0()
        .footer(footer)
        .children(shown_groups(items).map(|(group, resources)| {
            SidebarGroup::new(group).child(SidebarMenu::new().gap_0p5().mb_3().children(
                resources.into_iter().map(|(name, icon, item)| {
                    let SidebarItem {
                        view,
                        active,
                        count,
                        forbidden,
                    } = item;
                    SidebarMenuItem::new(name)
                        .icon(Icon::new(icon))
                        .active(active)
                        .accessibility_label(item_label(name, count, forbidden))
                        .on_click(move |_, window, cx| {
                            window.dispatch_action(Box::new(ShowView { view }), cx);
                        })
                        .suffix(move |_, cx| {
                            let muted = cx.theme().muted_foreground;
                            h_flex()
                                .text_xs()
                                .text_color(muted)
                                .when(forbidden, |suffix| {
                                    suffix.child(Icon::new(IconName::Lock).size_3())
                                })
                                .when_some(count.filter(|_| !forbidden), |suffix, count| {
                                    suffix.child(format_count(count))
                                })
                        })
                }),
            ))
        }));
    // The items are tree items, so a screen reader wants a tree around them.
    div()
        .id("resource-tree")
        .test_support()
        .role(Role::Tree)
        .aria_label("Resources")
        .h_full()
        .flex_shrink_0()
        .child(sidebar)
}

/// What a screen reader reads for a sidebar item, which shows its count or lock beside it.
fn item_label(name: &str, count: Option<usize>, forbidden: bool) -> String {
    match (forbidden, count) {
        (true, _) => format!("{name}, no permission"),
        (false, Some(count)) => format!("{name}, {}", format_count(count)),
        (false, None) => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{SidebarItem, item_label, shown_groups};
    use crate::views::View;

    #[test]
    fn an_item_reads_its_count_or_why_it_has_none() {
        assert_eq!(item_label("Pods", Some(1_204), false), "Pods, 1,204");
        assert_eq!(item_label("Pods", None, false), "Pods");
        assert_eq!(
            item_label("Deployments", Some(3), true),
            "Deployments, no permission"
        );
    }

    #[test]
    fn hides_resources_without_a_view_and_groups_left_empty() {
        let items = View::ALL.map(|view| SidebarItem {
            view,
            active: false,
            count: None,
            forbidden: false,
        });

        let shown: Vec<_> = shown_groups(&items)
            .map(|(group, resources)| {
                let names: Vec<_> = resources.into_iter().map(|(name, ..)| name).collect();
                (group, names)
            })
            .collect();

        assert_eq!(
            shown,
            [
                (
                    "Workloads",
                    vec!["Pods", "Deployments", "StatefulSets", "DaemonSets", "Jobs"]
                ),
                ("Cluster", vec!["Nodes", "Namespaces", "Events"]),
            ]
        );
    }
}
