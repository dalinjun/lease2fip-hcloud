use futures::TryStreamExt;
use hcloud::{
	apis::{configuration::Configuration, floating_ips_api, servers_api},
	models::AssignFloatingIpToServerRequest,
};
use k8s_openapi::api::coordination::v1::Lease;
use kube::{
	Api, Client,
	runtime::{WatchStreamExt, watcher::Config},
};
use serde::Deserialize;
use std::{collections::HashMap, env, str::FromStr, sync::Arc};
use tokio::sync::RwLock;
use tracing::*;

#[derive(Debug, Deserialize)]
struct ConfigRoot {
	floating_ips: HashMap<String, TargetServiceConfig>,
	log_level: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TargetServiceConfig {
	service_name: String,
	service_namespace: String,
}

const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const CONFIG_FILENAME: &str = "config.yaml";
const GIT_COMMIT_HASH: &str = env!("GIT_COMMIT_HASH");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let config: ConfigRoot = config::Config::builder()
		.add_source(config::File::with_name(CONFIG_FILENAME))
		.build()?
		.try_deserialize()?;

	let log_level = config.log_level.unwrap_or("info".to_string());

	let Ok(log_level) = Level::from_str(&log_level) else {
		panic!("invalid log level {}", log_level)
	};

	tracing_subscriber::fmt().with_max_level(log_level).init();

	info!("release {}-{}", CARGO_PKG_VERSION, GIT_COMMIT_HASH);

	let current_namespace = option_env!("POD_NAMESPACE").unwrap_or_else(|| "default");

	let Some(hcloud_token) = option_env!("HCLOUD_TOKEN") else {
		error!("HCLOUD_TOKEN environment variable is required");

		std::process::exit(1);
	};

	let mut hcloud_config = Configuration::new();
	hcloud_config.bearer_access_token = Some(hcloud_token.to_string());

	let k8s_client = Client::try_default().await?;

	info!("using namespace: {}", current_namespace);

	let api: Api<Lease> = Api::namespaced(k8s_client.clone(), current_namespace);

	let lease_to_fip_name_mapping: Arc<HashMap<String, String>> = Arc::new(
		config
			.floating_ips
			.iter()
			.map(|fip| {
				(
					format!(
						"cilium-l2announce-{}-{}",
						fip.1.service_namespace, fip.1.service_name
					),
					fip.0.clone(),
				)
			})
			.collect(),
	);

	let fip_name_to_previous_holder_identity_mapping = Arc::new(RwLock::new(HashMap::new()));

	info!("starting watcher");

	kube::runtime::watcher(api, Config::default())
		.applied_objects()
		.default_backoff()
		.try_for_each(|lease| {
			handle_update(
				fip_name_to_previous_holder_identity_mapping.clone(),
				Arc::new(hcloud_config.clone()),
				lease,
				lease_to_fip_name_mapping.clone(),
			)
		})
		.await?;

	Ok(())
}

async fn handle_update(
	fip_name_to_previous_holder_identity_mapping: Arc<RwLock<HashMap<String, String>>>,
	hcloud_config: Arc<Configuration>,
	lease: Lease,
	lease_to_fip_name_mapping: Arc<HashMap<String, String>>,
) -> Result<(), kube::runtime::watcher::Error> {
	let holder_identity = lease.spec.unwrap().holder_identity.unwrap();
	let lease_name = lease.metadata.name.as_deref().unwrap();

	let Some(fip_name) = lease_to_fip_name_mapping.get(lease_name).cloned() else {
		debug!(
			"skipping lease '{}' which is not mapped to a floating ip",
			lease_name
		);

		return Ok(());
	};

	if matches!(fip_name_to_previous_holder_identity_mapping.read().await.get(&fip_name), Some(previous_holder_identity) if previous_holder_identity == &holder_identity)
	{
		debug!(
			"lease '{}' is already held by node '{}', skipping",
			lease_name, holder_identity
		);

		return Ok(());
	}

	let Ok(floating_ips_api_response) = floating_ips_api::list_floating_ips(
		&hcloud_config,
		floating_ips_api::ListFloatingIpsParams {
			name: Some(fip_name.clone()),
			..Default::default()
		},
	)
	.await
	.inspect_err(|e| {
		error!("failed to list floating ips: {}", e);
	}) else {
		return Ok(());
	};

	if floating_ips_api_response.floating_ips.is_empty() {
		warn!("floating ip '{}' not found", fip_name);

		return Ok(());
	}

	let fip_id = floating_ips_api_response.floating_ips.first().unwrap().id;

	let Ok(servers_api_response) = servers_api::list_servers(
		&hcloud_config,
		servers_api::ListServersParams {
			name: Some(holder_identity.clone()),
			..Default::default()
		},
	)
	.await
	.inspect_err(|e| {
		error!("failed to list servers: {}", e);
	}) else {
		return Ok(());
	};

	if servers_api_response.servers.is_empty() {
		warn!("server '{}' not found", holder_identity);

		return Ok(());
	}

	let server = servers_api_response.servers.first().unwrap();

	if !server.public_net.floating_ips.contains(&fip_id) {
		info!(
			"assigning floating ip '{}' to server '{}'",
			fip_name, holder_identity
		);

		if let Err(e) = floating_ips_api::assign_floating_ip_to_server(
			&hcloud_config,
			floating_ips_api::AssignFloatingIpToServerParams {
				id: fip_id,
				assign_floating_ip_to_server_request: Some(AssignFloatingIpToServerRequest {
					server: Some(server.id),
				}),
			},
		)
		.await
		{
			error!("failed to assign floating ip: {}", e);

			return Ok(());
		}
	}

	fip_name_to_previous_holder_identity_mapping
		.write()
		.await
		.insert(fip_name.clone(), holder_identity);

	Ok(())
}
