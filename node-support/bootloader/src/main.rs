//! Bootloader process; manages the node's startup
#![deny(missing_docs)]
#![deny(clippy::missing_docs_in_private_items)]
#![deny(unsafe_code)]
#![deny(clippy::needless_pass_by_value)]
#![deny(clippy::needless_pass_by_ref_mut)]
#![allow(incomplete_features)]

use std::{collections::HashMap, fmt::Debug, path::Path, str::FromStr};

use aws_config::Region;
use aws_sdk_s3::Client as S3Client;
use config::parsing::parse_config_from_file;
use tokio::{fs, io::AsyncWriteExt, process::Command};
use toml::Value;
use tracing::{error, info};
use util::{
    raw_err_str,
    telemetry::{setup_system_logger, LevelFilter},
};

// --- Env Vars --- //

/// The snapshot bucket environment variable
const ENV_SNAP_BUCKET: &str = "SNAPSHOT_BUCKET";
/// The bucket in which the relayer config files are stored
const ENV_CONFIG_BUCKET: &str = "CONFIG_BUCKET";
/// The path in the config bucket
const ENV_CONFIG_FILE: &str = "CONFIG_FILE";
/// The HTTP port to listen on
const ENV_HTTP_PORT: &str = "HTTP_PORT";
/// The websocket port to listen on
const ENV_WS_PORT: &str = "WEBSOCKET_PORT";
/// The P2P port to listen on
const ENV_P2P_PORT: &str = "P2P_PORT";
/// The public IP of the node (optional)
const ENV_PUBLIC_IP: &str = "PUBLIC_IP";

// --- Constants --- //

/// The path at which the relayer expects its config
const CONFIG_PATH: &str = "/config.toml";
/// The http port key name in the relayer config
const CONFIG_HTTP_PORT: &str = "http-port";
/// The websocket port key name in the relayer config
const CONFIG_WS_PORT: &str = "websocket-port";
/// The P2P port key name in the relayer config
const CONFIG_P2P_PORT: &str = "p2p-port";
/// The public IP key name in the relayer config
const CONFIG_PUBLIC_IP: &str = "public-ip";

/// The default AWS region to build an s3 client
const DEFAULT_AWS_REGION: &str = "us-east-2";

/// The location of the snapshot sidecar binary
const SIDECAR_BIN: &str = "/bin/snapshot-sidecar";
/// The location of the relayer binary
const RELAYER_BIN: &str = "/bin/renegade-relayer";

// --- Main --- //

#[tokio::main]
async fn main() -> Result<(), String> {
    setup_system_logger(LevelFilter::INFO);

    // Build an s3 client
    let s3_client = build_s3_client().await;

    // Fetch the config, modify it, and download the most recent snapshot
    fetch_config(&s3_client).await?;
    modify_config().await?;
    download_snapshot(&s3_client).await?;

    // Start both the snapshot sidecar and the relayer
    let bucket = read_env_var::<String>(ENV_SNAP_BUCKET)?;
    let mut sidecar = Command::new(SIDECAR_BIN)
        .args(["--config-path", CONFIG_PATH])
        .args(["--bucket", &bucket])
        .spawn()
        .expect("Failed to start snapshot sidecar process");
    let mut relayer = Command::new(RELAYER_BIN)
        .args(["--config-file", CONFIG_PATH])
        .spawn()
        .expect("Failed to start relayer process");

    let sidecar_result = sidecar.wait();
    let relayer_result = relayer.wait();
    let (sidecar_result, relayer_result) = tokio::try_join!(sidecar_result, relayer_result)
        .expect("Either snapshot sidecar or relayer process encountered an error");

    error!("sidecar exited with: {:?}", sidecar_result);
    error!("relayer exited with: {:?}", relayer_result);
    Ok(())
}

/// Fetch the relayer's config from s3
async fn fetch_config(s3: &S3Client) -> Result<(), String> {
    // Read in the fetch info from environment variables
    let bucket = read_env_var::<String>(ENV_CONFIG_BUCKET)?;
    let file = read_env_var::<String>(ENV_CONFIG_FILE)?;
    download_s3_file(&bucket, &file, CONFIG_PATH, s3).await
}

