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
) -> Sidebar<SidebarGroup<SidebarMenu>> {
    Sidebar::new("resources")
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
                        return item.disable(true);
                    };
                    let label = match forbidden {
                        true => format!("{name}, no permission"),
                        false => name.to_owned(),
                    };
                    item.active(active)
                        .accessibility_label(label)
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
        }))
}
