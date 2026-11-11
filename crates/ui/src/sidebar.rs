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

/// Every resource the sidebar lists, in order, whether or not it has a view yet.
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

/// The resource navigation. Resources without a view yet are disabled.
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
        .children(GROUPS.map(|(group, resources)| {
            SidebarGroup::new(group).child(SidebarMenu::new().gap_0p5().mb_3().children(
                resources.iter().map(|&(name, icon)| {
                    let item = SidebarMenuItem::new(name).icon(Icon::new(icon));
                    let Some(&SidebarItem {
                        view,
                        active,
                        count,
                        forbidden,
                    }) = items.iter().find(|item| item.view.name() == name)
                    else {
                        return item
                            .disable(true)
                            .accessibility_label(item_label(name, None, false, false));
                    };
                    item.active(active)
                        .accessibility_label(item_label(name, count, forbidden, true))
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
fn item_label(name: &str, count: Option<usize>, forbidden: bool, has_view: bool) -> String {
    match (has_view, forbidden, count) {
        (false, _, _) => format!("{name}, not available yet"),
        (true, true, _) => format!("{name}, no permission"),
        (true, false, Some(count)) => format!("{name}, {}", format_count(count)),
        (true, false, None) => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::item_label;

    #[test]
    fn an_item_reads_its_count_or_why_it_has_none() {
        assert_eq!(item_label("Pods", Some(1_204), false, true), "Pods, 1,204");
        assert_eq!(item_label("Pods", None, false, true), "Pods");
        assert_eq!(
            item_label("Deployments", Some(3), true, true),
            "Deployments, no permission"
        );
        assert_eq!(
            item_label("CronJobs", None, false, false),
            "CronJobs, not available yet"
        );
    }
}