/// Modify the config using environment variables set at runtime
async fn modify_config() -> Result<(), String> {
    // Read the config file
    let config_content = fs::read_to_string(CONFIG_PATH)
        .await
        .map_err(raw_err_str!("Failed to read config file: {}"))?;
    let mut config: HashMap<String, Value> =
        toml::from_str(&config_content).map_err(raw_err_str!("Failed to parse config: {}"))?;

    // Add values from the environment variables
    let http_port = Value::String(read_env_var(ENV_HTTP_PORT)?);
    let ws_port = Value::String(read_env_var(ENV_WS_PORT)?);
    let p2p_port = Value::String(read_env_var(ENV_P2P_PORT)?);
    config.insert(CONFIG_HTTP_PORT.to_string(), http_port);
    config.insert(CONFIG_WS_PORT.to_string(), ws_port);
    config.insert(CONFIG_P2P_PORT.to_string(), p2p_port);

    if is_env_var_set(ENV_PUBLIC_IP) {
        let public_ip = Value::String(read_env_var(ENV_PUBLIC_IP)?);
        config.insert(CONFIG_PUBLIC_IP.to_string(), public_ip);
    }

    // Write the modified config back to the original file
    let new_config_content =
        toml::to_string(&config).map_err(raw_err_str!("Failed to serialize config: {}"))?;
    fs::write(CONFIG_PATH, new_config_content)
        .await
        .map_err(raw_err_str!("Failed to write config file: {}"))
}

/// Download the most recent snapshot
async fn download_snapshot(s3_client: &S3Client) -> Result<(), String> {
    let bucket = read_env_var::<String>(ENV_SNAP_BUCKET)?;

    // Parse the relayer's config
    let relayer_config =
        parse_config_from_file(CONFIG_PATH).expect("could not parse relayer config");
    let snap_path = format!("cluster-{}", relayer_config.cluster_id);

    // Get the latest snapshot
    let snaps = s3_client
        .list_objects_v2()
        .bucket(&bucket)
        .prefix(&snap_path)
        .send()
        .await
        .map_err(raw_err_str!("Failed to list objects in S3: {}"))?
        .contents
        .unwrap_or_default();
    if snaps.is_empty() {
        info!("no snapshots found in s3");
        return Ok(());
    }

    let latest = snaps.iter().max_by_key(|obj| obj.last_modified.as_ref().unwrap()).unwrap();
    let latest_key = latest.key.as_ref().unwrap();

    // Download the snapshot into the snapshot directory
    let path = format!("{}/snapshot.gz", relayer_config.raft_snapshot_path);
    download_s3_file(&bucket, latest_key, &path, s3_client).await
}

// --- Helpers --- //

/// Check whether the given environment variable is set
fn is_env_var_set(var_name: &str) -> bool {
    std::env::var(var_name).is_ok()
}

/// Read an environment variable
fn read_env_var<T: FromStr>(var_name: &str) -> Result<T, String>
where
    <T as FromStr>::Err: Debug,
{
    std::env::var(var_name)
        .map_err(raw_err_str!("{var_name} not set: {}"))?
        .parse::<T>()
        .map_err(|e| format!("Failed to read env var {}: {:?}", var_name, e))
}

/// Build an s3 client
async fn build_s3_client() -> S3Client {
    let region = Region::new(DEFAULT_AWS_REGION);
    let config = aws_config::from_env().region(region).load().await;
    aws_sdk_s3::Client::new(&config)
}

/// Download an s3 file to the given location
async fn download_s3_file(
    bucket: &str,
    key: &str,
    destination: &str,
    s3_client: &S3Client,
) -> Result<(), String> {
    // Get the object from S3
    let resp = s3_client
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .map_err(raw_err_str!("Failed to get object from S3: {}"))?;
    let body = resp.body.collect().await.map_err(raw_err_str!("Failed to read object body: {}"))?;

    // Create the directory if it doesn't exist
    if let Some(parent) = Path::new(destination).parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(raw_err_str!("Failed to create destination directory: {}"))?;
    }

    // Write the body to the destination file
    let mut file = fs::File::create(destination)
        .await
        .map_err(raw_err_str!("Failed to create destination file: {}"))?;
    file.write_all(&body.into_bytes())
        .await
        .map_err(raw_err_str!("Failed to write to destination file: {}"))?;

    Ok(())
}