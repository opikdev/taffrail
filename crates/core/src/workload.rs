//! What the views of workloads that run pods, such as StatefulSets, read the same way.

use k8s_openapi::api::core::v1::PodTemplateSpec;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;

use crate::deployments::Condition;
use crate::selector::Selector;

pub(crate) fn created_unix(metadata: &ObjectMeta) -> Option<i64> {
    metadata
        .creation_timestamp
        .as_ref()
        .map(|created| created.0.as_second())
}

pub(crate) fn labels(metadata: &ObjectMeta) -> Vec<(String, String)> {
    metadata
        .labels
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// Each container of a pod template, and its image.
pub(crate) fn images(template: &PodTemplateSpec) -> Vec<(String, String)> {
    template
        .spec
        .iter()
        .flat_map(|pod| &pod.containers)
        .map(|container| {
            let image = container.image.clone().unwrap_or_default();
            (container.name.clone(), image)
        })
        .collect()
}

/// A selector as text, or why Taffrail can't read it.
pub(crate) fn describe_selector(selector: &LabelSelector) -> String {
    match Selector::from_label_selector(selector) {
        Ok(selector) => selector.to_string(),
        Err(error) => error,
    }
}

/// A count or a percentage, such as `1` or `25%`, or `default` when unset.
pub(crate) fn amount(value: Option<&IntOrString>, default: &str) -> String {
    match value {
        Some(IntOrString::Int(count)) => count.to_string(),
        Some(IntOrString::String(text)) => text.clone(),
        None => default.to_owned(),
    }
}

pub(crate) fn condition(
    kind: &Option<String>,
    status: &Option<String>,
    reason: &Option<String>,
    message: &Option<String>,
) -> Condition {
    Condition {
        kind: kind.clone().unwrap_or_default(),
        status: status.clone().unwrap_or_default(),
        reason: reason.clone().unwrap_or_default(),
        message: message.clone().unwrap_or_default(),
    }
}
