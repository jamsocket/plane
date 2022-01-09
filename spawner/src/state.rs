use crate::pod_id::PodId;
use k8s_openapi::api::core::v1::Pod;
use kube::runtime::reflector::{ObjectRef, Store};

#[derive(Clone)]
pub struct SpawnerSettings {
    pub application_image: String,
    pub sidecar_image: Option<String>,
    pub base_url: String,
    pub application_port: u16,
    pub sidecar_port: u16,
    pub namespace: String,
    pub cleanup_frequency_seconds: u32,
}

#[derive(Clone)]
pub struct SpawnerState {
    pub settings: SpawnerSettings,
    pub store: Store<Pod>,
}

impl SpawnerSettings {
    pub fn url_for(&self, pod_id: &PodId) -> String {
        format!("{}/{}/", self.base_url, pod_id.name())
    }

    #[allow(unused)]
    pub fn get_object_ref(&self, pod_id: &PodId) -> ObjectRef<Pod> {
        ObjectRef::new(&pod_id.prefixed_name()).within(&self.namespace)
    }
}
