use crate::watch::ObjectKey;

/// An object that another names, such as a pod's Node or the Deployment behind it. The app
/// opens it in the view for its kind, if there is one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObjectLink {
    /// The Kubernetes kind, such as `Deployment`.
    pub kind: String,
    /// Empty for a cluster-scoped kind, such as `Node`.
    pub namespace: String,
    pub name: String,
}

impl ObjectLink {
    pub fn new(
        kind: impl Into<String>,
        namespace: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            namespace: namespace.into(),
            name: name.into(),
        }
    }

    /// The key its view selects it by.
    pub fn key(&self) -> ObjectKey {
        (self.namespace.clone(), self.name.clone())
    }

    /// The object as kubectl names it, such as `deployment/api`.
    pub fn label(&self) -> String {
        format!("{}/{}", self.kind.to_lowercase(), self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::ObjectLink;

    #[test]
    fn names_the_object_as_kubectl_does() {
        let link = ObjectLink::new("StatefulSet", "shop", "db");
        assert_eq!(link.label(), "statefulset/db");
        assert_eq!(link.key(), ("shop".to_owned(), "db".to_owned()));
    }
}
