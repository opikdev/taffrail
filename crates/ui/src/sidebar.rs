use gpui_kit::IntoElement;
use gpui_kit::assets::IconName;
use gpui_kit::component::sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::prelude::*;
use gpui_kit::*;

pub const SIDEBAR_WIDTH: f32 = 216.;

const PODS: &str = "Pods";
const GROUPS: [(&str, &[(&str, IconName)]); 4] = [
    (
        "Workloads",
        &[
            (PODS, IconName::Box),
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

/// The resource navigation. Only Pods exists so far, so the rest are disabled.
pub fn resource_sidebar(
    pod_count: Option<usize>,
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
                    if name != PODS {
                        return item.disable(true);
                    }
                    item.active(true).suffix(move |_, cx| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .when_some(pod_count, |count, pods| {
                                count.child(crate::pods::format_count(pods))
                            })
                    })
                }),
            ))
        }))
}
