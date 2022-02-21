use futures::Stream;
use k8s_openapi::api::core::v1::Event as KubeEventResource;
use kube::{
    api::ListParams,
    runtime::watcher::{watcher, Error as KubeWatcherError, Event as KubeWatcherEvent},
    Api, Client, Error as KubeError,
};
use tokio_stream::StreamExt;

fn list_params_for_resource(resource_name: &str) -> ListParams {
    ListParams {
        field_selector: Some(format!(
            "involvedObject.name={},involvedObject.kind=SessionLivedBackend",
            resource_name
        )),
        ..ListParams::default()
    }
}

pub async fn past_events(
    client: Client,
    resource_name: &str,
    namespace: &str,
) -> Result<Vec<KubeEventResource>, KubeError> {
    let api = Api::<KubeEventResource>::namespaced(client, &namespace);

    let list_params = list_params_for_resource(resource_name);

    Ok(api.list(&list_params).await?.items)
}

pub async fn event_stream(
    client: Client,
    resource_name: &str,
    namespace: &str,
) -> Result<impl Stream<Item = Result<KubeEventResource, KubeWatcherError>>, KubeError> {
    let api = Api::<KubeEventResource>::namespaced(client, &namespace);

    let list_params = list_params_for_resource(resource_name);

    let event_stream = watcher(api.clone(), list_params.clone()).filter_map(|event| match event {
        Ok(event) => match event {
            KubeWatcherEvent::Applied(event) => Some(Ok(event)),
            _ => None,
        },
        Err(err) => Some(Err(err)),
    });

    let mut pre_events = api.list(&list_params).await?.items;
    pre_events.sort_by_key(|event| event.event_time.clone());

    let pre_events = futures::stream::iter(pre_events.into_iter().map(|d| Ok(d)));

    Ok(pre_events.merge(event_stream))
}
