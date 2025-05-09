use anyhow::Result;
use anyhow::anyhow;
use chrono::Local;
use clap::Parser;
use futures::StreamExt;
use redis::AsyncCommands;
use std::process::ExitCode;
use tokio::select;
use tokio::time::{Duration, sleep};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
struct Args {
  #[arg(long, default_value = "redis://127.0.0.1")]
  redis: String,

  #[arg(long)]
  mode: String,

  #[arg(long, default_value = "5.0")]
  subscribe_for_seconds: f64,

  #[arg(long)]
  and_publish_in_seconds: Option<f64>,
}

async fn try_check(args: &Args) -> Result<()> {
  let _ = redis::Client::open(args.redis.to_string())?.get_multiplexed_async_connection().await?;
  println!("Redis is available.");
  Ok(())
}

async fn try_test(args: &Args) -> Result<()> {
  let client = redis::Client::open(args.redis.to_string())?;
  let mut con = client.get_multiplexed_async_connection().await?;

  let _: () = con.set("key", "value").await?;
  let val: String = con.get("key").await?;

  println!("Got value: {}", val);

  (val == "value").then(|| ()).ok_or(anyhow!("Wrong value"))
}

async fn try_pub(args: &Args) -> Result<()> {
  let client = redis::Client::open(args.redis.to_string())?;
  let mut con = client.get_multiplexed_async_connection().await?;
  con.publish::<_, _, ()>("redis_channel", format!("published from rust at {}", Local::now())).await?;
  Ok(())
}

async fn try_sub(args: &Args) -> Result<()> {
  let client = redis::Client::open(args.redis.to_string())?;
  let mut pubsub = client.get_async_pubsub().await?;

  pubsub.subscribe("redis_channel").await?;
  let mut stream = pubsub.on_message();

  let mut con = client.get_multiplexed_async_connection().await?;
  let mut listen = async || {
    while let Some(msg) = stream.next().await {
      let payload: String = msg.get_payload().unwrap();
      println!(">> {}", payload);
    }
  };

  let cancel_token = CancellationToken::new();
  let cloned_token = cancel_token.clone();
  let sleed_duration = args.subscribe_for_seconds;
  tokio::task::spawn_local(async move {
    sleep(Duration::from_secs_f64(sleed_duration)).await;
    cloned_token.cancel();
  });

  if let Some(delay) = args.and_publish_in_seconds {
    let cloned_token = cancel_token.clone();
    tokio::task::spawn_local(async move {
      select! {
        _ = async move {
          sleep(Duration::from_secs_f64(delay)).await;
          println!("publishing from rust from a separate green thread");
          con.publish::<_, _, ()>("redis_channel", "published from rust after a delay").await.unwrap();
        } => {
          println!("publishing from rust from a separate green thread finished");
        },
        _ = cloned_token.cancelled() => {
          println!("terminating publishing task by cancellationtoken");
        },
      }
    });
  };

  println!("listening to messages on `redis_channel`");

  select! {
    _ = listen() => {
      println!("terminating because the pubsub channel is closed");
      Ok(())
    },
    _ = cancel_token.cancelled() => {
      println!("terminating by timeout");
      Ok(())
    },
  }
}

#[tokio::main]
async fn main() -> ExitCode {
  let args = Args::parse();

  let local = tokio::task::LocalSet::new();

  local
    .run_until(async {
      match args.mode.as_str() {
        "check" => try_check(&args).await,
        "test" => try_test(&args).await,
        "pub" => try_pub(&args).await,
        "sub" => try_sub(&args).await,
        _ => Err(anyhow!("This `--mode` value is not supported.")),
      }
      .map_or_else(
        |err| {
          println!("Error: {}", err);
          1
        },
        |_| 0,
      )
      .into()
    })
    .await
}
