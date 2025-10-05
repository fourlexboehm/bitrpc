use std::future::pending;
use std::num::NonZeroUsize;
use std::{path::PathBuf, str::FromStr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bitrpc::{bitcode, cyper::CyperTransport, RpcError};
use compio_buf::bytes::Bytes;
use compio_dispatcher::Dispatcher;
use compio_fs::File;
use compio_io::AsyncWriteAtExt;
use http::Uri;
use url::Url;

use compio_h3_rpc::RpcClient;

const DEFAULT_TASKS_PER_THREAD: usize = 40;

#[compio_macros::main]
async fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() < 3 {
        eprintln!("Usage: {} <HOST|URI> [PORT] <OUT> [TASKS_PER_THREAD]", args[0]);
        std::process::exit(1);
    }

    let raw_target = &args[1];
    let (port_override, out_idx) = if args.len() >= 4 && args[2].parse::<u16>().is_ok() {
        (Some(&args[2]), 3)
    } else {
        (None, 2)
    };
    let outpath = PathBuf::from(&args[out_idx]);

    let port_override = port_override.map(|p| p.parse::<u16>().expect("port must be a number"));

    let tasks_per_thread = args
        .get(out_idx + 1)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_TASKS_PER_THREAD);
    if tasks_per_thread == 0 {
        eprintln!("tasks per thread must be at least 1");
        std::process::exit(1);
    }
    let uri = build_h3_uri(raw_target, port_override);
    let uri_str = uri.to_string();

    let dispatcher_count = NonZeroUsize::new(num_cpus::get_physical())
        .unwrap_or(NonZeroUsize::MIN)
        .get();
    let total_tasks = dispatcher_count * tasks_per_thread;
    println!(
        "Starting benchmark: {} dispatcher threads × {} tasks per thread ({} total loops)",
        dispatcher_count,
        tasks_per_thread,
        total_tasks
    );
    println!("Target: {}", uri);

    let mut warmup_client = RpcClient::new(CyperTransport::new(uri_str.clone()));
    let warmup_response = warmup_client
        .add(10, "hello".into())
        .await
        .expect("warmup rpc failed");

    let mut file = File::create(&outpath).await.unwrap();
    file.write_all_at(Bytes::from(bitcode::encode(&warmup_response)), 0).await.unwrap();
    println!("Sample response saved to: {}", outpath.display());

    let counter = Arc::new(AtomicU64::new(0));
    let start = std::time::Instant::now();

    let mut dispatchers = Vec::with_capacity(dispatcher_count);
    for _ in 0..dispatcher_count {
        dispatchers.push(
            Dispatcher::builder()
                .worker_threads(NonZeroUsize::new(1).unwrap())
                .build()
                .expect("failed to build client dispatcher"),
        );
    }

    for dispatcher in &dispatchers {
        for _ in 0..tasks_per_thread {
            let counter = counter.clone();
            let start = start;
            let uri = uri_str.clone();

            match dispatcher.dispatch(move || async move {
                let mut client = RpcClient::new(CyperTransport::new(uri));

                loop {
                    match client.add(10, "hello".into()).await {
                        Ok(_) => {
                            let count = counter.fetch_add(1, Ordering::Relaxed) + 1;

                            if count % 10000 == 0 {
                                let elapsed = start.elapsed().as_secs_f64();
                                let rps = count as f64 / elapsed;
                                println!("Completed: {} requests, RPS: {:.2}", count, rps);
                            }
                        }
                        Err(err) => {
                            if let RpcError::Transport { message } = &err {
                                if message.contains("connecting already in progress") {
                                    continue;
                                }
                            }

                            eprintln!("request failed: {:?}", err);
                        }
                    }
                }
            }) {
                Ok(rx) => {
                    drop(rx);
                }
                Err(_err) => {
                    eprintln!("dispatcher unavailable; dropping client worker");
                }
            }
        }
    }

    pending::<()>().await;
}

fn build_h3_uri(raw: &str, port_override: Option<u16>) -> Uri {
    let mut url = if raw.contains("://") {
        Url::parse(raw).expect("invalid URI")
    } else {
        Url::parse(&format!("https://{raw}")).expect("invalid host")
    };

    if url.scheme() != "https" {
        url.set_scheme("https").expect("set https scheme");
    }

    if let Some(port) = port_override {
        url.set_port(Some(port)).expect("set port");
    } else if url.port().is_none() {
        url.set_port(Some(4433)).expect("set default port");
    }

    if url.path().is_empty() {
        url.set_path("/");
    }

    Uri::from_str(url.as_str()).expect("valid HTTP URI")
}
