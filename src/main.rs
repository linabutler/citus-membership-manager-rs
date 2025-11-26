use std::{
    collections::HashMap,
    env,
    fs::{self, File},
    path::Path,
};

use anyhow::{Context, anyhow};
use bollard::{
    Docker,
    query_parameters::{EventsOptionsBuilder, InspectContainerOptions},
};
use tokio::time::{Duration, sleep};
use tokio_postgres::{Client, Config, NoTls};
use tokio_stream::StreamExt;

const HEALTHCHECK_FILE: &str = "/healthcheck/manager-ready";

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    if Path::new(HEALTHCHECK_FILE).exists() {
        let _ = fs::remove_file(HEALTHCHECK_FILE);
    }

    tokio::select! {
        result = run() => result?,

        _ = tokio::signal::ctrl_c() => {
            eprintln!("shutting down...");
        }
    }

    Ok(())
}

async fn run() -> anyhow::Result<()> {
    let docker = Docker::connect_with_unix_defaults()?;
    let mut db = connect().await;

    let my_hostname = env::var("HOSTNAME")?;
    let container = docker
        .inspect_container(&my_hostname, None::<InspectContainerOptions>)
        .await
        .context(format!("couldn't fetch container `{my_hostname}`"))?;

    let compose_project = container
        .config
        .and_then(|config| Some(config.labels?.get("com.docker.compose.project")?.clone()))
        .ok_or_else(|| anyhow!("container `{my_hostname}` missing Compose project label"))?;

    eprintln!("found Compose project: `{compose_project}`");

    let filters = HashMap::from([
        (
            "event".to_owned(),
            vec!["health_status: healthy".to_owned(), "destroy".to_owned()],
        ),
        (
            "label".to_owned(),
            vec![
                format!("com.docker.compose.project={compose_project}"),
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

    let mut events = docker.events(Some(
        EventsOptionsBuilder::default().filters(&filters).build(),
    ));

    while let Some(event) = events.try_next().await? {
        let action = match event.action {
            Some(action) => action,
            None => continue,
        };

        let worker = match event
            .actor
            .and_then(|actor| Some(actor.attributes?.get("name")?.clone()))
        {
            Some(worker) => worker,
            None => continue,
        };

        let result = match action.as_str() {
            "health_status: healthy" => add_worker(&db, &worker).await,
            "destroy" => remove_worker(&mut db, &worker).await,
            _ => continue,
        };

        if let Err(err) = result {
            eprintln!("error processing event: {err}");
        }
    }

    Ok(())
}

async fn connect() -> Client {
    let host = env::var("CITUS_HOST").unwrap_or_else(|_| "master".to_owned());
    let user = env::var("POSTGRES_USER").unwrap_or_else(|_| "postgres".to_owned());
    let password = env::var("POSTGRES_PASSWORD").ok();
    let db = env::var("POSTGRES_DB").unwrap_or_else(|_| user.clone());

    let mut config = Config::new();
    config.host(&host).user(user).dbname(db);
    if let Some(password) = password {
        config.password(password);
    }

    loop {
        match config.connect(NoTls).await {
            Ok((client, connection)) => {
                let _guard = tokio::spawn(async move {
                    if let Err(err) = connection.await {
                        eprintln!("connection error: {err}");
                    }
                });
                eprintln!("connected to `{host}`");
                return client;
            }
            Err(err) => {
                eprintln!("couldn't connect to `{host}`, trying again in 1 second: {err}");
                sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn add_worker(client: &Client, host: &str) -> anyhow::Result<()> {
    eprintln!("adding `{host}`");
    client
        .execute("SELECT master_add_node($1, 5432)", &[&host])
        .await?;
    Ok(())
}

async fn remove_worker(client: &mut Client, host: &str) -> anyhow::Result<()> {
    eprintln!("removing `{host}`");
    let tx = client.transaction().await?;
    tx.execute(
        "DELETE FROM pg_dist_placement WHERE groupid = (
             SELECT groupid FROM pg_dist_node
             WHERE nodename = $1 AND nodeport = 5432 LIMIT 1
         )",
        &[&host],
    )
    .await?;
    tx.execute("SELECT master_remove_node($1, 5432)", &[&host])
        .await?;
    tx.commit().await?;
    Ok(())
}
