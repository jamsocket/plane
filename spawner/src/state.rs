use crate::{hashutil::hash_key, name_generator::NameGenerator};
use k8s_openapi::api::core::v1::Pod;
use kube::runtime::reflector::{ObjectRef, Store};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct SpawnerSettings {
    pub application_image: String,
    pub sidecar_image: Option<String>,
    pub base_url: String,
    pub application_port: u16,
    pub sidecar_port: u16,
    pub namespace: String,
    pub cleanup_frequency_seconds: u32,
    pub nginx_internal_path: Option<String>,
}

#[derive(Clone)]
pub struct SpawnerState {
    pub name_generator: Arc<Mutex<NameGenerator>>,
    pub settings: SpawnerSettings,
    pub store: Store<Pod>,
}

impl SpawnerSettings {
    pub fn url_for(&self, key: &str, name: &str) -> String {
        if self.nginx_internal_path.is_some() {
            format!("{}/{}/", self.base_url, key)
        } else {
            format!("{}/{}/", self.base_url, name)
        }
    }

    pub fn get_object_ref(&self, key: &str) -> ObjectRef<Pod> {
        let hash = hash_key(&key);

        ObjectRef::new(&hash).within(&self.namespace)
    }
}
