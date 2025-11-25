use anyhow::anyhow;
use bollard::{
    Docker,
    query_parameters::{EventsOptionsBuilder, InspectContainerOptions},
};
use std::{
    collections::HashMap,
    env,
    fs::{self, File},
    path::Path,
};
use tokio::time::{Duration, sleep};
use tokio_postgres::{Client, NoTls};
use tokio_stream::StreamExt;

const HEALTHCHECK_FILE: &str = "/healthcheck/manager-ready";

async fn connect_to_master() -> Client {
    let citus_host = env::var("CITUS_HOST").unwrap_or_else(|_| "master".to_string());
    let postgres_pass = env::var("POSTGRES_PASSWORD").unwrap_or_default();
    let postgres_user = env::var("POSTGRES_USER").unwrap_or_else(|_| "postgres".to_string());
    let postgres_db = env::var("POSTGRES_DB").unwrap_or_else(|_| postgres_user.clone());

    let connection_string = format!(
        "host={} user={} password={} dbname={}",
        citus_host, postgres_user, postgres_pass, postgres_db
    );

    loop {
        match tokio_postgres::connect(&connection_string, NoTls).await {
            Ok((client, connection)) => {
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        eprintln!("connection error: {}", e);
                    }
                });
                eprintln!("connected to {}", citus_host);
                return client;
            }
            Err(e) => {
                eprintln!(
                    "Could not connect to {}, trying again in 1 second: {}",
                    citus_host, e
                );
                sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn add_worker(client: &Client, host: &str) -> Result<(), tokio_postgres::Error> {
    eprintln!("adding {}", host);
    client
        .execute("SELECT master_add_node($1, 5432)", &[&host])
        .await?;
    Ok(())
}

async fn remove_worker(client: &Client, host: &str) -> Result<(), tokio_postgres::Error> {
    eprintln!("removing {}", host);
    client
        .batch_execute(&format!(
            "DELETE FROM pg_dist_placement WHERE groupid = (
                SELECT groupid FROM pg_dist_node
                WHERE nodename = '{host}' AND nodeport = 5432 LIMIT 1
            );
            SELECT master_remove_node('{host}', 5432)",
            host = host,
        ))
        .await?;
    Ok(())
}

async fn docker_checker() -> anyhow::Result<()> {
    let docker = Docker::connect_with_unix_defaults()?;

    let client = connect_to_master().await;

    let my_hostname = env::var("HOSTNAME")?;
    let container = docker
        .inspect_container(&my_hostname, None::<InspectContainerOptions>)
        .await?;

    let compose_project = container
        .config
        .and_then(|config| Some(config.labels?.get("com.docker.compose.project")?.clone()))
        .ok_or_else(|| anyhow!("Could not find Compose project label"))?;

    eprintln!("found Compose project: {}", compose_project);

    let filters = HashMap::from([
        (
            "event".to_owned(),
            vec!["health_status: healthy".to_owned(), "destroy".to_owned()],
        ),
        (
            "label".to_owned(),
            vec![
                format!("com.docker.compose.project={}", compose_project),
                "com.citusdata.role=Worker".to_owned(),
            ],
        ),
        ("type".to_owned(), vec!["container".to_owned()]),
    ]);

    eprintln!("listening for events...");

    if let Some(parent) = Path::new(HEALTHCHECK_FILE).parent() {
        fs::create_dir_all(parent)?;
    }
    File::create(HEALTHCHECK_FILE)?;

    let events_options = EventsOptionsBuilder::default().filters(&filters).build();

    let mut events = docker.events(Some(events_options));

    while let Some(event_result) = events.next().await {
        match event_result {
            Ok(event) => {
                let worker_name = event
                    .actor
                    .and_then(|actor| Some(actor.attributes?.get("name")?.clone()))
                    .unwrap_or_default();

                let action = event.action.unwrap_or_default();

                let result = match action.as_str() {
                    "health_status: healthy" => add_worker(&client, &worker_name).await,
                    "destroy" => remove_worker(&client, &worker_name).await,
                    _ => Ok(()),
                };

                if let Err(e) = result {
                    eprintln!("Error processing event: {}", e);
                }
            }
            Err(e) => {
                eprintln!("Error receiving event: {}", e);
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    if Path::new(HEALTHCHECK_FILE).exists() {
        let _ = fs::remove_file(HEALTHCHECK_FILE);
    }

    tokio::select! {
        result = docker_checker() => {
            if let Err(e) = result {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            eprintln!("shutting down...");
        }
    }
}
